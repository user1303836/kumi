// What a card of the Make panel lets the producer set, under its tabs: each mode's own choices (Resonate's pairing,
// ring and decay; Modulate's carrier, stages and the kinds allowed; Attack's first sound; lengths and depths), the
// loop's tab (its style, tempo and bars, then rhythm, voices, feel, harmony and line), and the steer every card takes.
// Every control rests until it's moved: an option the seed draws ("Auto", which Reroll varies), a steer control at its
// neutral (it changes nothing).
import { T } from '@kumi/theme'
import { useState } from 'react'

import { Choice, Group, Pills, Slider, type SliderValue, Toggles } from './controls.tsx'
import {
  CARRIERS,
  CONTOURS,
  type DesignCard,
  type DesignOptions,
  type DesignSource,
  GRIDS,
  KEYS,
  LOOP_GROUPS,
  LOOP_SPANS,
  type LoopControl,
  type LoopNumber,
  type LoopOptions,
  loopStyleOf,
  MOD_KINDS,
  MODULATE_SPANS,
  PAIRINGS,
  RANGES,
  RESONATE_SPANS,
  RINGS,
  rollSteer,
  SCALES,
  type Span,
  STEER_LAYER,
  STEER_RANGES,
  STEER_SOUND,
  type Steer,
  steerCount,
  steerOf,
  type SteerLayer,
  type SteerRange,
  type SteerSound,
  TUNINGS
} from './design.ts'
import { clean, num, seconds } from './format.ts'
import { S, TextButton, useT } from './ui.tsx'

/** An option set or not: what a control hands back. */
type Patch = (current: DesignOptions) => DesignOptions

const percent = (value: number): number => Math.round(value * 100)
const signed = (value: number): string => (value > 0 ? `+${value}` : value < 0 ? `−${Math.abs(value)}` : '0')

/** The common meters, as the loop's meter pills offer them (the engine takes any from 1/2 to 16/16). */
const METERS: [number, number][] = [
  [4, 4],
  [3, 4],
  [5, 4],
  [7, 4],
  [6, 8],
  [7, 8],
  [9, 8],
  [12, 8],
  [5, 8],
  [11, 8]
]

