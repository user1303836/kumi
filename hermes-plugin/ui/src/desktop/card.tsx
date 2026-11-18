// The inline card Desktop draws for the audio a `kumi_listen` call measured (its `MEDIA:` line): both waveforms when
// it compared two files, the measured numbers side by side, and the differences Kumi computed. Every number is a
// local measurement; a model's reading of them shows only when the user turned it on, and says so.
import type { ToolMediaRendererProps } from '@hermes/plugin-sdk'
import { Badge, Button, Tip } from '@kumi/controls'
import { T } from '@kumi/theme'
import type { ReactNode } from 'react'
import { useEffect, useLayoutEffect, useRef, useState } from 'react'

import { clean, num, signed } from '../shared/format.ts'
import { type Audition, useKumi, usePlaying } from '../shared/hooks.ts'
import type { Translate } from '../shared/i18n.ts'
import { displayReferenceName, listenView, parseListen, referenceName, roundKey, roundWords, type ListenView } from '../shared/listen.ts'
import type { KumiStore } from '../shared/store.ts'
import type { ListenReference, ListenResult, SampleWindow } from '../shared/types.ts'
import { FOCUSABLE, S, TYPE, useT } from '../shared/ui.tsx'

/** A data URL's bytes, decoded to peak levels for a waveform. */
async function peaksOf(dataUrl: string, buckets = 96): Promise<number[]> {
  const comma = dataUrl.indexOf(',')
  const binary = atob(dataUrl.slice(comma + 1))
  const bytes = new Uint8Array(binary.length)
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i)
  const decoded = await new OfflineAudioContext(1, 1, 44100).decodeAudioData(bytes.buffer)
  const samples = decoded.getChannelData(0)
  const step = Math.max(1, Math.floor(samples.length / buckets))
  const peaks: number[] = []
  for (let start = 0; start < samples.length && peaks.length < buckets; start += step) {
    let peak = 0
    for (let i = start; i < Math.min(samples.length, start + step); i++) peak = Math.max(peak, Math.abs(samples[i]))
    peaks.push(peak)
  }
  const top = Math.max(...peaks, 1e-6)
  return peaks.map(peak => peak / top)
}

/** A file's waveform with a play button, drawn from its audio (`audition`): the listen and generate cards' shared part.
 *  `label` names it for screen readers; `caption` is the text shown beside the button (the label, unless a card already
 *  shows the name). */
export function Wave({ audition, id, label, caption = label }: { audition: Audition; id: string; label: string; caption?: string }) {
  const t = useT()
  const [peaks, setPeaks] = useState<null | number[]>(null)
  const [failed, setFailed] = useState(false)
  const playing = usePlaying(audition) === id
  useEffect(() => {
    let live = true
    audition
      .url(id)
      .then(peaksOf)
      .then(found => live && setPeaks(found))
      .catch(() => live && setFailed(true))
    return () => {
      live = false
    }
  }, [audition, id])
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 3, flex: '1 1 160px', minWidth: 140 }}>
      <div style={{ ...S.row, justifyContent: 'space-between' }}>
        <span style={{ ...S.faint, ...S.ellipsis }}>{caption}</span>
        <Button
          kind="quiet"
          label={playing ? t('listen.stop', label) : t('listen.play', label)}
          onClick={() => (playing ? audition.stop() : void audition.play(id).catch(() => setFailed(true)))}
          pressed={playing}
        >
          {playing ? '■' : '▶'}
        </Button>
      </div>
      <svg aria-label={t('listen.waveform', label)} height={36} preserveAspectRatio="none" role="img" viewBox="0 0 96 36" width="100%">
        {(peaks ?? []).map((peak, index) => (
          <rect fill={playing ? T.accent : T.secondary} height={Math.max(1, peak * 34)} key={index} opacity={0.85} width={0.7} x={index + 0.15} y={18 - Math.max(0.5, peak * 17)} />
        ))}
        {!peaks && <line stroke={T.hairline} x1={0} x2={96} y1={18} y2={18} />}
      </svg>
      {failed && <span style={S.faint}>{t('listen.audioGone')}</span>}
    </div>
  )
}

