// Kumi's UI words in English, Japanese and Simplified Chinese. Desktop takes them through `ctx.i18n.register` and
// translates them in its own language (`usePluginI18n`); the dashboard, and a Desktop without plugin i18n, use
// `translator` with its locale. What goes to the model (Undo, Use, Set up, Connect, look again) stays English: those
// requests are a contract (C5), not UI words. FOCUS and HISTORY stay as they are in every language, as in Kumi's docs.
// No React here: the pure helpers in format.ts and listen.ts take a `Translate`; ui.tsx holds the React context.

/** `t('map.count', '640')`: a dotted key, then the message's arguments. A key no catalog has comes back as itself. */
export type Translate = (key: string, ...args: unknown[]) => string

export type Locale = 'en' | 'ja' | 'zh'

type Shape<T> = { [K in keyof T]: T[K] extends (...args: infer A) => string ? (...args: A) => string : T[K] extends string ? string : Shape<T[K]> }

const en = {
  common: {
    yourDaw: 'your DAW'
  },
  chip: {
    kumi: 'Kumi',
    stopped: 'Kumi stopped',
    notAnswering: 'not answering',
    notConnected: (daw: string) => `Kumi · ${daw} not connected`,
    label: (text: string) => `Kumi: ${text}`,
    openMap: 'Open the sound map',
    settings: 'Kumi settings'
  },
  step: {
    setup: (daw: string) => `Set up Kumi for ${daw}`,
    connect: (daw: string) => `Connect to ${daw}`
  },
  time: {
    now: 'now',
    minutes: (n: number) => `${n} min ago`,
    hours: (n: number) => `${n} h ago`,
    days: (n: number) => `${n} d ago`
  },
  pane: {
    title: 'Kumi',
    agentOff: "Kumi's Agent switch is off in this profile.",
    agentOffHow:
      "Turn on the kumi plugin's Agent switch in Capabilities → Plugins, then restart the gateway. This pane shows your DAW's FOCUS and Kumi's HISTORY once it's on.",
    reading: "Reading Kumi's state…",
    backendError: (error: string) => `Hermes didn't answer Kumi's pane: ${error}`,
    unsure: (op: string, reason: string) => `Kumi isn't sure your project is as it was after ${op}${reason ? `: ${reason}` : ''}.`,
    unsureOp: 'its last action',
    needsLook: 'It needs a fresh look before another change.',
    lookAgain: 'Ask Hermes to look again',
    focus: 'FOCUS',
    jobs: 'Jobs',
    history: 'HISTORY',
    undoable: (n: number) => `· ${n} can be undone`,
    undo: 'Undo',
    undoTitle: 'Ask Hermes to undo this change',
    undoChangeTitle: (n: number) => `Ask Hermes to undo all ${n} steps of this change, with one approval`,
    undoLabel: (change: string) => `Undo ${change}`,
    openMap: 'Open the sound map',
    undoNeedsConnection: 'Undo needs a connected Kumi.'
  },
  hint: {
    sees: 'Kumi sees',
    lastSaw: 'Kumi last saw',
    title: 'What Kumi sees selected in your DAW: words like “this track” in this chat mean it.',
    titleUnsure: "What Kumi last saw selected in your DAW. It isn't sure your project is as it was: ask it to look again."
  },
  focus: {
    nothing: "Nothing in focus yet. Kumi shows what you select in your DAW once it's connected.",
    nothingSelected: 'Nothing selected',
    project: 'The open project',
    playing: '▶ playing',
    stopped: '■ stopped',
    busy: "The DAW answers late while this lasts, so what's shown may be behind",
    kumiStopped: 'Kumi stopped',
    notAnswering: 'not answering',
    since: 'Since Kumi last looked',
    more: (n: number) => `and ${n} more`,
    lastHeard: (ago: string) => `Last heard ${ago}.`
  },
  history: {
    empty: 'No changes yet. Each change Kumi makes in your DAW shows here, with its undo.',
    applied: 'applied',
    undone: 'undone',
    kept: 'kept (you changed it since)',
    expired: "can't be undone: Kumi restarted since",
    partial: 'partly applied',
    unsure: 'unsure',
    refused: 'refused',
    failed: 'failed',
    recorded: 'recorded',
    unconfirmed: 'Kumi couldn’t confirm what happened.',
    lookFirst: 'Ask Hermes to look again before another change.',
    restored: 'Kumi put back what it changed meanwhile.',
    restoreUnsure: 'Kumi isn’t sure it put everything back.',
    restartedSince: (daw: string) =>
      `Kumi restarted since this change, so it can't undo it safely: undo it in ${daw} if it's still the latest step there, or ask Hermes to set it back.`,
    dawUndo: (daw: string) => `Next in ${daw}'s Undo, if nothing has changed there since`,
    dawUndoTitle: (daw: string, keys: string) =>
      `No later Kumi change sits above it in ${daw}'s Undo. If nothing has changed there since, press ${keys} as this row says, right away.`,
    byHand: (daw: string) => `Remove it by hand in ${daw}, or undo the later changes first.`,
    byHandTitle: (daw: string) =>
      `${daw}'s undo would take back what came after first. Remove it by hand instead, or undo the later changes first.`,
    scriptUndo: (daw: string) => `Kumi can't undo a script: ${daw}'s own Undo may, if its step is still the latest there.`
  },
  job: {
    scanning: 'Scanning',
    generating: 'Generating',
    generation: 'Generation',
    scan: 'Scan',
    job: 'Job',
    running: (kind: string) => `${kind} running`,
    of: (done: string, total: string) => `${done} of ${total}`,
    done: (noun: string) => `${noun} done`,
    sounds: (n: number, shown: string) => (n === 1 ? `${shown} sound` : `${shown} sounds`),
    unreadable: (shown: string) => `${shown} couldn't be read`,
    failed: (noun: string) => `${noun} failed`,
    stopped: (noun: string) => `${noun} stopped`,
    interrupted: (noun: string) => `${noun} interrupted`,
    interruptedWhy: 'Kumi stopped before it finished.',
    underWay: (title: string) => `${title}: under way`,
    percent: (title: string, percent: number) => `${title}: ${percent}%`
  },
  map: {
    tab: 'Sound map',
    canvas: 'Sound map',
    roledescription: 'sound map',
    keys: 'Arrow keys move to the nearest sound that way. Shift with left or right steps through every sound in turn, Shift with up or down zooms, and Home shows the whole map. Tab goes on to Play and Use… for the sound picked.',
    agentOff: "Kumi's Agent switch is off in this profile, so there's no map to show.",
    reading: 'Reading the sound map…',
    empty: 'No sound map yet. Ask Hermes to add a sample folder and scan it: the map is made from what it finds there.',
    search: 'Search sounds',
    searchHint: 'Search the map: name, kind or folder',
    count: (n: string) => `${n} sounds`,
    countOf: (shown: string, total: string) => `${shown} of ${total} sounds`,
    revision: (revision: string) => `map ${revision}`,
    show: (group: string) => `Show ${group}`,
    hide: (group: string) => `Hide ${group}`,
    matches: 'Sounds that match',
    found: 'Sounds Kumi found',
    more: (n: number) => `and ${n} more on the map`,
    play: '▶ Play',
    stop: '■ Stop',
    playTitle: 'Play it here (nothing changes in your DAW)',
    use: 'Use…',
    useTitle: 'Put a request to use this sound in the chat, to finish and send',
    moreLike: 'More like this',
    moreLikeTitle: 'Put a request for sounds like this one in the chat, to send when you like',
    playFailed: (error: string) => `Kumi couldn't play that sound: ${error}`
  },
  measurements: {
    tab: 'Measurements',
    nothing: 'Nothing measured yet. When Kumi measures audio in your chat, its numbers show here as well.',
    tooMuch: "Kumi measured it, but there's too much to show here: the numbers are in its reply in the chat.",
    inReply: 'Kumi measured it: the numbers are in its reply in the chat.',
    measured: (ago: string) => `Measured ${ago}`
  },
  script: {
    waiting: 'Script waiting for your approval',
    deviceWaiting: 'Device waiting for your approval',
    says: (purpose: string) => `Hermes says: ${purpose}`,
    python: 'Python',
    lua: 'Lua',
    device: 'A Max for Live device (GenExpr)',
    controls: 'The device’s controls',
    noControls: 'No controls.',
    control: 'Control',
    range: 'Range',
    default: 'Default',
    in: (language: string, daw: string) => `${language} in ${daw}`,
    lines: (count: number) => (count === 1 ? '1 line' : `${count} lines`),
    sha: (short: string) => `SHA-256 ${short}…`,
    shaTitle: (sha: string) => `SHA-256 ${sha}: the hash of the code the approval names`,
    code: 'The script’s code',
    deviceCode: 'The device’s GenExpr code',
    codeTitle: 'Every line of the code as it will run. Nothing runs from here.',
    reading: 'Reading the code Kumi saved…',
    unreadable: "Kumi couldn't read the saved code, or it's no longer what the approval names: read it in the approval, or reject it.",
    unreadableSaved: "Kumi couldn't read the code this script ran: its saved copy is gone, or no longer matches the hash Kumi kept.",
    show: 'Show code',
    hide: 'Hide code',
    showTitle: 'The code this script ran, as Kumi saved it when its approval asked'
  },
  listen: {
    measured: 'measured',
    measuredTitle: 'Measured on this computer by Kumi, from the audio file itself',
    captureTitle: "Kumi played this window and measured it; it isn't a change, so there's nothing to undo",
    waveform: (label: string) => `Waveform of ${label}`,
    play: (label: string) => `Play ${label}`,
    stop: (label: string) => `Stop ${label}`,
    audioGone: "The audio isn't available any more.",
    this: 'This',
    reference: 'Reference',
    difference: 'Difference',
    bandsDifference: 'Band balance, this minus reference (level-matched)',
    bandsLevel: 'Band levels',
    measuredAudio: 'Measured audio',
    loudness: 'Loudness',
    truePeak: 'True peak',
    brightness: 'Brightness (centroid)',
    range: 'Loudness range',
    crest: 'Crest',
    tempo: 'Tempo',
    key: 'Key',
    peakToLoudness: 'Peak to loudness',
    tilt: 'Tilt',
    perOctave: (value: string) => `${value} dB/oct`,
    width: 'Stereo width',
    lowEnd: 'Low end',
    mono: 'mono',
    notMono: 'not mono',
    overlay: 'Band levels, level-matched',
    overlayBand: (name: string, hz: string, mine: string, reference: string, difference: string) =>
      `${name} (${hz} Hz): this ${mine} dB, reference ${reference} dB, ${difference} dB`,
    round: (number: number) => `Round ${number}`,
    roundTitle: 'Each comparison with this reference since this window opened, oldest first: this minus reference',
    loudnessGap: (trend: string) => `loudness ${trend} LU`,
    tiltGap: (trend: string) => `tilt ${trend} dB/oct`,
    widthGap: (trend: string) => `width ${trend}`,
    interpretation: 'Interpretation',
    noInterpretation: (why: string) => `No model interpretation: ${why}`,
    more: 'More',
    less: 'Less',
    beats: (from: string, to: string) => `Beats ${from}–${to}`,
    secondsMeasured: (seconds: string) => `${seconds} s measured`,
    through: (by: string) => `through ${by}`,
    ears: 'Kumi Ears',
    offline: 'an offline render',
    restored: 'your project put back as it was',
    versus: (reference: string) => ` vs ${reference}`
  },
  settings: {
    intro: (daw: string) =>
      `Kumi reads and changes your ${daw || 'DAW'} project through typed changes you approve, keeps HISTORY with undo, measures audio on this computer, and maps the sample folders you allow.`,
    agentOff:
      "Kumi's Agent switch is off in this profile. Turn on the kumi plugin's Agent switch in Capabilities → Plugins and restart the gateway; its settings form appears under Agent settings.",
    setup: 'Setup',
    state: 'State',
    next: 'Next',
    runtime: 'Kumi runtime',
    installed: (version: string) => `Kumi ${version}`,
    notInstalled: 'Not installed',
    needs: (version: string) => `needs ${version}`,
    download: 'Download',
    unpacked: (size: string) => `(${size} unpacked)`,
    part: (daw: string) => `Kumi's part in ${daw}`,
    componentLine: (name: string, state: string) => `${name}: ${state}`,
    standalone: 'Standalone Kumi',
    adopted: 'adopted',
    leftAsIs: 'left as it is',
    audioFormats: 'Audio formats',
    connection: 'Connection',
    now: 'Now',
    chat: "Kumi's chat",
    chatHere: 'this chat',
    chatElsewhere: 'another chat',
    openMap: 'Open the sound map',
    agentSettings: 'Agent settings',
    thisWindow: 'This window',
    openOnFind: 'Open the sound map when Kumi finds sounds',
    petLines: 'Let the pet say when Kumi finishes a change or a job',
    states: {
      ready: 'Ready',
      needs_runtime: 'Not set up yet',
      outdated_runtime: 'Needs an update: run setup',
      damaged_runtime: 'Needs reinstalling',
      needs_daw_component: "Needs Kumi's part in your DAW",
      no_pin: "This copy of the plugin can't set Kumi up",
      unsupported_platform: 'Not available on this system',
      unknown: 'Unknown'
    },
    platforms: {
      'aarch64-apple-darwin': 'macOS (Apple silicon)',
      'x86_64-apple-darwin': 'macOS (Intel)',
      'aarch64-pc-windows-msvc': 'Windows (ARM64)',
      'x86_64-pc-windows-msvc': 'Windows (x64)',
      'aarch64-unknown-linux-gnu': 'Linux (ARM64)',
      'x86_64-unknown-linux-gnu': 'Linux (x86-64)'
    },
    componentStates: {
      not_installed: 'not installed',
      installed: 'installed',
      outdated: 'outdated',
      unknown: 'unknown'
    },
    partInUse: 'in use (Kumi is connected through it)'
  },
  generate: {
    generated: 'generated',
    generatedTitle: "Made by Mirelo from your words. It isn't in your project until you place it.",
    soundTitle: 'Generated sound',
    notesTitle: 'Notes from audio',
    kinds: { video_to_sfx: 'Sound for a video', extend: 'Extended audio', inpaint: 'Inpainted audio' },
    // CC-15: what a generation's billed seconds covered, never the returned file's length.
    window: {
      video_to_sfx: (first: string) => `Sound for video · first ${first}`,
      extend: (added: string, source: string) => `Extended by ${added} (source ${source})`,
      inpaint: (span: string, source: string) => `Replaced ${span} of ${source}`
    },
    credits: (n: number, shown: string) => (n === 1 ? `${shown} credit` : `${shown} credits`),
    creditsTitle: 'Mirelo API credits, not money. An estimate or a reservation is not a price.',
    cost: (credits: string, estimate: string) => `${credits} (est. ${estimate})`,
    reserved: (credits: string) => `${credits} reserved`,
    unknownCharge: (credits: string) => `charge not known yet · ${credits} reserved`,
    notSubmitted: 'never sent to Mirelo · nothing charged',
    left: (left: string, cap: string) => `${left} of ${cap} credits left under your cap`,
    capTitle: "Kumi's own budget for paid generation, which approvals count against. It isn't the provider's bill.",
    frozen: 'New paid generations are paused: a charge came in above its estimate',
    notes: (n: number, shown: string) => (n === 1 ? `${shown} note` : `${shown} notes`),
    outputs: { midi: 'MIDI file', notes: 'Notes', musicxml: 'MusicXML' },
    states: { running: 'generating', failed: 'failed', cancelled: 'cancelled', interrupted: 'interrupted' },
    place: 'Place…',
    placeTitle: 'Put a request to place this file in your project in the chat, to finish and send',
    planNotes: 'Plan notes…',
    planNotesTitle: "Put a request in the chat to plan where these notes go, from your project's own timing, to finish and send"
  },
  pet: {
    undone: (change: string) => `Undone: ${change}`
  },
  palette: {
    map: 'Kumi: open the sound map',
    undo: 'Kumi: undo the last change',
    settings: 'Kumi: settings'
  },
  notify: {
    nothingToUndo: 'Kumi has no change it can undo right now.',
    undoUnsure: "Kumi isn't sure of its last change: ask Hermes to have it look at your project again first.",
    copied: 'Copied the request: paste it into the chat to send it.',
    askInChat: (text: string) => `Ask in the chat: ${text}`,
    openChat: 'Open a chat first, then try again.',
    notOpenHere: "Kumi's chat isn't open here, so the request is in your input, unsent. Send it from the chat Kumi is connected to."
  },
  dashboard: {
    agentOff: "Kumi's Agent switch is off in this profile.",
    agentOffHow: "Turn it on (the Plugins page, or `hermes plugins enable kumi`) and restart the dashboard to see Kumi here.",
    copyTitle: (text: string) => `Copy “${text}” to paste into a chat`,
    copied: 'Copied',
    copyRequest: 'Copy request',
    copyUndo: 'Copy undo',
    mapEmpty: 'No sound map yet: ask Hermes to add a sample folder and scan it.',
    askLook: 'Ask Hermes in your chat to look again before another change.'
  }
}

