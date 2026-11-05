// The inline card Desktop draws for the audio a `kumi_listen` call measured (its `MEDIA:` line): both waveforms when
// it compared two files, the measured numbers side by side, and the differences Kumi computed. Every number is a
// local measurement; a model's reading of them shows only when the user turned it on, and says so.
import type { ToolMediaRendererProps } from '@hermes/plugin-sdk'
import { T } from '@kumi/theme'
import type { ReactNode } from 'react'
import { useEffect, useState } from 'react'

import { num, signed } from '../shared/format.ts'
import { type Audition, usePlaying } from '../shared/hooks.ts'
import { listenView, parseListen, type ListenView } from '../shared/listen.ts'
import { Chip, S, TextButton, useT } from '../shared/ui.tsx'

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

function Wave({ audition, id, label }: { audition: Audition; id: string; label: string }) {
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
        <span style={{ ...S.faint, ...S.ellipsis }}>{label}</span>
        <TextButton onClick={() => (playing ? audition.stop() : void audition.play(id).catch(() => setFailed(true)))}>{playing ? '■' : '▶'}</TextButton>
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
    <table style={{ borderCollapse: 'collapse', fontSize: 12, color: T.text, width: '100%' }}>
      {compared && (
        <thead>
          <tr style={{ color: T.muted, fontSize: 11 }}>
            <th style={{ ...cell, textAlign: 'left', fontWeight: 400 }} />
            <th style={{ ...cell, fontWeight: 400 }}>{t('listen.this')}</th>
            <th style={{ ...cell, fontWeight: 400 }}>{t('listen.reference')}</th>
            <th style={{ ...cell, fontWeight: 400 }}>{t('listen.difference')}</th>
          </tr>
        </thead>
      )}
      <tbody>
        {view.rows.map(row => (
          <tr key={row.label} style={{ borderTop: `1px solid ${T.border}` }}>
            <td style={{ ...cell, textAlign: 'left', color: T.muted }}>{row.label}</td>
            <td style={cell}>{row.mine}</td>
            {compared && <td style={{ ...cell, color: T.muted }}>{row.reference ?? '—'}</td>}
            {compared && <td style={{ ...cell, color: row.direction === 0 ? T.muted : T.text, fontWeight: 600 }}>{row.delta ?? '—'}</td>}
          </tr>
        ))}
      </tbody>
    </table>
  )
}

function Bands({ view }: { view: ListenView }) {
  const t = useT()
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

export function ListenCard({ media, audition }: { media: ToolMediaRendererProps; audition: Audition }): ReactNode {
  // Anything this card can't draw keeps Desktop's own player, so the audio always plays.
  if (media.file?.kind !== 'audio') return media.fallback
  return <ListenResultCard audition={audition} fallback={media.fallback} result={media.result} />
}

/** The card for a `kumi_listen` result, wherever it's drawn: in the transcript's tool part, or in the Measurements tab
 *  when the transcript shows no tool parts (CC-9). `fallback` stands in for what it can't draw. */
export function ListenResultCard({ result, audition, fallback }: { result: unknown; audition: Audition; fallback: ReactNode }): ReactNode {
  const t = useT()
  const parsed = parseListen(result)
  if (!parsed) return fallback
  const view = listenView(parsed, t)
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
      {view.mineId ? (
        <div style={{ ...S.row, gap: 12, flexWrap: 'wrap', alignItems: 'flex-start' }}>
          <Wave audition={audition} id={view.mineId} label={view.title} />
          {view.referenceId && <Wave audition={audition} id={view.referenceId} label={view.referenceTitle ?? t('listen.reference')} />}
        </div>
      ) : (
        fallback
      )}
      <Rows view={view} />
      <Bands view={view} />
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