function Rows({ view }: { view: ListenView }) {
  const t = useT()
  const compared = view.referenceTitle !== undefined
  const cell = { padding: '2px 6px', textAlign: 'right', fontVariantNumeric: 'tabular-nums', whiteSpace: 'nowrap' } as const
  return (
    // The card sits in the chat's markdown, whose prose styles reach any table in it: a 2em margin above and below,
    // a taller line, and a bright line under each row and the header. The table sets its own, so only its rows' quiet
    // lines show. In a narrow chat (the map docked beside it) the columns don't fit: the table scrolls sideways in its
    // own region, focusable so the keyboard can scroll it, rather than spilling past the card, where the chat cut off
    // the reference and the differences.
    <div aria-label={t('measurements.tab')} className={FOCUSABLE} role="region" style={{ overflowX: 'auto', maxWidth: '100%' }} tabIndex={0}>
      <table style={{ borderCollapse: 'collapse', ...TYPE.small, color: T.text, width: '100%', margin: 0 }}>
        {compared && (
          <thead style={{ border: 'none' }}>
            <tr style={{ ...TYPE.caption, color: T.muted, border: 'none' }}>
              <th style={{ ...cell, textAlign: 'left', fontWeight: 400 }} />
              <th style={{ ...cell, fontWeight: 400 }}>{t('listen.this')}</th>
              <th style={{ ...cell, fontWeight: 400 }}>{t('listen.reference')}</th>
              <th style={{ ...cell, fontWeight: 400 }}>{t('listen.difference')}</th>
            </tr>
          </thead>
        )}
        <tbody>
          {view.rows.map(row => (
            <tr key={row.label} style={{ border: 'none', borderTop: `1px solid ${T.hairline}` }}>
              <td style={{ ...cell, textAlign: 'left', color: T.muted }}>{row.label}</td>
              <td style={cell}>{row.mine}</td>
              {compared && <td style={{ ...cell, color: T.muted }}>{row.reference ?? '—'}</td>}
              {compared && <td style={{ ...cell, color: row.direction === 0 ? T.muted : T.text, fontWeight: 600 }}>{row.delta ?? '—'}</td>}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

/** With a reference: each band's level in this file (a bar) and in the reference (a line across it), on one scale. Both
 *  are the band's share of its own file's energy, so loudness doesn't move them: the shapes compare as they are. */
function Overlay({ view }: { view: ListenView }) {
  const t = useT()
  const values = view.overlay.flatMap(band => [band.mine, band.reference])
  const low = Math.min(...values) - 3
  const high = Math.max(...values) + 1
  const at = (value: number) => `${Math.max(2, ((value - low) / (high - low)) * 100)}%`
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 3 }}>
      <span style={{ ...S.row, gap: 10, flexWrap: 'wrap' }}>
        <span style={S.faint}>{t('listen.overlay')}</span>
        <span style={{ ...S.row, gap: 4, ...S.faint }}>
          <span aria-hidden style={{ width: 8, height: 8, background: T.accent, opacity: 0.8 }} />
          {t('listen.this')}
        </span>
        <span style={{ ...S.row, gap: 4, ...S.faint }}>
          <span aria-hidden style={{ width: 10, borderTop: `2px solid ${T.text}` }} />
          {t('listen.reference')}
        </span>
      </span>
      <div style={{ display: 'flex', alignItems: 'stretch', gap: 3, height: 64 }}>
        {view.overlay.map(band => {
          const name = bandName(t, band.name)
          const label = t('listen.overlayBand', name, band.hz, num(band.mine), num(band.reference), signed(band.mine - band.reference))
          return (
            <Tip key={band.name} label={label}>
              <div role="img" aria-label={label} style={{ flex: 1, display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 2, minWidth: 0 }}>
                <div style={{ position: 'relative', flex: 1, width: '100%' }}>
                  <div style={{ position: 'absolute', left: '18%', right: '18%', bottom: 0, height: at(band.mine), background: T.accent, opacity: 0.8 }} />
                  <div style={{ position: 'absolute', left: 0, right: 0, bottom: at(band.reference), borderTop: `2px solid ${T.text}` }} />
                </div>
                <span style={{ ...TYPE.caption, color: T.faint, ...S.ellipsis, maxWidth: '100%' }}>{name}</span>
              </div>
            </Tip>
          )
        })}
      </div>
    </div>
  )
}

function Bands({ view }: { view: ListenView }) {
  const t = useT()
  if (view.overlay.length) return <Overlay view={view} />
  if (!view.bands.length) return null
  const top = Math.max(...view.bands.map(band => Math.abs(band.value)), view.bandsAre === 'difference' ? 3 : 1)
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 3 }}>
      <span style={S.faint}>{view.bandsAre === 'difference' ? t('listen.bandsDifference') : t('listen.bandsLevel')}</span>
      <div style={{ display: 'flex', alignItems: 'stretch', gap: 3, height: 54 }}>
        {view.bands.map(band => {
          const share = Math.min(1, Math.abs(band.value) / top)
          const name = bandName(t, band.name)
          const label = view.bandsAre === 'difference' ? `${name}: ${signed(band.value)} dB` : `${name}: ${num(band.value)} dB`
          return (
            <Tip key={band.name} label={label}>
              <div role="img" aria-label={label} style={{ flex: 1, display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 2, minWidth: 0 }}>
                <div style={{ position: 'relative', flex: 1, width: '100%' }}>
                  {view.bandsAre === 'difference' ? (
                    <div
                      style={{
                        position: 'absolute',
                        left: '15%',
                        right: '15%',
                        background: band.value < 0 ? T.secondary : T.accent,
                        opacity: 0.8,
                        ...(band.value < 0 ? { top: '50%', height: `${share * 50}%` } : { bottom: '50%', height: `${share * 50}%` })
                      }}
                    />
                  ) : (
                    <div style={{ position: 'absolute', left: '15%', right: '15%', bottom: 0, height: `${Math.max(4, (1 - share) * 100)}%`, background: T.secondary, opacity: 0.8 }} />
                  )}
                  {view.bandsAre === 'difference' && <div style={{ position: 'absolute', left: 0, right: 0, top: '50%', borderTop: `1px solid ${T.hairline}` }} />}
                </div>
                <span style={{ ...TYPE.caption, color: T.faint, ...S.ellipsis, maxWidth: '100%' }}>{name}</span>
              </div>
            </Tip>
          )
        })}
      </div>
    </div>
  )
}