const ja: Shape<typeof en> = {
  common: {
    yourDaw: 'DAW'
  },
  chip: {
    kumi: 'Kumi',
    stopped: 'Kumi は停止中',
    notAnswering: '応答なし',
    notConnected: daw => `Kumi · ${daw} 未接続`,
    label: text => `Kumi: ${text}`,
    openMap: 'サンプルマップを開く',
    settings: 'Kumi の設定'
  },
  step: {
    setup: daw => `${daw} 用に Kumi をセットアップ`,
    connect: daw => `${daw} に接続`
  },
  time: {
    now: 'たった今',
    minutes: n => `${n} 分前`,
    hours: n => `${n} 時間前`,
    days: n => `${n} 日前`
  },
  pane: {
    title: 'Kumi',
    agentOff: 'このプロファイルでは Kumi の Agent スイッチがオフです。',
    agentOffHow:
      '「スキルとツール → プラグイン」で kumi プラグインの Agent スイッチをオンにし、ゲートウェイを再起動してください。オンになると、このペインに DAW の FOCUS と Kumi の HISTORY が表示されます。',
    reading: 'Kumi の状態を読み込み中…',
    backendError: error => `Kumi のペインに Hermes が応答しませんでした: ${error}`,
    unsure: (op, reason) => `${op}のあと、プロジェクトが元のままか Kumi は確信できません${reason ? `（${reason}）` : ''}。`,
    unsureOp: '直前の操作',
    needsLook: '次の変更の前に、もう一度確認が必要です。',
    lookAgain: 'Hermes にもう一度確認してもらう',
    focus: 'FOCUS',
    jobs: 'ジョブ',
    history: 'HISTORY',
    undoable: n => `· ${n} 件取り消せます`,
    undo: '取り消す',
    undoTitle: 'この変更の取り消しを Hermes に頼む',
    undoChangeTitle: n => `この変更の ${n} ステップすべての取り消しを、1 回の承認で Hermes に頼む`,
    undoLabel: change => `${change} を取り消す`,
    openMap: 'サンプルマップを開く',
    undoNeedsConnection: '取り消すには Kumi の接続が必要です。'
  },
  hint: {
    sees: 'Kumi が見ているもの:',
    lastSaw: 'Kumi が最後に見たもの:',
    title: 'Kumi が DAW で選択中と見ているもの。このチャットで「このトラック」などと言うと、これを指します。',
    titleUnsure: 'Kumi が最後に DAW で選択中と見たもの。プロジェクトが元のままか確信できないので、もう一度確認するよう頼んでください。'
  },
  focus: {
    nothing: 'まだ何もフォーカスされていません。接続すると、DAW で選択したものを Kumi が表示します。',
    nothingSelected: '何も選択されていません',
    project: '開いているプロジェクト',
    playing: '▶ 再生中',
    stopped: '■ 停止中',
    busy: 'この間は DAW の応答が遅れるため、表示が実際より遅れることがあります',
    kumiStopped: 'Kumi は停止中',
    notAnswering: '応答なし',
    since: 'Kumi が前回見てから',
    more: n => `ほか ${n} 件`,
    lastHeard: ago => `最終応答: ${ago}。`
  },
  history: {
    empty: 'まだ変更はありません。Kumi が DAW に加えた変更は、取り消しとともにここに表示されます。',
    applied: '適用済み',
    undone: '取り消し済み',
    kept: '維持（その後あなたが変更しました）',
    expired: '取り消せません: Kumi が再起動しました',
    partial: '一部のみ適用',
    unsure: '不確か',
    refused: '拒否',
    failed: '失敗',
    recorded: '記録',
    unconfirmed: '何が起きたか Kumi は確認できませんでした。',
    lookFirst: '次の変更の前に、Hermes にもう一度確認してもらってください。',
    restored: 'その間に変えたものは Kumi が元に戻しました。',
    restartedSince: daw =>
      `この変更のあと Kumi が再起動したため、安全に取り消せません。${daw} でまだ最後の操作なら ${daw} で取り消すか、Hermes に元に戻すよう頼んでください。`,
    restoreUnsure: 'すべて元に戻せたか Kumi は確信できません。',
    dawUndo: daw => `${daw} の取り消しで次の操作（その後 ${daw} で何も変えていなければ）`,
    dawUndoTitle: (daw, keys) => `${daw} の取り消しで、この上に Kumi の後の変更はありません。その後 ${daw} で何も変えていなければ、この行の説明のとおり、すぐに ${keys} を押してください。`,
    byHand: daw => `${daw} で手動で削除するか、後の変更を先に取り消してください。`,
    byHandTitle: daw => `${daw} の取り消しは、その後の変更を先に戻してしまいます。手動で削除するか、後の変更を先に取り消してください。`,
    scriptUndo: daw => `Kumi はスクリプトを取り消せません。${daw} でまだ最後の操作なら、${daw} の取り消しで戻せることがあります。`
  },
  job: {
    scanning: 'スキャン中',
    generating: '生成中',
    generation: '生成',
    scan: 'スキャン',
    job: 'ジョブ',
    running: kind => `${kind} 実行中`,
    of: (done, total) => `${done} / ${total}`,
    done: noun => `${noun}完了`,
    sounds: (_n, shown) => `${shown} 個のサウンド`,
    unreadable: shown => `${shown} 個は読み込めませんでした`,
    failed: noun => `${noun}失敗`,
    stopped: noun => `${noun}停止`,
    interrupted: noun => `${noun}中断`,
    interruptedWhy: '完了する前に Kumi が停止しました。',
    underWay: title => `${title}: 進行中`,
    percent: (title, percent) => `${title}: ${percent}%`
  },
  map: {
    tab: 'サンプルマップ',
    canvas: 'サンプルマップ',
    roledescription: 'サンプルマップ',
    keys: '矢印キーでその方向の一番近いサウンドへ移動します。Shift と左右キーで全サウンドを順に移動、Shift と上下キーでズーム、Home でマップ全体を表示します。Tab で選んだサウンドの再生と使用に移れます。',
    agentOff: 'このプロファイルでは Kumi の Agent スイッチがオフのため、表示できるマップがありません。',
    reading: 'サンプルマップを読み込み中…',
    empty: 'まだサンプルマップがありません。Hermes にサンプルフォルダの追加とスキャンを頼んでください。見つかったサウンドからマップが作られます。',
    search: 'サウンドを検索',
    searchHint: 'マップを検索: 名前・種類・フォルダ',
    count: n => `${n} 個のサウンド`,
    countOf: (shown, total) => `${total} 個中 ${shown} 個のサウンド`,
    revision: revision => `マップ ${revision}`,
    show: group => `${group} を表示`,
    hide: group => `${group} を隠す`,
    matches: '一致するサウンド',
    found: 'Kumi が見つけたサウンド',
    more: n => `ほか ${n} 個はマップ上に`,
    play: '▶ 再生',
    stop: '■ 停止',
    playTitle: 'ここで再生（DAW は何も変わりません）',
    use: '使う…',
    useTitle: 'このサウンドを使う依頼をチャットに入れます。仕上げて送信してください',
    moreLike: '似たサウンド',
    moreLikeTitle: 'これに似たサウンドを探す依頼をチャットに入れます。送信はご自身で',
    playFailed: error => `Kumi はそのサウンドを再生できませんでした: ${error}`
  },
  measurements: {
    tab: '測定結果',
    nothing: 'まだ測定はありません。Kumi がチャットでオーディオを測定すると、その数値がここにも表示されます。',
    tooMuch: 'Kumi は測定しましたが、ここに表示するには多すぎます。数値はチャットの返答にあります。',
    inReply: 'Kumi は測定しました。数値はチャットの返答にあります。',
    measured: ago => `測定: ${ago}`
  },
  script: {
    waiting: '承認待ちのスクリプト',
    deviceWaiting: '承認待ちのデバイス',
    says: purpose => `Hermes の説明: ${purpose}`,
    python: 'Python',
    lua: 'Lua',
    device: 'Max for Live デバイス（GenExpr）',
    controls: 'デバイスのコントロール',
    noControls: 'コントロールはありません。',
    control: 'コントロール',
    range: '範囲',
    default: '初期値',
    in: (language, daw) => `${daw} の ${language}`,
    lines: count => `${count} 行`,
    sha: short => `SHA-256 ${short}…`,
    shaTitle: sha => `SHA-256 ${sha}（承認が示すコードのハッシュ）`,
    code: 'スクリプトのコード',
    deviceCode: 'デバイスの GenExpr コード',
    codeTitle: '実行されるとおりのコードの全行です。ここからは何も実行されません。',
    reading: 'Kumi が保存したコードを読み込み中…',
    unreadable: '保存されたコードを読めないか、承認が示すコードではなくなっています。承認で確認するか、拒否してください。',
    unreadableSaved: 'このスクリプトが実行したコードを読めませんでした。保存したコピーがないか、Kumi が控えたハッシュと一致しません。',
    show: 'コードを表示',
    hide: 'コードを隠す',
    showTitle: 'このスクリプトが実行したコード（承認を求めたときに Kumi が保存したもの）'
  },
  listen: {
    measured: '測定値',
    measuredTitle: 'Kumi がこのコンピュータ上で、オーディオファイルそのものから測定しました',
    captureTitle: 'Kumi はこの区間を再生して測定しました。変更ではないので、取り消すものはありません',
    waveform: label => `${label} の波形`,
    play: label => `${label} を再生`,
    stop: label => `${label} を停止`,
    audioGone: 'このオーディオはもう利用できません。',
    this: 'これ',
    reference: 'リファレンス',
    difference: '差',
    bandsDifference: '帯域バランス（これ − リファレンス、レベル合わせ済み）',
    bandsLevel: '帯域レベル',
    measuredAudio: '測定したオーディオ',
    loudness: 'ラウドネス',
    truePeak: 'トゥルーピーク',
    brightness: '明るさ（セントロイド）',
    range: 'ラウドネスレンジ',
    crest: 'クレスト',
    tempo: 'テンポ',
    key: 'キー',
    peakToLoudness: 'ピーク対ラウドネス',
    tilt: 'ティルト',
    perOctave: value => `${value} dB/oct`,
    width: 'ステレオ幅',
    lowEnd: '低域',
    mono: 'モノ',
    notMono: 'モノではない',
    overlay: '帯域レベル（レベル合わせ済み）',
    overlayBand: (name, hz, mine, reference, difference) => `${name}（${hz} Hz）: これ ${mine} dB、リファレンス ${reference} dB、差 ${difference} dB`,
    round: number => `ラウンド ${number}`,
    roundTitle: 'このウィンドウを開いてからの、このリファレンスとの各比較（古い順、これ − リファレンス）',
    loudnessGap: trend => `ラウドネス ${trend} LU`,
    tiltGap: trend => `ティルト ${trend} dB/oct`,
    widthGap: trend => `幅 ${trend}`,
    interpretation: '解釈',
    noInterpretation: why => `モデルによる解釈なし: ${why}`,
    more: 'もっと見る',
    less: '閉じる',
    beats: (from, to) => `${from}–${to} 拍`,
    secondsMeasured: seconds => `${seconds} 秒を測定`,
    through: by => (/[A-Za-z]$/.test(by) ? `${by} 経由` : `${by}経由`),
    ears: 'Kumi Ears',
    offline: 'オフラインレンダリング',
    restored: 'プロジェクトは元に戻しました',
    versus: reference => `（${reference} と比較）`
  },
  settings: {
    intro: daw =>
      `Kumi は、あなたが承認した型付きの変更で ${daw || 'DAW'} のプロジェクトを読み書きし、HISTORY と取り消しを保ち、このコンピュータ上でオーディオを測定し、許可したサンプルフォルダをマップにします。`,
    agentOff:
      'このプロファイルでは Kumi の Agent スイッチがオフです。「スキルとツール → プラグイン」で kumi プラグインの Agent スイッチをオンにし、ゲートウェイを再起動してください。設定フォームは「エージェント設定」に表示されます。',
    setup: 'セットアップ',
    state: '状態',
    next: '次にすること',
    runtime: 'Kumi ランタイム',
    installed: version => `Kumi ${version}`,
    notInstalled: '未インストール',
    needs: version => `${version} が必要`,
    download: 'ダウンロード',
    unpacked: size => `（展開後 ${size}）`,
    part: daw => `${daw} 用の Kumi の部品`,
    componentLine: (name, state) => `${name}：${state}`,
    standalone: '単体の Kumi',
    adopted: '引き継ぎ済み',
    leftAsIs: 'そのまま',
    audioFormats: 'オーディオ形式',
    connection: '接続',
    now: '現在',
    chat: 'Kumi のチャット',
    chatHere: 'このチャット',
    chatElsewhere: '別のチャット',
    openMap: 'サンプルマップを開く',
    agentSettings: 'エージェント設定',
    thisWindow: 'このウィンドウ',
    openOnFind: 'Kumi がサウンドを見つけたらサンプルマップを開く',
    petLines: 'Kumi が変更やジョブを終えたら、ペットの吹き出しで知らせる',
    states: {
      ready: '準備完了',
      needs_runtime: 'まだセットアップされていません',
      outdated_runtime: '更新が必要です: セットアップを実行してください',
      damaged_runtime: '再インストールが必要です',
      needs_daw_component: 'DAW 用の Kumi の部品が必要です',
      no_pin: 'このプラグインのコピーでは Kumi をセットアップできません',
      unsupported_platform: 'このシステムでは利用できません',
      unknown: '不明'
    },
    platforms: {
      'aarch64-apple-darwin': 'macOS（Apple シリコン）',
      'x86_64-apple-darwin': 'macOS（Intel）',
      'aarch64-pc-windows-msvc': 'Windows（ARM64）',
      'x86_64-pc-windows-msvc': 'Windows（x64）',
      'aarch64-unknown-linux-gnu': 'Linux（ARM64）',
      'x86_64-unknown-linux-gnu': 'Linux（x86-64）'
    },
    componentStates: {
      not_installed: '未インストール',
      installed: 'インストール済み',
      outdated: '古い版',
      unknown: '不明'
    },
    partInUse: '使用中（Kumi はこれを通じて接続しています）'
  },
  generate: {
    generated: '生成',
    generatedTitle: 'Mirelo があなたの言葉から作りました。配置するまでプロジェクトには入りません。',
    soundTitle: '生成したサウンド',
    notesTitle: 'オーディオから取り出したノート',
    kinds: { video_to_sfx: '映像の効果音', extend: '延長したオーディオ', inpaint: '一部を差し替えたオーディオ' },
    window: {
      video_to_sfx: first => `映像の効果音 · 最初の ${first}`,
      extend: (added, source) => `${added} 延長（元は ${source}）`,
      inpaint: (span, source) => `${source} のうち ${span} を差し替え`
    },
    credits: (_n, shown) => `${shown} クレジット`,
    creditsTitle: 'Mirelo API のクレジットで、お金ではありません。見積や確保額は価格ではありません。',
    cost: (credits, estimate) => `${credits}（見積 ${estimate}）`,
    reserved: credits => `${credits} を確保`,
    unknownCharge: credits => `請求額はまだ不明 · ${credits} を確保`,
    notSubmitted: 'Mirelo には送信されませんでした · 請求なし',
    left: (left, cap) => `上限 ${cap} クレジット中 残り ${left}`,
    capTitle: '有料生成のための Kumi 自身の予算で、承認ごとに差し引かれます。提供元の請求額ではありません。',
    frozen: '見積を超える請求があったため、新しい有料生成は止めています',
    notes: (_n, shown) => `${shown} ノート`,
    outputs: { midi: 'MIDI ファイル', notes: 'ノート', musicxml: 'MusicXML' },
    states: { running: '生成中', failed: '失敗', cancelled: 'キャンセル済み', interrupted: '中断' },
    place: '配置…',
    placeTitle: 'このファイルをプロジェクトに配置する依頼をチャットに入れます。仕上げて送信してください',
    planNotes: 'ノートの配置を計画…',
    planNotesTitle: 'プロジェクト自身のタイミングから、これらのノートをどこに置くか計画する依頼をチャットに入れます。仕上げて送信してください'
  },
  pet: {
    undone: change => `取り消しました: ${change}`
  },
  palette: {
    map: 'Kumi: サンプルマップを開く',
    undo: 'Kumi: 最後の変更を取り消す',
    settings: 'Kumi: 設定'
  },
  notify: {
    nothingToUndo: '今すぐ取り消せる変更はありません。',
    undoUnsure: 'Kumi は直前の変更に確信がありません。まずプロジェクトをもう一度確認するよう Hermes に頼んでください。',
    copied: '依頼をコピーしました。チャットに貼り付けて送信してください。',
    askInChat: text => `チャットで頼んでください: ${text}`,
    openChat: '先にチャットを開いてから、もう一度試してください。',
    notOpenHere: 'Kumi のチャットがここで開いていないため、依頼は未送信のまま入力欄にあります。Kumi が接続しているチャットから送信してください。'
  },
  dashboard: {
    agentOff: 'このプロファイルでは Kumi の Agent スイッチがオフです。',
    agentOffHow: 'オンにして（プラグインのページ、または `hermes plugins enable kumi`）、ダッシュボードを再起動すると、ここに Kumi が表示されます。',
    copyTitle: text => `「${text}」をコピーしてチャットに貼り付け`,
    copied: 'コピーしました',
    copyRequest: '依頼をコピー',
    copyUndo: '取り消しをコピー',
    mapEmpty: 'まだサンプルマップがありません。Hermes にサンプルフォルダの追加とスキャンを頼んでください。',
    askLook: '次の変更の前に、チャットで Hermes にもう一度確認するよう頼んでください。'
  }
}

