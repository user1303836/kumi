// Setup's next step, shared by Desktop (buttons that ask for it) and the dashboard (requests to copy): the step names
// the producer's DAW, or each DAW until one is chosen, never the default one.
import { connectRequest, dawName, setupRequest } from './format.ts'
import { english, type Translate } from './i18n.ts'
import { dawPartWorks, type KumiState } from './store.ts'

// Setup states (W5's `setup.status()`) that a setup request fixes. `no_pin` and `unsupported_platform` it can't.
const NEEDS_SETUP: Record<string, true> = { needs_runtime: true, outdated_runtime: true, damaged_runtime: true, needs_daw_component: true }
// The DAWs setup is offered for before one is chosen in Kumi's settings, as setup names them.
const SETUP_DAWS = ['ableton', 'reaper', 'renoise']
// Setup's last steps, taken in the DAW (choose Kumi's control surface, restart the DAW so it loads Kumi's part, turn on
// its tool): setup's message says what to do there until Kumi connects.
const IN_DAW: Record<string, true> = { choose_control_surface: true, restart_daw: true, enable_tool: true }

/** Whether a DAW is chosen in Kumi's settings. A backend from before `daw_chosen` reads as chosen, as it always did. */
export function dawChosen(state: KumiState): boolean {
  return state.setup?.daw_chosen !== false
}

/** Setup's next step, as the buttons that ask for it (labels in `t`'s words; the requests stay English, C5). Until a
 *  DAW is chosen in Kumi's settings, setup is offered for each DAW: setting one up makes it Kumi's DAW in the same
 *  approval (setup's `use_daw`). */
export function nextSteps(state: KumiState, t: Translate = english): { label: string; request: string }[] {
  const setup = state.setup
  const daw = state.status?.daw ?? setup?.daw
  if (setup && !dawPartWorks(state) && (NEEDS_SETUP[setup.state ?? ''] || setup.next_step === 'install_runtime' || setup.next_step === 'install_daw_component')) {
    return (dawChosen(state) ? [daw] : SETUP_DAWS).map(each => ({ label: t('step.setup', dawName(each, t)), request: setupRequest(each) }))
  }
  if (setup?.state === 'ready' && !state.status?.connected) return [{ label: t('step.connect', dawName(daw, t)), request: connectRequest(daw) }]
  return []
}

/** Setup's own words: only while there's something to do, and not ones that name a DAW nobody chose. */
export function setupWords(state: KumiState): string | null {
  const setup = state.setup
  const said = setup?.state
  if (typeof setup?.message !== 'string' || !setup.message || !said || said === 'unknown' || dawPartWorks(state)) return null
  if (said === 'ready') return !state.status?.connected && IN_DAW[setup.next_step ?? ''] ? setup.message : null
  // "next, setup installs Kumi's part in Live" for a producer who hasn't said Live: the buttons name each DAW instead.
  if (said === 'needs_daw_component' && !dawChosen(state)) return null
  return setup.message
}