function Interpretation({ view }: { view: ListenView }) {
  const t = useT()
  const [open, setOpen] = useState(false)
  if (view.interpretationError) return <span style={S.faint}>{t('listen.noInterpretation', view.interpretationError)}</span>
  if (!view.interpretation) return null
  const text = view.interpretation.text
  const long = text.length > 320
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 3, borderLeft: `2px solid ${T.border}`, paddingLeft: 8 }}>
      <span style={S.faint}>{view.interpretation.label}</span>
      <span style={{ ...S.muted, whiteSpace: 'pre-wrap' }}>{long && !open ? `${text.slice(0, 320)}…` : text}</span>
      {long && (
        <span>
          <Button kind="quiet" onClick={() => setOpen(!open)}>
            {open ? t('listen.less') : t('listen.more')}
          </Button>
        </span>
      )}
    </div>
  )
}

/** The bands the runtime names (the analysis's ten, the profile's eight regions among them) in the card's words; a
 *  band Kumi doesn't know shows by the name it came with. These lookups are Maps because their keys come from the
 *  runtime: an object would answer a name like `constructor` with its own prototype's function. */
const BANDS = new Map([
  ['sub', 'listen.bandNames.sub'],
  ['bass', 'listen.bandNames.bass'],
  ['upper bass', 'listen.bandNames.upperBass'],
  ['low mids', 'listen.bandNames.lowMids'],
  ['mids', 'listen.bandNames.mids'],
  ['upper mids', 'listen.bandNames.upperMids'],
  ['presence', 'listen.bandNames.presence'],
  ['bite', 'listen.bandNames.bite'],
  ['brilliance', 'listen.bandNames.brilliance'],
  ['air', 'listen.bandNames.air']
])

