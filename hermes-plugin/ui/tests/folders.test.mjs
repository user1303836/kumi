// node --test: the Sample folders control (src/shared/SampleFolders.tsx): what it shows, and what its button does with
// a typed path: the plugin's folder guard answers first, and only a folder it takes is asked for in the chat.
import assert from 'node:assert/strict'
import { test } from 'node:test'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { load } from './load.mjs'

const { SampleFolders, addTyped } = await load('src/shared/SampleFolders.tsx', { react: 'real' })
const { KumiStore } = await load('src/shared/store.ts')
const { addFolderRequest, folderExample } = await load('src/shared/format.ts')
const { english } = await load('src/shared/i18n.ts')

const SAMPLES = 'C:\\Users\\you\\Samples'

test('a refused folder says why in the guard’s words and asks nothing; one it takes is asked for as the guard keeps it', async () => {
  const asked = []
  const ask = outcome => async (folder, known) => (asked.push([folder, known]), outcome)
  // The backend's answers, as `POST /library/folder` gives them, through the store.
  const store = new KumiStore(async (path, init) => {
    assert.deepEqual([path, init.method], ['/library/folder', 'POST'])
    const typed = init.body.folder
    if (typed === 'Samples') return { refusal: "'Samples' isn't a full path." }
    if (typed === 'offline') throw new Error('503: {"detail":"Kumi is off"}')
    return { folder: SAMPLES, known: typed === SAMPLES }
  })
  const check = typed => store.checkFolder(typed)
  assert.deepEqual(await addTyped('Samples', check, ask('sent'), english), { refused: true, text: "Kumi didn't add the folder: 'Samples' isn't a full path." })
  assert.deepEqual(await addTyped('offline', check, ask('sent'), english), { refused: true, text: english('folders.failed', 'Kumi is off') })
  assert.deepEqual(asked, [], 'nothing reached the chat')
  assert.deepEqual(await addTyped(`"${SAMPLES}\\"`, check, ask('sent'), english), { refused: false, text: english('folders.sent') })
  assert.deepEqual(asked, [[SAMPLES, false]], 'the path the guard keeps, never the one typed')
  const again = await addTyped(SAMPLES, check, ask('copied'), english)
  assert.equal(again.text, `${english('folders.known')} ${english('folders.copied')}`)
  assert.deepEqual(asked[1], [SAMPLES, true], 'one Kumi has already: a scan is asked, not another add')
  assert.equal(await addTyped(SAMPLES, check, ask('none'), english), null, 'the host said why itself')
})

test('the request names the folder as it is, and the field shows a path of the computer Kumi runs on', () => {
  assert.equal(addFolderRequest(SAMPLES), 'Kumi, add this sample folder and scan it: `C:\\Users\\you\\Samples`')
  assert.equal(addFolderRequest('/Volumes/Work/Drum `Hits`'), 'Kumi, add this sample folder and scan it: /Volumes/Work/Drum `Hits`')
  assert.equal(folderExample('x86_64-pc-windows-msvc'), SAMPLES)
  assert.equal(folderExample('aarch64-apple-darwin'), '/Users/you/Samples')
  assert.equal(folderExample(undefined), '/Users/you/Samples')
})

test('the control lists the folders and offers the dialog only where there is one, and a copy where nothing can be sent', () => {
  const draw = props => renderToStaticMarkup(createElement(SampleFolders, { folders: ['D:\\Samples', SAMPLES], example: SAMPLES, check: async () => ({}), ask: async () => 'none', ...props }))
  const desktop = draw({ pick: async () => null })
  for (const text of ['D:\\Samples', SAMPLES, english('folders.choose'), english('folders.add'), english('folders.note')]) assert.ok(desktop.includes(text), `${text} in ${desktop}`)
  assert.ok(desktop.includes(`placeholder="${english('folders.pathHint', SAMPLES).replaceAll("'", '&#x27;')}"`), desktop)
  const dashboard = draw({ copies: true })
  assert.ok(dashboard.includes(english('folders.copy')) && !dashboard.includes(english('folders.add')) && !dashboard.includes(english('folders.choose')), dashboard)
  assert.ok(draw({ folders: [] }).includes(english('folders.none')))
  assert.ok(draw({ folders: undefined }).includes(english('folders.reading')))
  assert.ok(!draw({ folders: null }).includes(english('folders.none')), 'folders that can’t be read are not said to be none')
})