/** The options a card shows under its tabs, besides the steer: its own sliders and choices. */
export function CardOptions({ card, options, sources, onChange }: { card: DesignCard; options: DesignOptions; sources: DesignSource[]; onChange: (patch: Patch) => void }) {
  const t = useT()
  const auto = t('make.auto')
  const set = <K extends keyof DesignOptions>(key: K, value: DesignOptions[K] | undefined) => onChange(current => ({ ...current, [key]: value }))
  const sliders = card.options.flatMap(name => {
    if (name === 'attack_from' || name === 'bpm' || name === 'bars') return []
    const labels: Record<string, (value: number) => string> = {
      seconds: value => seconds(value),
      depth: value => `${percent(value)}%`,
      stretch: value => `${value}×`
    }
    return [
      <Slider
        key={name}
        label={t(`make.options.${name}`)}
        onChange={value => set(name, value as number | undefined)}
        rest={auto}
        span={RANGES[name]}
        value={options[name]}
        format={value => labels[name](value as number)}
        title={t('make.autoTitle')}
      />
    ]
  })
  const resonate = options.resonate ?? {}
  const modulate = options.modulate ?? {}
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
      {card.options.includes('attack_from') && (
        <Choice
          label={t('make.options.attack_from')}
          onChange={value => set('attack_from', value === undefined ? undefined : Number(value))}
          options={sources.slice(0, 64).map((source, index) => ({ value: String(index), label: clean(source.name ?? source.sound_id ?? source.artifact_id ?? '', 40) }))}
          rest={auto}
          value={options.attack_from === undefined ? undefined : String(options.attack_from)}
        />
      )}
      {card.key === 'resonate' && (
        <>
          <Pills
            label={t('make.resonate.pairing')}
            onChange={pairing => set('resonate', { ...resonate, pairing })}
            options={PAIRINGS.map(pairing => ({ value: pairing, label: t(`make.resonate.pairings.${pairing}`), title: t(`make.resonate.pairingHints.${pairing}`) }))}
            rest={auto}
            value={resonate.pairing}
          />
          <Pills label={t('make.resonate.ring')} onChange={ring => set('resonate', { ...resonate, ring })} options={RINGS.map(ring => ({ value: ring, label: t(`make.resonate.rings.${ring}`) }))} rest={auto} value={resonate.ring} />
        </>
      )}
      {card.key === 'modulate' && (
        <>
          <Pills
            label={t('make.modulate.carrier')}
            onChange={carrier => set('modulate', { ...modulate, carrier })}
            options={CARRIERS.filter(carrier => carrier !== 'random').map(carrier => ({ value: carrier, label: t(`make.modulate.carriers.${carrier}`) }))}
            rest={t('make.modulate.carriers.random')}
            value={modulate.carrier === 'random' ? undefined : modulate.carrier}
          />
          <Toggles
            label={t('make.modulate.kinds')}
            onChange={kinds => set('modulate', { ...modulate, kinds })}
            options={MOD_KINDS.map(kind => ({ value: kind, label: t(`make.modulate.kindNames.${kind}`), title: t(`make.modulate.kindHints.${kind}`) }))}
            rest={t('make.modulate.allKinds')}
            value={modulate.kinds}
          />
        </>
      )}
      {(sliders.length > 0 || card.key === 'resonate' || card.key === 'modulate') && (
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(150px, 1fr))', gap: '10px 16px', alignItems: 'start' }}>
          {sliders}
          {card.key === 'resonate' && (
            <Slider
              ends={[t('make.resonate.short'), t('make.resonate.long')]}
              format={value => `${percent(value as number)}%`}
              label={t('make.resonate.decay')}
              onChange={decay => set('resonate', { ...resonate, decay: decay as number | undefined })}
              rest={auto}
              span={RESONATE_SPANS.decay}
              value={resonate.decay}
            />
          )}
          {card.key === 'modulate' && (
            <>
              <Slider
                format={value => String(value)}
                label={t('make.modulate.stages')}
                onChange={stages => set('modulate', { ...modulate, stages: stages as number | undefined })}
                rest={auto}
                span={MODULATE_SPANS.stages}
                title={t('make.modulate.stagesTitle')}
                value={modulate.stages}
              />
              <Slider
                format={value => `${percent(value as number)}%`}
                label={t('make.modulate.retrigger')}
                onChange={retrigger => set('modulate', { ...modulate, retrigger: retrigger as number | undefined })}
                rest={auto}
                span={MODULATE_SPANS.retrigger}
                title={t('make.modulate.retriggerTitle')}
                value={modulate.retrigger}
              />
            </>
          )}
        </div>
      )}
    </div>
  )
}

/** The loop's tab: Beat or Melody, the tempo (the DAW's, until it's moved) and bars, then its groups, each folding, the
 *  first open. A group of the other style hides. */