function bandName(t: Translate, name: string): string {
  const key = BANDS.get(name)
  return key ? t(key) : name
}

/** The reference profile's quantities by the runtime's codes (`listen_reference.rs`); `regions` is what the
 *  availability calls all eight bands at once. */
const QUANTITIES = new Map([
  ['integrated', 'listen.loudness'],
  ['plr', 'listen.peakToLoudness'],
  ['crest', 'listen.crest'],
  ['tilt', 'listen.tilt'],
  ['range', 'listen.range'],
  ['centroid', 'listen.brightness'],
  ['regions', 'listen.bandsLevel'],
  ['low_width', 'listen.profile.lowWidth'],
  ['noise', 'listen.profile.noise'],
  ['attack', 'listen.profile.attack'],
  ['decay', 'listen.profile.decay'],
  ['sustain', 'listen.profile.sustain']
])

function quantityName(t: Translate, code: string): string {
  const key = QUANTITIES.get(code)
  if (key) return t(key)
  if (code.startsWith('region/')) return t('listen.profile.region', bandName(t, code.slice('region/'.length)))
  return clean(code, 60)
}

/** Why a value is missing, by the runtime's code. A code Kumi doesn't know yet shows the runtime's own sentence, else
 *  "Not measured": never the code itself. The two bandwidth reasons name the file's top frequency (`nyquist`, Hz). */
const REASONS = new Map([
  ['not_requested', 'listen.profile.reasons.not_requested'],
  ['zero_energy', 'listen.profile.reasons.zero_energy'],
  ['too_short_for_profile_frames', 'listen.profile.reasons.too_short_for_profile_frames'],
  ['no_usable_profile_energy', 'listen.profile.reasons.no_usable_profile_energy'],
  ['too_short_for_loudness', 'listen.profile.reasons.too_short_for_loudness'],
  ['below_loudness_gate', 'listen.profile.reasons.below_loudness_gate'],
  ['too_short_for_range', 'listen.profile.reasons.too_short_for_range'],
  ['mono', 'listen.profile.reasons.mono'],
  ['no_low_band_energy', 'listen.profile.reasons.no_low_band_energy'],
  ['not_measured', 'listen.profile.reasons.not_measured'],
  ['no_reference_measurement', 'listen.profile.reasons.no_reference_measurement'],
  ['above_nyquist', 'listen.profile.reasons.above_nyquist'],
  ['partial_frequency_coverage', 'listen.profile.reasons.partial_frequency_coverage']
])

function reasonWords(t: Translate, code: unknown, words?: unknown, nyquist?: unknown): string {
  const key = typeof code === 'string' ? REASONS.get(code) : undefined
  if (key) return t(key, typeof nyquist === 'number' && nyquist > 0 ? num(nyquist / 1000, 1) : '')
  return clean(words, 300) || t('listen.profile.reasons.not_measured')
}

/** The part of a file that was measured: `10–12 s (2 s at 48 kHz, 96,000 samples)`. Seconds to the hundredth; the
 *  exact count is the samples. */
function windowLine(t: Translate, window: SampleWindow | null | undefined): string | undefined {
  if (!window || !(window.sample_rate > 0) || !Number.isFinite(window.frame_count) || !Number.isFinite(window.start_frame)) return undefined
  const start = window.start_frame / window.sample_rate
  const seconds = window.frame_count / window.sample_rate
  return t('listen.facts.window', num(start, 2), num(start + seconds, 2), num(seconds, 2), num(window.sample_rate / 1000, 2), num(window.frame_count, 0))
}

