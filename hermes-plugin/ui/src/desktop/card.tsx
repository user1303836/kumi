// The inline card Desktop draws for the audio a `kumi_listen` call measured (its `MEDIA:` line): both waveforms when
// it compared two files, the measured numbers side by side, and the differences Kumi computed. Every number is a
// local measurement; a model's reading of them shows only when the user turned it on, and says so.
import type { ToolMediaRendererProps } from '@hermes/plugin-sdk'
import { T } from '@kumi/theme'
import type { ReactNode } from 'react'
import { useEffect, useState } from 'react'

import { clean, num, signed } from '../shared/format.ts'
import { type Audition, useKumi, usePlaying } from '../shared/hooks.ts'
import { displayReferenceName, listenView, parseListen, referenceName, roundKey, roundWords, windowWords, type ListenView } from '../shared/listen.ts'
import type { KumiStore } from '../shared/store.ts'
import type { ListenReference, ListenResult } from '../shared/types.ts'
import { Chip, FOCUSABLE, S, TextButton, useT } from '../shared/ui.tsx'

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
        <TextButton
          label={playing ? t('listen.stop', label) : t('listen.play', label)}
          onClick={() => (playing ? audition.stop() : void audition.play(id).catch(() => setFailed(true)))}
          pressed={playing}
        >
          {playing ? '■' : '▶'}
        </TextButton>
      </div>
      <svg aria-label={t('listen.waveform', label)} height={36} preserveAspectRatio="none" role="img" viewBox="0 0 96 36" width="100%">
        {(peaks ?? []).map((peak, index) => (
          <rect fill={playing ? T.accent : T.secondary} height={Math.max(1, peak * 34)} key={index} opacity={0.85} width={0.7} x={index + 0.15} y={18 - Math.max(0.5, peak * 17)} />
        ))}
        {!peaks && <line stroke={T.border} x1={0} x2={96} y1={18} y2={18} />}
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
      <table style={{ borderCollapse: 'collapse', fontSize: 12, lineHeight: 1.45, color: T.text, width: '100%', margin: 0 }}>
        {compared && (
          <thead style={{ border: 'none' }}>
            <tr style={{ color: T.muted, fontSize: 11, border: 'none' }}>
              <th style={{ ...cell, textAlign: 'left', fontWeight: 400 }} />
              <th style={{ ...cell, fontWeight: 400 }}>{t('listen.this')}</th>
              <th style={{ ...cell, fontWeight: 400 }}>{t('listen.reference')}</th>
              <th style={{ ...cell, fontWeight: 400 }}>{t('listen.difference')}</th>
            </tr>
          </thead>
        )}
        <tbody>
          {view.rows.map(row => (
            <tr key={row.label} style={{ border: 'none', borderTop: `1px solid ${T.border}` }}>
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
          const label = t('listen.overlayBand', band.name, band.hz, num(band.mine), num(band.reference), signed(band.mine - band.reference))
          return (
            <div key={band.name} role="img" aria-label={label} title={label} style={{ flex: 1, display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 2, minWidth: 0 }}>
              <div style={{ position: 'relative', flex: 1, width: '100%' }}>
                <div style={{ position: 'absolute', left: '18%', right: '18%', bottom: 0, height: at(band.mine), background: T.accent, opacity: 0.8 }} />
                <div style={{ position: 'absolute', left: 0, right: 0, bottom: at(band.reference), borderTop: `2px solid ${T.text}` }} />
              </div>
              <span style={{ fontSize: 9, color: T.faint, ...S.ellipsis, maxWidth: '100%' }}>{band.name}</span>
            </div>
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
          const label = view.bandsAre === 'difference' ? `${band.name}: ${signed(band.value)} dB` : `${band.name}: ${num(band.value)} dB`
          return (
            <div key={band.name} style={{ flex: 1, display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 2, minWidth: 0 }} title={label}>
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
                {view.bandsAre === 'difference' && <div style={{ position: 'absolute', left: 0, right: 0, top: '50%', borderTop: `1px solid ${T.border}` }} />}
              </div>
              <span style={{ fontSize: 9, color: T.faint, ...S.ellipsis, maxWidth: '100%' }}>{band.name}</span>
            </div>
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
          <TextButton onClick={() => setOpen(!open)}>{open ? t('listen.less') : t('listen.more')}</TextButton>
        </span>
      )}
    </div>
  )
}

function ReferenceFacts({ reference }: { reference: ListenReference }) {
  const identity = reference.origin?.identity ?? reference.identity
  const file = identity?.file_identity
  // Render only the native public identity fields, never an arbitrary nested value or source path.
  const facts = file && typeof file === 'object' ? Object.entries(file).filter(([key, value]) =>
    ['dev', 'ino', 'size', 'mtime_ns'].includes(key) && (typeof value === 'number' || typeof value === 'string')
  ).map(([key, value]) => `${key}=${clean(String(value), 60)}`).join(' · ') : ''
  const original = reference.origin?.window
  const measured = windowWords(reference.window)
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 3, overflowWrap: 'anywhere' }}>
      {identity && <span style={S.faint}>Identity: {clean(identity.kind ?? '', 20)} {facts}
        {identity.sound_id && ` · sound ${clean(identity.sound_id.split(/[\\/]/).pop() ?? '', 80)}`}
        {identity.map_revision && ` · map ${clean(identity.map_revision, 80)}`}
        {identity.artifact_id && ` · artifact ${clean(identity.artifact_id, 80)}`}
      </span>}
      {original && <span style={S.faint}>Original provenance · {windowWords(original)}</span>}
      {reference.artifact_id && <span style={S.faint}>Retained artifact: {clean(reference.artifact_id, 100)}</span>}
      {measured && <span style={S.faint}>{original ? 'Artifact measured window · ' : ''}{measured}</span>}
      {reference.window && <span style={S.faint}>Retained artifact playback starts at {num(reference.window.replay_from, 6)} s</span>}
    </div>
  )
}