export function LoopSection({ options, tempo, onChange }: { options: DesignOptions; tempo: number; onChange: (patch: Patch) => void }) {
  const t = useT()
  const auto = t('make.auto')
  const loop: LoopOptions = options.loop ?? {}
  const style = loop.style === 'melody' ? 'melody' : 'beat'
  const [open, setOpen] = useState<Record<string, boolean>>({ rhythm: true })
  const setLoop = (patch: Partial<LoopOptions>) => onChange(current => ({ ...current, loop: { ...(current.loop ?? {}), ...patch } }))
  const control = (name: LoopControl) => {
    const label = t(`make.loop.controls.${name}`)
    if (name === 'meter') {
      const chosen = loop.meter ? `${loop.meter[0]}/${loop.meter[1]}` : undefined
      return (
        <Pills
          key={name}
          label={label}
          onChange={value => setLoop({ meter: value === undefined ? undefined : (value.split('/').map(Number) as [number, number]) })}
          options={METERS.map(([beats, unit]) => ({ value: `${beats}/${unit}`, label: `${beats}/${unit}` }))}
          rest={auto}
          value={chosen}
        />
      )
    }
    if (name === 'grid') return <Pills key={name} label={label} onChange={grid => setLoop({ grid })} options={GRIDS.map(grid => ({ value: grid, label: t(`make.loop.grids.${grid}`) }))} rest={auto} value={loop.grid} />
    if (name === 'key') return <Pills key={name} label={label} onChange={key => setLoop({ key })} options={KEYS.map(key => ({ value: key, label: key }))} rest={auto} value={loop.key} />
    if (name === 'scale') return <Choice key={name} label={label} onChange={scale => setLoop({ scale })} options={SCALES.map(scale => ({ value: scale, label: t(`make.loop.scales.${scale}`) }))} rest={auto} value={loop.scale} />
    if (name === 'tuning') return <Choice key={name} label={label} onChange={tuning => setLoop({ tuning })} options={TUNINGS.map(tuning => ({ value: tuning, label: t(`make.loop.tunings.${tuning}`) }))} rest={auto} value={loop.tuning} />
    if (name === 'contour') return <Pills key={name} label={label} onChange={contour => setLoop({ contour })} options={CONTOURS.map(contour => ({ value: contour, label: t(`make.loop.contours.${contour}`) }))} rest={auto} value={loop.contour} />
    const span: Span = LOOP_SPANS[name as LoopNumber]
    const format = (value: number): string =>
      name === 'register' ? t('make.loop.registerValue', signed(value)) : name === 'octaves' ? t('make.loop.octavesValue', value) : name === 'lean' ? `${value < 0 ? t('make.loop.early') : value > 0 ? t('make.loop.late') : ''} ${percent(Math.abs(value))}%`.trim() : `${percent(value)}%`
    return (
      <Slider
        ends={name === 'lean' ? [t('make.loop.early'), t('make.loop.late')] : undefined}
        format={value => format(value as number)}
        key={name}
        label={label}
        onChange={value => setLoop({ [name]: value as number | undefined })}
        rest={auto}
        span={span}
        title={t(`make.loop.hints.${name}`)}
        value={loop[name as LoopNumber]}
      />
    )
  }
  const counted = (controls: LoopControl[]): number => controls.filter(name => loop[name as keyof LoopOptions] !== undefined).length
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(150px, 1fr))', gap: '10px 16px', alignItems: 'end' }}>
        <Pills
          label={t('make.loop.style')}
          onChange={value => setLoop({ style: value ?? 'beat' })}
          options={[
            { value: 'beat', label: t('make.loop.styles.beat'), title: t('make.loop.styleHints.beat') },
            { value: 'melody', label: t('make.loop.styles.melody'), title: t('make.loop.styleHints.melody') }
          ]}
          rest={null}
          value={style}
        />
        <Slider
          format={value => t('make.optionValues.bpm', num(value as number, 0))}
          label={t('make.options.bpm')}
          onChange={value => onChange(current => ({ ...current, bpm: value as number | undefined }))}
          rest={t('make.loop.tempoFrom', num(tempo, 0))}
          restAt={tempo}
          span={RANGES.bpm}
          title={t('make.loop.tempoTitle')}
          value={options.bpm}
        />
        <Slider
          format={value => t('make.optionValues.bars', value as number)}
          label={t('make.options.bars')}
          onChange={value => onChange(current => ({ ...current, bars: value as number | undefined }))}
          rest={auto}
          span={RANGES.bars}
          value={options.bars}
        />
      </div>
      {LOOP_GROUPS.filter(group => !group.style || group.style === style).map(group => {
        const controls = group.controls.filter(name => {
          const owner = loopStyleOf(name)
          return !owner || owner === style
        })
        const count = counted(controls)
        return (
          <Group
            actions={
              count > 0 ? (
                <TextButton onClick={() => setLoop(Object.fromEntries(controls.map(name => [name, undefined])))} title={t('make.loop.resetTitle')}>
                  {t('make.loop.reset')}
                </TextButton>
              ) : undefined
            }
            count={count > 0 ? t('make.steer.count', count) : undefined}
            hint={t(`make.loop.groupHints.${group.key}`)}
            key={group.key}
            onToggle={() => setOpen(current => ({ ...current, [group.key]: !current[group.key] }))}
            open={open[group.key] === true}
            title={t(`make.loop.groups.${group.key}`)}
          >
            {controls.map(control)}
          </Group>
        )
      })}
    </div>
  )
}

/** Steer, on every card: the sound's own controls, then each layer's (each source draws within its range from the
 *  seed). Kept per card by the panel; Reset puts them all back to neutral, Surprise moves a few a good way. */
