// Synthetic DTOs for UI behavior only, not evidence of native measurement.
export const window = { original_total_frames: 4800000, original_seconds: 100, original_duration_exact: true, requested: { from: 10, seconds: 2 }, start_frame: 480000, frame_count: 96000, sample_rate: 48000, channels: 2, replay_from: 0 }
export const reference = (id = 'one') => ({
  artifact_id: `ref-${id}`, name: `${id}.wav`,
  identity: { kind: 'file', file_identity: { dev: 1, ino: id, size: 123, mtime_ns: '1' } }, window,
  metrics: { file: '/private/never-display.wav', seconds: 100, loudness: { integratedLufs: -12, truePeakDbtp: -0.5 } },
  availability: { energy_zero: false, profile_reason: null, loud_frame_count: 4, fixed_region_tolerance: true, unavailable: [{ quantity: 'range', reason: 'too_short_for_range' }] }
})
export const replay = ref => ({ ...ref, identity: { kind: 'artifact', artifact_id: ref.artifact_id }, origin: { name: ref.name, identity: ref.identity, window: ref.window }, window: { ...ref.window, requested: null, original_total_frames: 96000, original_seconds: 2, start_frame: 0 } })
export const profileResult = () => ({
  artifact_id: 'source', metrics: { file: 'Mix.wav', loudness: { integratedLufs: -14 } },
  capture: { from_beat: 16, beats: 8, seconds: 2, method: 'ears', cleanup: 'restored' },
  references: [reference(), reference('two'), { ...reference('silent'), availability: { energy_zero: true, profile_reason: 'zero_energy', loud_frame_count: 0, fixed_region_tolerance: false, unavailable: [] } }],
  reference_profile: {
    basis: 'Real Heard frames / Profile::of / equal-per-reference Profile::combine', references: 3, usable_references: 2, source_window: { ...window, start_frame: 0, requested: null },
    quantities: [
      { quantity: 'integrated', unit: 'LUFS', basis: 'Heard integrated / Profile::integrated; distinct from canonical Analysis', source: -13.25, source_reason: null, target: { mid: -11.25, low: -12.25, high: -10.25 }, target_reason: null, contributors: 2, references: 3, deviation: -2, inside_spread: false },
      { quantity: 'low_width', unit: 'dB', basis: 'Heard side/mid below 120 Hz / Profile::low_width', source: null, source_reason: 'mono', target: null, target_reason: 'no_reference_measurement', contributors: 0, references: 3, deviation: null, inside_spread: null }
    ], range_note: 'Spreads include minimum tolerances, not confidence intervals.', peak_note: 'No Profile true-peak target; canonical peaks remain separate.'
  }
})