function ReferenceProfile({ result, audition }: { result: ListenResult; audition: Audition }) {
  const t = useT()
  const profile = result.reference_profile
  if (!profile) return null
  const cell = { padding: '5px 6px', verticalAlign: 'top', textAlign: 'left' } as const
  return (
    <section aria-label="Measured reference profile" style={{ display: 'flex', flexDirection: 'column', gap: 9 }}>
      <strong style={S.text}>Reference target and spread</strong>
      <span style={S.muted}>{profile.usable_references}/{profile.references} usable references · equal per-reference weighting</span>
      <span style={S.faint}>{clean(profile.basis, 500)}</span>
      {profile.source_window && <span style={S.faint}>Source measured window · {windowWords(profile.source_window)?.replace('Reference file seconds', 'seconds')}</span>}
      <div style={{ overflowX: 'auto' }}>
        <table style={{ borderCollapse: 'collapse', fontSize: 12, lineHeight: 1.45, width: '100%', margin: 0 }}>
          <thead><tr style={{ color: T.muted }}>
            {['Quantity / basis', 'Measured source', 'Target [spread]', 'Source − target', 'Contributors / total'].map(label => <th key={label} style={{ ...cell, fontWeight: 400 }}>{label}</th>)}
          </tr></thead>
          <tbody>{profile.quantities.map(quantity => {
            const value = (number: number | null) => typeof number === 'number' && Number.isFinite(number) ? `${num(number, 2)} ${clean(quantity.unit, 30)}` : '—'
            return <tr key={quantity.quantity} style={{ borderTop: `1px solid ${T.border}` }}>
              <th scope="row" style={{ ...cell, fontWeight: 400 }}>
                {clean(quantity.quantity, 80)}<div style={S.faint}>{clean(quantity.basis, 300)}</div>
              </th>
              <td style={cell}>{value(quantity.source)}{quantity.source === null && <div style={S.faint}>{clean(quantity.source_reason_detail ?? quantity.source_reason ?? 'not measured', 300)}</div>}</td>
              <td style={cell}>{quantity.target ? <>{value(quantity.target.mid)}<div style={S.faint}>[{value(quantity.target.low)} … {value(quantity.target.high)}]</div></> : <>—<div style={S.faint}>{clean(quantity.target_reason ?? 'not measured', 100)}</div></>}</td>
              <td style={cell}>{quantity.deviation !== null && Number.isFinite(quantity.deviation) ? `${signed(quantity.deviation, 2)} ${quantity.unit === 'LUFS' ? 'LU' : clean(quantity.unit, 30)}` : '—'}
                {quantity.inside_spread !== null && <div style={S.faint}>{quantity.inside_spread ? 'Inside spread' : 'Outside spread'}</div>}
              </td>
              <td style={cell}>{quantity.contributors}/{quantity.references}</td>
            </tr>
          })}</tbody>
        </table>
      </div>
      <span style={S.faint}>{clean(profile.range_note, 500)}</span>
      <span style={S.faint}>{clean(profile.peak_note, 500)}</span>
      {(result.references ?? []).map((reference, index) => {
        const name = displayReferenceName(reference)
        const availability = reference.availability
        const canonical = listenView({ metrics: reference.metrics }, t)
        const unavailable = new Map<string, string[]>()
        for (const quantity of reference.profile_quantities ?? []) {
          if (quantity.value !== null) continue
          const reason = clean(quantity.reason ?? quantity.reason_code ?? 'not measured', 300)
          const quantities = unavailable.get(reason) ?? []
          quantities.push(clean(quantity.quantity, 80))
          unavailable.set(reason, quantities)
        }
        return <section key={reference.artifact_id ?? index} aria-label={`Reference ${index + 1}: ${name}`} style={{ display: 'flex', flexDirection: 'column', gap: 5, borderTop: `1px solid ${T.border}`, paddingTop: 8 }}>
          <strong style={S.text}>{index + 1}. {name}</strong>
          <ReferenceFacts reference={reference} />
          {reference.artifact_id && <Wave audition={audition} id={reference.artifact_id} label={name} />}
          {unavailable.size > 0 && <ul aria-label={`${name}: unavailable profile quantities`} style={{ margin: 0, paddingLeft: 16, ...S.faint }}>
            {[...unavailable].map(([reason, quantities]) => <li key={reason}>{quantities.join(', ')}: — · {reason}</li>)}
          </ul>}
          {reference.profile_quantities && <details>
            <summary className={FOCUSABLE} style={{ ...S.muted, cursor: 'pointer' }}>{t('listen.more')} · Each reference's measurements</summary>
            <div style={{ overflowX: 'auto', marginTop: 5 }}>
            <table aria-label={`${name}: reference measurements`} style={{ borderCollapse: 'collapse', fontSize: 12, lineHeight: 1.45, width: '100%', margin: 0 }}>
              <caption style={{ ...S.faint, textAlign: 'left' }}>Each reference's measurements · only covered regions contribute to the target</caption>
              <thead><tr style={{ color: T.muted }}>
                {['Quantity / basis', 'Measured reference', 'Unavailable reason'].map(label => <th key={label} style={{ ...cell, fontWeight: 400 }}>{label}</th>)}
              </tr></thead>
              <tbody>{reference.profile_quantities.map(quantity => <tr key={quantity.quantity} style={{ borderTop: `1px solid ${T.border}` }}>
                <th scope="row" style={{ ...cell, fontWeight: 400 }}>{clean(quantity.quantity, 80)}<div style={S.faint}>{clean(quantity.basis, 300)}</div></th>
                <td style={cell}>{typeof quantity.value === 'number' && Number.isFinite(quantity.value) ? `${num(quantity.value, 2)} ${clean(quantity.unit, 30)}` : '—'}</td>
                <td style={{ ...cell, color: T.muted }}>{quantity.value === null ? clean(quantity.reason ?? 'not measured', 300) : '—'}</td>
              </tr>)}</tbody>
            </table>
            </div>
          </details>}
          <span style={S.faint}>Standard measurements · a different meter from the reference target</span>
          {canonical.rows.length ? <Rows view={canonical} /> : <span style={S.faint}>Standard measurements unavailable</span>}
          {availability && <span style={S.faint}>
            Profile availability: {availability.profile_reason ? clean(availability.profile_reason, 100) : `${availability.loud_frame_count} loud frames`}
            {availability.energy_zero && ' · zero energy'}
            {availability.fixed_region_tolerance && ' · fixed region tolerance, not observed variation'}
            {!reference.profile_quantities && availability.unavailable.map(missing => ` · ${clean(missing.quantity, 60)}: ${clean(missing.reason, 100)}`).join('')}
          </span>}
        </section>
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
  return (
    <div
      data-kumi-card="listen"
      style={{ display: 'flex', flexDirection: 'column', gap: 9, maxWidth: 560, padding: '10px 12px', borderRadius: 6, border: `1px solid ${T.border}`, background: T.card, color: T.text }}
    >
      <div style={{ ...S.row, justifyContent: 'space-between', alignItems: 'flex-start', gap: 8 }}>
        <span style={{ ...S.text, fontWeight: 600, minWidth: 0 }}>
          <span style={{ color: T.accent }}>♪</span> {view.title}
          {view.referenceTitle && <span style={{ color: T.muted, fontWeight: 400 }}>{t('listen.versus', view.referenceTitle)}</span>}
        </span>
        <Chip title={t('listen.measuredTitle')}>{t('listen.measured')}</Chip>
      </div>
      {view.capture && (
        <span style={S.faint} title={t('listen.captureTitle')}>
          {view.capture}
        </span>
      )}
      {/* The runtime's own note on the capture (CC-2 A3): muted plain text, no tone, never a control. */}
      {view.captureNote && <span style={{ ...S.muted, whiteSpace: 'normal' }}>{view.captureNote}</span>}
      {view.verdict && <span style={S.text}>{view.verdict}</span>}
      {round && (
        <span style={{ ...S.muted, fontVariantNumeric: 'tabular-nums' }} title={t('listen.roundTitle')}>
          {round}
        </span>
      )}
      {view.mineId ? (
        <div style={{ ...S.row, gap: 12, flexWrap: 'wrap', alignItems: 'flex-start' }}>
          <Wave audition={audition} id={view.mineId} label={view.title} />
          {view.referenceId && <Wave audition={audition} id={view.referenceId} label={view.referenceTitle ?? t('listen.reference')} />}
        </div>
      ) : (
        fallback
      )}
      {parsed.compare && (parsed.compare.window || parsed.compare.origin || parsed.compare.identity) && <ReferenceFacts reference={parsed.compare} />}
      {parsed.reference_profile && <span style={S.faint}>Source standard measurements · a different meter from the reference target</span>}
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
    </div>
  )
}
