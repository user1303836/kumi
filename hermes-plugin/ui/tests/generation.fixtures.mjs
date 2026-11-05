// CC-13's fixtures (CONTRACT-CHANGES.md): a quote, a create answer, a running and a finished `kumi_job` envelope, and
// the cases the UI must word with care (an unknown charge, a frozen ledger, notes from audio, a raw snapshot).
const ledger = (over = {}) => ({ cap: 100, spent: 20, reserved: 0, left: 80, frozen: false, freeze_reason: null, ...over })

export const QUOTE = {
  state: 'quoted',
  quote_id: 'q-opaque-id',
  kind: 'sfx',
  prompt: 'a short metallic impact',
  file: null,
  seconds: 5,
  estimate: { credits: 15, max_credits: 15 },
  credit_unit: 'mirelo_credits',
  upload: false,
  expires_at: 1793833200000,
  request_sha256: '0'.repeat(64),
  summary: 'One 5-second sound effect',
  ledger: ledger()
}

export const CREATE = {
  state: 'running',
  job_id: 'native-job-id',
  quote_id: 'q-opaque-id',
  kind: 'sfx',
  prompt: 'a short metallic impact',
  seconds: 5,
  estimate: { credits: 15, max_credits: 15 },
  credit_unit: 'mirelo_credits',
  ledger: ledger({ reserved: 15, left: 65 })
}

const running = {
  quote_id: 'q-opaque-id',
  kind: 'sfx',
  prompt: 'a short metallic impact',
  seconds: 5,
  provider: 'mirelo',
  provider_job_id: 'vendor-job-id',
  submission: 'submitted',
  artifact_id: null,
  file: null,
  output_type: null,
  outputs: [],
  media_tag: null,
  credits: { estimate: 15, reserved: 15, actual: null },
  credit_unit: 'mirelo_credits',
  ledger: ledger({ reserved: 15, left: 65 })
}

export const RUNNING = { job_id: 'native-job-id', kind: 'generate', state: 'running', progress: { done: 0, message: 'Mirelo is generating' }, result: running }

const done = {
  ...running,
  artifact_id: 'a-primary',
  file: 'a-primary.wav',
  output_type: 'audio',
  outputs: [{ artifact_id: 'a-primary', file: 'a-primary.wav', type: 'audio', mime: 'audio/wav', bytes: 480044 }],
  media_tag: 'MEDIA:/profile-local/state/artifacts/a-primary.wav',
  credits: { estimate: 15, reserved: 15, actual: 12 },
  ledger: ledger({ spent: 32, left: 68 })
}

export const DONE = { job_id: 'native-job-id', kind: 'generate', state: 'done', progress: { done: 1, total: 1 }, result: done }

/** A raw native job snapshot (the plugin's `/job` route): the same result without `ledger`. */
const { ledger: _dropped, ...unreconciled } = done
export const RAW_DONE = { ...DONE, result: unreconciled }

/** Submitted, then no answer: the charge isn't known, and the reservation stays. */
export const FAILED_UNKNOWN = {
  job_id: 'native-job-2',
  kind: 'generate',
  state: 'failed',
  progress: { done: 0 },
  error: { code: 'submission_unknown', reason: 'Mirelo didn’t answer after the request was sent' },
  result: { ...running, submission: 'unknown' }
}

/** A charge above the job's reservation: the outputs stand, and new creates are frozen. */
export const FROZEN = {
  ...DONE,
  job_id: 'native-job-3',
  result: { ...done, credits: { estimate: 15, reserved: 15, actual: 18 }, ledger: ledger({ spent: 38, left: 62, frozen: true, freeze_reason: 'actual_exceeded_estimate' }) }
}

/** Notes from audio: a real `.mid` as the primary output, with the notes in seconds. */
export const MIDI_DONE = {
  job_id: 'native-job-4',
  kind: 'generate',
  state: 'done',
  progress: { done: 1, total: 1 },
  result: {
    ...done,
    kind: 'audio_to_midi',
    prompt: null,
    seconds: 8.2,
    artifact_id: 'a-midi',
    file: 'a-midi.mid',
    output_type: 'midi',
    outputs: [
      { artifact_id: 'a-midi', file: 'a-midi.mid', type: 'midi', mime: 'audio/midi', bytes: 1024 },
      { artifact_id: 'a-notes', file: 'a-notes.json', type: 'notes', mime: 'application/json', bytes: 9000 }
    ],
    media_tag: 'MEDIA:/profile-local/state/artifacts/a-midi.mid',
    note_count: 128,
    notes: [{ pitch: 60, velocity: 100, start: 0, end: 0.5, instrument: 0 }],
    timing: 'seconds',
    credits: { estimate: 10, reserved: 10, actual: 10 },
    ledger: ledger({ spent: 30, left: 70 })
  }
}