const zh: Shape<typeof en> = {
  common: {
    yourDaw: 'DAW'
  },
  chip: {
    kumi: 'Kumi',
    stopped: 'Kumi 已停止',
    notAnswering: '无响应',
    notConnected: daw => `Kumi · ${daw} 未连接`,
    label: text => `Kumi：${text}`,
    openMap: '打开采样地图',
    settings: 'Kumi 设置'
  },
  step: {
    setup: daw => `为 ${daw} 设置 Kumi`,
    connect: daw => `连接到 ${daw}`
  },
  time: {
    now: '刚刚',
    minutes: n => `${n} 分钟前`,
    hours: n => `${n} 小时前`,
    days: n => `${n} 天前`
  },
  pane: {
    title: 'Kumi',
    agentOff: '此配置中 Kumi 的 Agent 开关已关闭。',
    agentOffHow: '请在“技能与工具 → 插件”中打开 kumi 插件的 Agent 开关，然后重启网关。打开后，此面板会显示 DAW 的 FOCUS 和 Kumi 的 HISTORY。',
    reading: '正在读取 Kumi 的状态…',
    backendError: error => `Hermes 没有响应 Kumi 的面板：${error}`,
    unsure: (op, reason) => `在${op}之后，Kumi 不确定你的项目是否保持原样${reason ? `：${reason}` : ''}。`,
    unsureOp: '上一个操作',
    needsLook: '下一次改动之前，需要重新查看一次。',
    lookAgain: '让 Hermes 重新查看',
    focus: 'FOCUS',
    jobs: '任务',
    history: 'HISTORY',
    undoable: n => `· ${n} 项可撤销`,
    undo: '撤销',
    undoTitle: '让 Hermes 撤销这次改动',
    undoChangeTitle: n => `让 Hermes 用一次批准撤销这次改动的全部 ${n} 步`,
    undoLabel: change => `撤销 ${change}`,
    openMap: '打开采样地图',
    undoNeedsConnection: '撤销需要 Kumi 已连接。'
  },
  hint: {
    sees: 'Kumi 看到：',
    lastSaw: 'Kumi 上次看到：',
    title: 'Kumi 在你的 DAW 中看到的选中内容：在这个对话里说“这条音轨”等，指的就是它。',
    titleUnsure: 'Kumi 上次在你的 DAW 中看到的选中内容。它不确定项目是否保持原样：请让它重新查看。'
  },
  focus: {
    nothing: '还没有焦点。连接后，Kumi 会显示你在 DAW 中选中的内容。',
    nothingSelected: '未选中任何内容',
    project: '当前打开的项目',
    playing: '▶ 播放中',
    stopped: '■ 已停止',
    busy: '在此期间 DAW 响应较慢，显示的内容可能滞后',
    kumiStopped: 'Kumi 已停止',
    notAnswering: '无响应',
    since: '自 Kumi 上次查看以来',
    more: n => `还有 ${n} 项`,
    lastHeard: ago => `最后响应：${ago}。`
  },
  history: {
    empty: '还没有改动。Kumi 在你的 DAW 中做的每次改动都会显示在这里，并可撤销。',
    applied: '已应用',
    undone: '已撤销',
    kept: '已保留（之后你改过它）',
    expired: '无法撤销：Kumi 已重启',
    partial: '部分应用',
    unsure: '不确定',
    refused: '已拒绝',
    failed: '失败',
    recorded: '已记录',
    unconfirmed: 'Kumi 无法确认发生了什么。',
    lookFirst: '下一次改动之前，请让 Hermes 重新查看。',
    restored: 'Kumi 已还原期间做出的改动。',
    restartedSince: daw => `此改动之后 Kumi 已重启，无法安全撤销：如果它仍是 ${daw} 中的最后一步，请在 ${daw} 中撤销，或让 Hermes 把它改回去。`,
    restoreUnsure: 'Kumi 不确定是否已全部还原。',
    dawUndo: daw => `${daw} 撤销中的下一步，前提是之后那里没有任何改动`,
    dawUndoTitle: (daw, keys) => `在 ${daw} 的撤销中，它上面没有 Kumi 之后的改动。如果之后在 ${daw} 中没有任何改动，请按这一行的说明立即按 ${keys}。`,
    byHand: daw => `在 ${daw} 中手动删除，或先撤销之后的改动。`,
    byHandTitle: daw => `${daw} 的撤销会先撤回之后的改动。请手动删除，或先撤销之后的改动。`,
    scriptUndo: daw => `Kumi 无法撤销脚本：如果它仍是 ${daw} 中的最后一步，${daw} 自己的撤销也许可以。`
  },
  job: {
    scanning: '正在扫描',
    generating: '正在生成',
    generation: '生成',
    scan: '扫描',
    job: '任务',
    running: kind => `${kind} 运行中`,
    of: (done, total) => `${done} / ${total}`,
    done: noun => `${noun}完成`,
    sounds: (_n, shown) => `${shown} 个声音`,
    unreadable: shown => `${shown} 个无法读取`,
    failed: noun => `${noun}失败`,
    stopped: noun => `${noun}已停止`,
    interrupted: noun => `${noun}中断`,
    interruptedWhy: 'Kumi 在完成前停止了。',
    underWay: title => `${title}：进行中`,
    percent: (title, percent) => `${title}：${percent}%`
  },
  map: {
    tab: '采样地图',
    canvas: '采样地图',
    roledescription: '采样地图',
    keys: '方向键移到该方向最近的声音。Shift 加左右键按顺序经过每个声音，Shift 加上下键缩放，Home 显示整张地图。按 Tab 可到所选声音的播放和使用。',
    agentOff: '此配置中 Kumi 的 Agent 开关已关闭，没有可显示的地图。',
    reading: '正在读取采样地图…',
    empty: '还没有采样地图。请让 Hermes 添加一个采样文件夹并扫描；地图会根据找到的声音生成。',
    search: '搜索声音',
    searchHint: '搜索地图：名称、种类或文件夹',
    count: n => `${n} 个声音`,
    countOf: (shown, total) => `${total} 个声音中的 ${shown} 个`,
    revision: revision => `地图 ${revision}`,
    show: group => `显示 ${group}`,
    hide: group => `隐藏 ${group}`,
    matches: '匹配的声音',
    found: 'Kumi 找到的声音',
    more: n => `地图上还有 ${n} 个`,
    play: '▶ 播放',
    stop: '■ 停止',
    playTitle: '在这里播放（DAW 不会有任何改动）',
    use: '使用…',
    useTitle: '把使用这个声音的请求放进对话，补充完整后发送',
    moreLike: '更多类似的',
    moreLikeTitle: '把寻找类似声音的请求放进对话，由你决定何时发送',
    playFailed: error => `Kumi 无法播放这个声音：${error}`
  },
  measurements: {
    tab: '测量结果',
    nothing: '还没有测量。Kumi 在对话中测量音频时，数值也会显示在这里。',
    tooMuch: 'Kumi 已完成测量，但内容太多无法在此显示：数值在它在对话中的回复里。',
    inReply: 'Kumi 已完成测量：数值在它在对话中的回复里。',
    measured: ago => `测量于 ${ago}`
  },
  script: {
    waiting: '等待你批准的脚本',
    deviceWaiting: '等待你批准的设备',
    says: purpose => `Hermes 说：${purpose}`,
    python: 'Python',
    lua: 'Lua',
    device: 'Max for Live 设备（GenExpr）',
    controls: '设备的控件',
    noControls: '没有控件。',
    control: '控件',
    range: '范围',
    default: '默认值',
    in: (language, daw) => `${daw} 中的 ${language}`,
    lines: count => `${count} 行`,
    sha: short => `SHA-256 ${short}…`,
    shaTitle: sha => `SHA-256 ${sha}：批准所指代码的哈希`,
    code: '脚本代码',
    deviceCode: '设备的 GenExpr 代码',
    codeTitle: '将要运行的完整代码。这里不会运行任何东西。',
    reading: '正在读取 Kumi 保存的代码…',
    unreadable: 'Kumi 无法读取保存的代码，或它已不是批准所指的代码：请在批准中查看，或拒绝它。',
    unreadableSaved: '无法读取这个脚本运行的代码：保存的副本已不存在，或与 Kumi 记下的哈希不再一致。',
    show: '显示代码',
    hide: '隐藏代码',
    showTitle: '这个脚本运行的代码（在请求批准时由 Kumi 保存）'
  },
  listen: {
    measured: '实测',
    measuredTitle: '由 Kumi 在这台电脑上根据音频文件本身测量',
    captureTitle: 'Kumi 播放并测量了这个片段；这不是改动，所以没有需要撤销的内容',
    waveform: label => `${label} 的波形`,
    play: label => `播放 ${label}`,
    stop: label => `停止 ${label}`,
    audioGone: '这段音频已不可用。',
    this: '当前',
    reference: '参考',
    difference: '差值',
    bandsDifference: '频段平衡（当前减参考，已对齐电平）',
    bandsLevel: '频段电平',
    measuredAudio: '已测量的音频',
    loudness: '响度',
    truePeak: '真峰值',
    brightness: '明亮度（质心）',
    range: '响度范围',
    crest: '峰值因数',
    tempo: '速度',
    key: '调性',
    peakToLoudness: '峰值响度比',
    tilt: '倾斜',
    perOctave: value => `${value} dB/oct`,
    width: '立体声宽度',
    lowEnd: '低频',
    mono: '单声道',
    notMono: '非单声道',
    overlay: '频段电平（已对齐电平）',
    overlayBand: (name, hz, mine, reference, difference) => `${name}（${hz} Hz）：当前 ${mine} dB，参考 ${reference} dB，差 ${difference} dB`,
    round: number => `第 ${number} 轮`,
    roundTitle: '本窗口打开以来与此参考的每次比较（从旧到新，当前减参考）',
    loudnessGap: trend => `响度 ${trend} LU`,
    tiltGap: trend => `倾斜 ${trend} dB/oct`,
    widthGap: trend => `宽度 ${trend}`,
    interpretation: '解读',
    noInterpretation: why => `没有模型解读：${why}`,
    more: '展开',
    less: '收起',
    beats: (from, to) => `第 ${from}–${to} 拍`,
    secondsMeasured: seconds => `测量了 ${seconds} 秒`,
    through: by => (/^[A-Za-z]/.test(by) ? `通过 ${by}` : `通过${by}`),
    ears: 'Kumi Ears',
    offline: '离线渲染',
    restored: '项目已还原',
    versus: reference => `（对比 ${reference}）`
  },
  settings: {
    intro: daw => `Kumi 通过你批准的类型化改动读取和修改你的 ${daw || 'DAW'} 项目，保留可撤销的 HISTORY，在这台电脑上测量音频，并为你允许的采样文件夹生成地图。`,
    agentOff: '此配置中 Kumi 的 Agent 开关已关闭。请在“技能与工具 → 插件”中打开 kumi 插件的 Agent 开关并重启网关；它的设置表单会出现在“智能体设置”中。',
    setup: '设置',
    state: '状态',
    next: '下一步',
    runtime: 'Kumi 运行时',
    installed: version => `Kumi ${version}`,
    notInstalled: '未安装',
    needs: version => `需要 ${version}`,
    download: '下载',
    unpacked: size => `（解压后 ${size}）`,
    part: daw => `Kumi 的 ${daw} 组件`,
    componentLine: (name, state) => `${name}：${state}`,
    standalone: '独立的 Kumi',
    adopted: '已接管',
    leftAsIs: '保持不变',
    audioFormats: '音频格式',
    connection: '连接',
    now: '当前',
    chat: 'Kumi 的对话',
    chatHere: '此对话',
    chatElsewhere: '另一个对话',
    openMap: '打开采样地图',
    agentSettings: '智能体设置',
    thisWindow: '此窗口',
    openOnFind: 'Kumi 找到声音时打开采样地图',
    petLines: 'Kumi 完成改动或任务时，让宠物在气泡里说一声',
    states: {
      ready: '就绪',
      needs_runtime: '尚未设置',
      outdated_runtime: '需要更新：请运行设置',
      damaged_runtime: '需要重新安装',
      needs_daw_component: '需要 Kumi 的 DAW 组件',
      no_pin: '这份插件无法设置 Kumi',
      unsupported_platform: '此系统上不可用',
      unknown: '未知'
    },
    platforms: {
      'aarch64-apple-darwin': 'macOS（Apple 芯片）',
      'x86_64-apple-darwin': 'macOS（Intel）',
      'aarch64-pc-windows-msvc': 'Windows（ARM64）',
      'x86_64-pc-windows-msvc': 'Windows（x64）',
      'aarch64-unknown-linux-gnu': 'Linux（ARM64）',
      'x86_64-unknown-linux-gnu': 'Linux（x86-64）'
    },
    componentStates: {
      not_installed: '未安装',
      installed: '已安装',
      outdated: '版本过旧',
      unknown: '未知'
    },
    partInUse: '使用中（Kumi 正通过它连接）'
  },
  generate: {
    generated: '生成',
    generatedTitle: '由 Mirelo 根据你的描述生成。放入项目之前，它不会出现在你的项目中。',
    soundTitle: '生成的声音',
    notesTitle: '从音频提取的音符',
    kinds: { video_to_sfx: '视频音效', extend: '延长的音频', inpaint: '局部替换的音频' },
    window: {
      video_to_sfx: first => `视频音效 · 前 ${first}`,
      extend: (added, source) => `延长 ${added}（原长 ${source}）`,
      inpaint: (span, source) => `替换了 ${source} 中的 ${span}`
    },
    credits: (_n, shown) => `${shown} 积分`,
    creditsTitle: 'Mirelo API 积分，不是钱。预估或预留不是价格。',
    cost: (credits, estimate) => `${credits}（预估 ${estimate}）`,
    reserved: credits => `已预留 ${credits}`,
    unknownCharge: credits => `费用尚不清楚 · 已预留 ${credits}`,
    notSubmitted: '未发送给 Mirelo · 未收费',
    left: (left, cap) => `上限 ${cap} 积分，剩余 ${left}`,
    capTitle: 'Kumi 自己的付费生成预算，批准时从中扣除。这不是服务商的账单。',
    frozen: '有一笔费用超出了预估，新的付费生成已暂停',
    notes: (_n, shown) => `${shown} 个音符`,
    outputs: { midi: 'MIDI 文件', notes: '音符', musicxml: 'MusicXML' },
    states: { running: '生成中', failed: '失败', cancelled: '已取消', interrupted: '已中断' },
    place: '放入…',
    placeTitle: '把放入这个文件的请求放进对话，补充完整后发送',
    planNotes: '规划音符…',
    planNotesTitle: '把根据项目自身的节拍规划这些音符放在哪里的请求放进对话，补充完整后发送'
  },
  pet: {
    undone: change => `已撤销：${change}`
  },
  palette: {
    map: 'Kumi：打开采样地图',
    undo: 'Kumi：撤销最近一次改动',
    settings: 'Kumi：设置'
  },
  notify: {
    nothingToUndo: 'Kumi 现在没有可以撤销的改动。',
    undoUnsure: 'Kumi 不确定它的上一次改动：请先让 Hermes 让它重新查看你的项目。',
    copied: '已复制请求：粘贴到对话中发送。',
    askInChat: text => `请在对话中提出：${text}`,
    openChat: '请先打开一个对话，然后重试。',
    notOpenHere: 'Kumi 的对话没有在这里打开，所以请求还在你的输入框中，尚未发送。请在 Kumi 连接的对话中发送。'
  },
  dashboard: {
    agentOff: '此配置中 Kumi 的 Agent 开关已关闭。',
    agentOffHow: '打开它（插件页面，或 `hermes plugins enable kumi`）并重启仪表板，即可在这里看到 Kumi。',
    copyTitle: text => `复制“${text}”以粘贴到对话中`,
    copied: '已复制',
    copyRequest: '复制请求',
    copyUndo: '复制撤销',
    mapEmpty: '还没有采样地图：请让 Hermes 添加一个采样文件夹并扫描。',
    askLook: '下一次改动之前，请在对话中让 Hermes 重新查看。'
  }
}