/** Which file a reference is and which part of it was measured, in plain words: how Kumi knows the file, the part of
 *  the original an excerpt came from, the part measured, and that Kumi kept a copy to play. Never an id, an inode or
 *  a timestamp: the model has those; the producer reads these behind "Details". */
function referenceFacts(t: Translate, reference: ListenReference): string[] {
  const identity = reference.origin?.identity ?? reference.identity
  const sound = identity?.kind === 'sound' && identity.sound_id ? clean(identity.sound_id.split(/[\\/]/).pop() ?? '', 80) : ''
  const original = windowLine(t, reference.origin?.window)
  const measured = windowLine(t, reference.window)
  const lines = [
    identity?.kind === 'file' ? t('listen.facts.file') : '',
    sound ? t('listen.facts.sound', sound) : '',
    original ? t('listen.facts.original', original) : '',
    measured ? (original ? t('listen.facts.kept', measured) : t('listen.facts.measured', measured)) : '',
    reference.artifact_id ? t('listen.facts.retained') : ''
  ]
  return lines.filter(Boolean)
}

/** One quiet disclosure for what a producer rarely needs: how a number was made, which file, which part of it. It
 *  starts closed; its summary is a 24 px target. */
function Details({ children }: { children: ReactNode }) {
  const t = useT()
  return (
    <details>
      <summary className={FOCUSABLE} style={{ ...TYPE.small, color: T.muted, cursor: 'pointer', padding: '4px 0', width: 'fit-content' }}>
        {t('listen.details')}
      </summary>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 4, paddingTop: 4, overflowWrap: 'anywhere' }}>{children}</div>
    </details>
  )
}

/** The width the profile's four columns need to read without breaking a word; a narrower card (the chat column beside
 *  the map) stacks each quantity instead. */
const PROFILE_TABLE_LEAST = 400

/** A comparison against several references: per quantity, this file, the references' target with its spread, and
 *  where this file sits; then each reference with its player and what it couldn't measure. How the target is made,
 *  each reference's own numbers and which part of which file was measured sit behind "Details". The spread is never
 *  a confidence, and nothing here is a percentage. */