export function SteerSection({ steer, onChange, open, onToggle }: { steer: Steer; onChange: (steer: Steer) => void; open: boolean; onToggle: () => void }) {
  const t = useT()
  const count = steerCount(steer)
  const bipolar = (key: SteerSound) => STEER_SOUND[key].min < 0
  const valueWords = (key: SteerSound | SteerLayer, value: SliderValue): string => {
    if (Array.isArray(value)) {
      if (key === 'pitch') return t('make.steer.values.semitones', signed(value[0]), signed(value[1]))
      if (key === 'delay') return t('make.steer.values.ms', value[0], value[1])
      return t('make.steer.values.percentRange', percent(value[0]), percent(value[1]))
    }
    if (key === 'level') return t('make.steer.values.down', Math.round(value * 18))
    if (key === 'pan') return t('make.steer.values.pan', percent(value))
    if (key in STEER_SOUND && bipolar(key as SteerSound)) {
      const end = value < 0 ? t(`make.steer.ends.${key}.low`) : t(`make.steer.ends.${key}.high`)
      return `${end} ${percent(Math.abs(value))}%`
    }
    return `${percent(value)}%`
  }
  const slider = (key: SteerSound | SteerLayer) => {
    const span: Span = key in STEER_SOUND ? STEER_SOUND[key as SteerSound] : STEER_LAYER[key as SteerLayer]
    const range = (STEER_RANGES as readonly string[]).includes(key)
    return (
      <Slider
        ends={key in STEER_SOUND && bipolar(key as SteerSound) ? [t(`make.steer.ends.${key}.low`), t(`make.steer.ends.${key}.high`)] : undefined}
        format={value => valueWords(key, value)}
        key={key}
        label={t(`make.steer.controls.${key}`)}
        onChange={value => {
          const next = { ...steer } as Record<string, SliderValue | undefined>
          next[key] = value
          onChange(next as Steer)
        }}
        range={range}
        rest=""
        span={span}
        title={t(`make.steer.hints.${key}`)}
        value={range ? steer[key as SteerRange] : steer[key as SteerSound]}
      />
    )
  }
  // What's steered, in words, for the folded section's line: "Space 45% · Pitch −5…+3 st".
  const steered = steerOf(steer) ?? {}
  const summary = (Object.keys(steered) as (SteerSound | SteerLayer)[]).map(key => `${t(`make.steer.controls.${key}`)} ${valueWords(key, steered[key as SteerSound] as SliderValue)}`).join(' · ')
  return (
    <Group
      actions={
        <span style={{ ...S.row, gap: 4 }}>
          <TextButton onClick={() => onChange(rollSteer())} title={t('make.steer.rollTitle')}>
            <svg aria-hidden height="10" style={{ marginRight: 4, verticalAlign: '-1px' }} viewBox="0 0 12 12" width="10">
              <rect fill="none" height="10" rx="2.5" stroke="currentColor" strokeWidth="1.2" width="10" x="1" y="1" />
              <circle cx="4" cy="4" fill="currentColor" r="1" />
              <circle cx="6" cy="6" fill="currentColor" r="1" />
              <circle cx="8" cy="8" fill="currentColor" r="1" />
            </svg>
            {t('make.steer.roll')}
          </TextButton>
          {count > 0 && (
            <TextButton onClick={() => onChange({})} title={t('make.steer.resetTitle')}>
              {t('make.steer.reset')}
            </TextButton>
          )}
        </span>
      }
      count={count > 0 ? t('make.steer.count', count) : undefined}
      hint={t('make.steer.hint')}
      onToggle={onToggle}
      open={open}
      summary={summary}
      title={t('make.steer.title')}
    >
      <span style={{ gridColumn: '1 / -1', ...S.heading, fontSize: 10.5 }}>{t('make.steer.sound')}</span>
      {(Object.keys(STEER_SOUND) as SteerSound[]).map(slider)}
      <span style={{ gridColumn: '1 / -1', ...S.heading, fontSize: 10.5, marginTop: 4 }} title={t('make.steer.layersHint')}>
        {t('make.steer.layers')}
      </span>
      {(Object.keys(STEER_LAYER) as SteerLayer[]).map(slider)}
      <span style={{ gridColumn: '1 / -1', fontSize: 10.5, color: T.faint }}>{t('make.steer.layersHint')}</span>
    </Group>
  )
}