/** The catalogs as Desktop's `ctx.i18n.register` takes them: its own locale ids (`zh` is Simplified Chinese). */
export const MESSAGES = { en, ja, zh }

/** A locale id from Desktop or the dashboard (`ja`, `zh`, `zh-CN`, `en-US`…) as one of ours; English otherwise.
 *  Traditional Chinese (`zh-Hant`, `zh-TW`, `zh-HK`, `zh-MO`) reads English, not the Simplified catalog. */
export function localeOf(raw: unknown): Locale {
  const id = typeof raw === 'string' ? raw.toLowerCase() : ''
  if (id.startsWith('ja')) return 'ja'
  if (/^zh[-_](?:hant|tw|hk|mo)(?![a-z])/.test(id)) return 'en'
  if (id.startsWith('zh')) return 'zh'
  return 'en'
}

function lookup(messages: unknown, key: string): unknown {
  let value = messages
  for (const part of key.split('.')) value = value && typeof value === 'object' ? (value as Record<string, unknown>)[part] : undefined
  return value
}

/** Our own translator for a locale: the message there, else English's, else the key; a function message gets the
 *  arguments. Desktop's `usePluginI18n` resolves the same catalogs the same way. */
export function translator(locale?: unknown): Translate {
  const own = MESSAGES[localeOf(locale)]
  return (key, ...args) => {
    const message = lookup(own, key) ?? lookup(en, key)
    if (typeof message === 'function') return String((message as (...values: unknown[]) => unknown)(...args))
    return typeof message === 'string' ? message : key
  }
}

export const english = translator('en')