function ReferenceProfile({ result, audition }: { result: ListenResult; audition: Audition }) {
  const t = useT()
  const box = useRef<HTMLElement | null>(null)
  const [narrow, setNarrow] = useState(false)
  useLayoutEffect(() => {
    const element = box.current
    if (!element) return
    const measure = () => setNarrow(element.clientWidth < PROFILE_TABLE_LEAST)
    measure()
    const resized = new ResizeObserver(measure)
    resized.observe(element)
    return () => resized.disconnect()
  }, [])
  const profile = result.reference_profile
  if (!profile) return null
  const cell = { padding: '4px 6px', verticalAlign: 'top', textAlign: 'left' } as const
  const head = { ...cell, ...TYPE.caption, fontWeight: 400, color: T.muted }
  const label = { ...cell, fontWeight: 400, color: T.muted }
  const table = { borderCollapse: 'collapse', ...TYPE.small, color: T.text, width: '100%', margin: 0, overflowWrap: 'normal' } as const
  const row = { borderTop: `1px solid ${T.hairline}` }
  const title = { ...TYPE.body, fontWeight: 600, color: T.text, overflowWrap: 'anywhere' } as const
  const source = windowLine(t, profile.source_window)
  // Each quantity's words once, for the table and for the stacked rows alike.
  const rows = profile.quantities.map(quantity => {
    const unit = clean(quantity.unit, 30)
    const value = (number: number | null) => (typeof number === 'number' && Number.isFinite(number) ? `${num(number, 2)} ${unit}` : '—')
    const deviation = quantity.deviation
    return {
      key: quantity.quantity,
      name: quantityName(t, quantity.quantity),
      source: value(quantity.source),
      sourceWhy: quantity.source === null ? reasonWords(t, quantity.source_reason, quantity.source_reason_detail, quantity.source_nyquist_hz) : '',
      target: quantity.target ? value(quantity.target.mid) : '—',
      spread: quantity.target ? t('listen.profile.spread', `${num(quantity.target.low, 2)} … ${value(quantity.target.high)}`) : '',
      from: quantity.target && quantity.contributors < quantity.references ? t('listen.profile.from', quantity.contributors, quantity.references) : '',
      targetWhy: quantity.target ? '' : reasonWords(t, quantity.target_reason),
      difference: deviation !== null && Number.isFinite(deviation) ? `${signed(deviation, 2)} ${quantity.unit === 'LUFS' ? 'LU' : unit}` : '—',
      sits: quantity.inside_spread === null || deviation === null ? '' : quantity.inside_spread ? t('listen.profile.inside') : deviation < 0 ? t('listen.profile.below') : t('listen.profile.above')
    }
  })
  return (
    <section aria-label={t('listen.profile.title')} ref={box} style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
      <strong style={title}>{t('listen.profile.title')}</strong>
      <span style={S.muted}>{t('listen.profile.usable', profile.usable_references, profile.references)}</span>
      {narrow ? (
        // Narrow: each quantity on its own, its name and where this file sits on the first line, the values under it.
        <ul style={{ listStyle: 'none', margin: 0, padding: 0, display: 'flex', flexDirection: 'column', ...TYPE.small, color: T.text }}>
          {rows.map(each => (
            <li key={each.key} style={{ ...row, display: 'flex', flexDirection: 'column', gap: 2, padding: '6px 0' }}>
              <span style={{ ...S.row, justifyContent: 'space-between', alignItems: 'baseline', gap: 8 }}>
                <span style={{ color: T.muted }}>{each.name}</span>
                <span style={{ textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>
                  {each.difference}
                  {each.sits && <span style={S.faint}> {each.sits}</span>}
                </span>
              </span>
              <span style={{ fontVariantNumeric: 'tabular-nums' }}>
                {t('listen.this')} {each.source} · {t('listen.profile.target')} {each.target}
              </span>
              {each.sourceWhy && <span style={S.faint}>{each.sourceWhy}</span>}
              {(each.spread || each.from || each.targetWhy) && <span style={S.faint}>{[each.spread, each.from, each.targetWhy].filter(Boolean).join(' · ')}</span>}
            </li>
          ))}
        </ul>
      ) : (
        <div style={{ overflowX: 'auto' }}>
          <table style={table}>
            <thead>
              <tr>
                <th style={head} />
                <th style={head}>{t('listen.this')}</th>
                <th style={head}>{t('listen.profile.target')}</th>
                <th style={head}>{t('listen.difference')}</th>
              </tr>
            </thead>
            <tbody>
              {rows.map(each => (
                <tr key={each.key} style={row}>
                  <th scope="row" style={label}>
                    {each.name}
                  </th>
                  <td style={cell}>
                    {each.source}
                    {each.sourceWhy && <div style={S.faint}>{each.sourceWhy}</div>}
                  </td>
                  <td style={cell}>
                    {each.target}
                    {each.spread && <div style={S.faint}>{each.spread}</div>}
                    {each.from && <div style={S.faint}>{each.from}</div>}
                    {each.targetWhy && <div style={S.faint}>{each.targetWhy}</div>}
                  </td>
                  <td style={cell}>
                    {each.difference}
                    {each.sits && <div style={S.faint}>{each.sits}</div>}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <span style={S.faint}>{t('listen.profile.spreadNote')}</span>
      <Details>
        <span style={S.faint}>{t('listen.profile.how')}</span>
        <span style={S.faint}>{t('listen.profile.noPeak')}</span>
        {source && <span style={S.faint}>{t('listen.facts.thisFile', source)}</span>}
      </Details>
      {(result.references ?? []).map((reference, index) => {
        const name = displayReferenceName(reference)
        const availability = reference.availability
        const canonical = listenView({ metrics: reference.metrics }, t)
        // Why this reference has no profile at all (it's out of the target), and what it couldn't measure, grouped
        // by why: on the face, since that's why a target has fewer references behind it.
        const out = availability?.profile_reason ?? (availability?.energy_zero ? 'zero_energy' : null)
        const unavailable = new Map<string, string[]>()
        const miss = (reason: string, quantity: string) => unavailable.set(reason, [...(unavailable.get(reason) ?? []), quantityName(t, quantity)])
        for (const quantity of reference.profile_quantities ?? []) {
          if (quantity.value === null) miss(reasonWords(t, quantity.reason_code, quantity.reason, quantity.nyquist_hz), quantity.quantity)
        }
        if (!reference.profile_quantities && !out) for (const missing of availability?.unavailable ?? []) miss(reasonWords(t, missing.reason), missing.quantity)
        return (
          <section
            aria-label={t('listen.profile.member', index + 1, name)}
            key={reference.artifact_id ?? index}
            style={{ display: 'flex', flexDirection: 'column', gap: 4, borderTop: `1px solid ${T.hairline}`, paddingTop: 8 }}
          >
            <strong style={title}>
              {index + 1}. {name}
            </strong>
            {/* In a row: Wave's flex basis is a width there; in this column it was a height, a blank under each wave. */}
            {reference.artifact_id && (
              <div style={{ display: 'flex' }}>
                <Wave audition={audition} caption="" id={reference.artifact_id} label={name} />
              </div>
            )}
            {out && <span style={S.muted}>{t('listen.profile.out', reasonWords(t, out))}</span>}
            {unavailable.size > 0 && (
              <ul aria-label={t('listen.profile.missingLabel', name)} style={{ margin: 0, paddingLeft: 16, ...S.faint }}>
                {[...unavailable].map(([reason, quantities]) => (
                  <li key={reason}>{t('listen.profile.missing', quantities.join(', '), reason)}</li>
                ))}
              </ul>
            )}
            <Details>
              {reference.profile_quantities && (
                <div style={{ overflowX: 'auto' }}>
                  <table aria-label={t('listen.profile.memberTable', name)} style={table}>
                    <caption style={{ ...S.faint, textAlign: 'left' }}>{t('listen.profile.memberCaption')}</caption>
                    <thead>
                      <tr>
                        <th style={head} />
                        <th style={head}>{t('listen.profile.measured')}</th>
                        <th style={head}>{t('listen.profile.why')}</th>
                      </tr>
                    </thead>
                    <tbody>
                      {reference.profile_quantities.map(quantity => (
                        <tr key={quantity.quantity} style={row}>
                          <th scope="row" style={label}>
                            {quantityName(t, quantity.quantity)}
                          </th>
                          <td style={cell}>{typeof quantity.value === 'number' && Number.isFinite(quantity.value) ? `${num(quantity.value, 2)} ${clean(quantity.unit, 30)}` : '—'}</td>
                          <td style={{ ...cell, color: T.muted }}>{quantity.value === null ? reasonWords(t, quantity.reason_code, quantity.reason, quantity.nyquist_hz) : ''}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
              <span style={S.faint}>{t('listen.profile.standard')}</span>
              {canonical.rows.length ? <Rows view={canonical} /> : <span style={S.faint}>{t('listen.profile.noStandard')}</span>}
              {referenceFacts(t, reference).map(line => (
                <span key={line} style={S.faint}>
                  {line}
                </span>
              ))}
              {availability && !out && <span style={S.faint}>{t('listen.profile.loud', availability.loud_frame_count)}</span>}
              {availability?.fixed_region_tolerance && <span style={S.faint}>{t('listen.profile.fixedSpread')}</span>}
            </Details>
          </section>
        )
      })}
    </section>
  )
}

export function ListenCard({ media, audition, store }: { media: ToolMediaRendererProps; audition: Audition; store: KumiStore }): ReactNode {
  // Anything this card can't draw keeps Desktop's own player, so the audio always plays.
  if (media.file?.kind !== 'audio') return media.fallback
  return <ListenResultCard audition={audition} fallback={media.fallback} result={media.result} store={store} />
}

/** The card for a `kumi_listen` result, wherever it's drawn: in the transcript's tool part, or in the Measurements tab
 *  when the transcript shows no tool parts (CC-9). `fallback` stands in for what it can't draw. */
export function ListenResultCard({ result, audition, fallback, store }: { result: unknown; audition: Audition; fallback: ReactNode; store: KumiStore }): ReactNode {
  const t = useT()
  const { rounds } = useKumi(store)
  const parsed = parseListen(result)
  if (!parsed) return fallback
  const view = listenView(parsed, t, referenceName(parsed, rounds))
  // Which comparison with this reference this is, among those that arrived while the window was open: a mastering
  // pass's rounds, with how far each was from the reference. Nothing for one the window didn't see arrive.
  const key = roundKey(parsed, rounds)
  const seen = key ? (rounds[key] ?? []) : []
  const index = seen.findIndex(round => round.id === parsed.artifact_id)
  const round = index < 0 ? undefined : roundWords(index + 1, seen.slice(Math.max(0, index - 2), index + 1).map(each => each.gaps), t)
  const facts = parsed.compare ? referenceFacts(t, parsed.compare) : []
  return (
    <div
      data-kumi-card="listen"
      style={{ display: 'flex', flexDirection: 'column', gap: 8, maxWidth: 560, padding: '10px 12px', borderRadius: 6, border: `1px solid ${T.hairline}`, background: T.card, color: T.text }}
    >
      <div style={{ ...S.row, justifyContent: 'space-between', alignItems: 'flex-start', gap: 8 }}>
        <span style={{ ...S.text, ...TYPE.body, fontWeight: 600, minWidth: 0 }}>
          <span style={{ color: T.accent }}>♪</span> {view.title}
          {view.referenceTitle && <span style={{ color: T.muted, fontWeight: 400 }}>{t('listen.versus', view.referenceTitle)}</span>}
        </span>
        <Badge tip={t('listen.measuredTitle')}>{t('listen.measured')}</Badge>
      </div>
      {view.capture && (
        <Tip label={t('listen.captureTitle')}>
          <span style={S.faint}>{view.capture}</span>
        </Tip>
      )}
      {/* The runtime's own note on the capture (CC-2 A3): muted plain text, no tone, never a control. */}
      {view.captureNote && <span style={{ ...S.muted, whiteSpace: 'normal' }}>{view.captureNote}</span>}
      {view.verdict && <span style={S.text}>{view.verdict}</span>}
      {round && (
        <Tip label={t('listen.roundTitle')}>
          <span style={{ ...S.muted, fontVariantNumeric: 'tabular-nums' }}>{round}</span>
        </Tip>
      )}
      {view.mineId ? (
        <div style={{ ...S.row, gap: 12, flexWrap: 'wrap', alignItems: 'flex-start' }}>
          {/* Its caption tells two waves apart; alone, the card's title already names it. */}
          <Wave audition={audition} caption={view.referenceId ? view.title : ''} id={view.mineId} label={view.title} />
          {view.referenceId && <Wave audition={audition} id={view.referenceId} label={view.referenceTitle ?? t('listen.reference')} />}
        </div>
      ) : (
        fallback
      )}
      {parsed.reference_profile && <span style={S.faint}>{t('listen.profile.standard')}</span>}
      <Rows view={view} />
      <Bands view={view} />
      <ReferenceProfile result={parsed} audition={audition} />
      {view.headlines.length > 0 && (
        <ul style={{ margin: 0, paddingLeft: 16, display: 'flex', flexDirection: 'column', gap: 1 }}>
          {view.headlines.map((line, index) => (
            <li key={index} style={S.muted}>
              {line}
            </li>
          ))}
        </ul>
      )}
      <Interpretation view={view} />
      {facts.length > 0 && (
        <Details>
          {facts.map(line => (
            <span key={line} style={S.faint}>
              {line}
            </span>
          ))}
        </Details>
      )}
    </div>
  )
}
