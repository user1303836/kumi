// Built from hermes-plugin/ui/src/ by `npm run build` in hermes-plugin/ui/. Edit ui/src/, not this file.

// src/desktop/plugin.tsx
import * as sdk from "@hermes/plugin-sdk";
import { host as host6, PALETTE_AREA, PANES_AREA, STATUSBAR_AREAS } from "@hermes/plugin-sdk";

// src/shared/i18n.ts
var en = {
  common: {
    yourDaw: "your DAW"
  },
  chip: {
    kumi: "Kumi",
    stopped: "Kumi stopped",
    notAnswering: "not answering",
    notConnected: (daw) => `Kumi · ${daw} not connected`,
    label: (text) => `Kumi: ${text}`,
    openMap: "Open the sound map",
    settings: "Kumi settings"
  },
  step: {
    setup: (daw) => `Set up Kumi for ${daw}`,
    connect: (daw) => `Connect to ${daw}`
  },
  time: {
    now: "now",
    minutes: (n) => `${n} min ago`,
    hours: (n) => `${n} h ago`,
    days: (n) => `${n} d ago`
  },
  pane: {
    title: "Kumi",
    agentOff: "Kumi's Agent switch is off in this profile.",
    agentOffHow: "Turn on the kumi plugin's Agent switch in Capabilities → Plugins, then restart the gateway. This pane shows your DAW's FOCUS and Kumi's HISTORY once it's on.",
    reading: "Reading Kumi's state…",
    backendError: (error) => `Hermes didn't answer Kumi's pane: ${error}`,
    unsure: (op, reason) => `Kumi isn't sure your project is as it was after ${op}${reason ? `: ${reason}` : ""}.`,
    unsureOp: "its last action",
    needsLook: "It needs a fresh look before another change.",
    lookAgain: "Ask Hermes to look again",
    focus: "FOCUS",
    jobs: "Jobs",
    history: "HISTORY",
    undoable: (n) => `· ${n} can be undone`,
    undo: "Undo",
    undoTitle: "Ask Hermes to undo this change",
    undoChangeTitle: (n) => `Ask Hermes to undo all ${n} steps of this change, with one approval`,
    undoLabel: (change) => `Undo ${change}`,
    openMap: "Open the sound map",
    undoNeedsConnection: "Undo needs a connected Kumi."
  },
  hint: {
    sees: "Kumi sees",
    lastSaw: "Kumi last saw",
    title: "What Kumi sees selected in your DAW: words like “this track” in this chat mean it.",
    titleUnsure: "What Kumi last saw selected in your DAW. It isn't sure your project is as it was: ask it to look again."
  },
  focus: {
    nothing: "Nothing in focus yet. Kumi shows what you select in your DAW once it's connected.",
    nothingSelected: "Nothing selected",
    project: "The open project",
    playing: "▶ playing",
    stopped: "■ stopped",
    busy: "The DAW answers late while this lasts, so what's shown may be behind",
    kumiStopped: "Kumi stopped",
    notAnswering: "not answering",
    since: "Since Kumi last looked",
    more: (n) => `and ${n} more`,
    lastHeard: (ago2) => `Last heard ${ago2}.`
  },
  history: {
    empty: "No changes yet. Each change Kumi makes in your DAW shows here, with its undo.",
    applied: "applied",
    undone: "undone",
    kept: "kept (you changed it since)",
    expired: "can't be undone: Kumi restarted since",
    partial: "partly applied",
    unsure: "unsure",
    refused: "refused",
    failed: "failed",
    recorded: "recorded",
    unconfirmed: "Kumi couldn’t confirm what happened.",
    lookFirst: "Ask Hermes to look again before another change.",
    restored: "Kumi put back what it changed meanwhile.",
    restoreUnsure: "Kumi isn’t sure it put everything back.",
    restartedSince: (daw) => `Kumi restarted since this change, so it can't undo it safely: undo it in ${daw} if it's still the latest step there, or ask Hermes to set it back.`,
    dawUndo: (daw) => `Next in ${daw}'s Undo, if nothing has changed there since`,
    dawUndoTitle: (daw, keys) => `No later Kumi change sits above it in ${daw}'s Undo. If nothing has changed there since, press ${keys} as this row says, right away.`,
    byHand: (daw) => `Remove it by hand in ${daw}, or undo the later changes first.`,
    byHandTitle: (daw) => `${daw}'s undo would take back what came after first. Remove it by hand instead, or undo the later changes first.`,
    scriptUndo: (daw) => `Kumi can't undo a script: ${daw}'s own Undo may, if its step is still the latest there.`,
    copyKept: (file) => `Before this, Kumi kept a copy of the project as last saved, next to it: ${file}`
  },
  job: {
    scanning: "Scanning",
    generating: "Generating",
    generation: "Generation",
    scan: "Scan",
    job: "Job",
    running: (kind) => `${kind} running`,
    of: (done, total) => `${done} of ${total}`,
    done: (noun) => `${noun} done`,
    sounds: (n, shown) => n === 1 ? `${shown} sound` : `${shown} sounds`,
    unreadable: (shown) => `${shown} couldn't be read`,
    failed: (noun) => `${noun} failed`,
    stopped: (noun) => `${noun} stopped`,
    interrupted: (noun) => `${noun} interrupted`,
    interruptedWhy: "Kumi stopped before it finished.",
    underWay: (title) => `${title}: under way`,
    percent: (title, percent) => `${title}: ${percent}%`
  },
  map: {
    tab: "Sound map",
    canvas: "Sound map",
    roledescription: "sound map",
    keys: "Arrow keys move to the nearest sound that way. Shift with left or right steps through every sound in turn, Shift with up or down zooms, and Home shows the whole map. Tab goes on to Play and Use… for the sound picked.",
    agentOff: "Kumi's Agent switch is off in this profile, so there's no map to show.",
    reading: "Reading the sound map…",
    empty: "No sound map yet. Ask Hermes to add a sample folder and scan it: the map is made from what it finds there.",
    search: "Search sounds",
    searchHint: "Search the map: name, kind or folder",
    count: (n) => `${n} sounds`,
    countOf: (shown, total) => `${shown} of ${total} sounds`,
    revision: (revision) => `map ${revision}`,
    show: (group) => `Show ${group}`,
    hide: (group) => `Hide ${group}`,
    matches: "Sounds that match",
    found: "Sounds Kumi found",
    indexed: (values) => `indexed ${values}`,
    indexedTitle: "Tempo, key and root note as Kumi indexed them at the folder's last scan: from the audio, or from the file's or folder's name. A rescan updates them.",
    note: (note) => `note ${note}`,
    closeness: (value) => `closeness ${value}, relative to this map`,
    closenessTitle: "How close this sound is to the one you asked about, 0 to 100, against the other sounds on this map: not a percentage, and not a confidence.",
    near: (name) => `Near “${name}”`,
    nearMany: (name, n) => `Near “${name}” and ${n} more`,
    more: (n) => `and ${n} more on the map`,
    play: "▶ Play",
    stop: "■ Stop",
    playTitle: "Play it here (nothing changes in your DAW)",
    use: "Use…",
    useTitle: "Put a request to use this sound in the chat, to finish and send",
    moreLike: "More like this",
    moreLikeTitle: "Put a request for sounds like this one in the chat, to send when you like",
    moreLikeThese: (n) => `More like these (${n})`,
    moreLikeTheseTitle: "Put a request for sounds like these in the chat, to send when you like",
    moreLikeTheseFirst: (most, n) => `Put a request for sounds like the first ${most} of your ${n} picks in the chat, to send when you like`,
    useOnPad: "Use on pad…",
    useOnPadTitle: "Pick a drum pad, an empty one included: the request goes in the chat, to send when you like",
    pads: "Drum pads",
    pad: (name, note) => name === String(note) ? `Pad for note ${note}` : `Pad ${name}, note ${note}`,
    playFailed: (error) => `Kumi couldn't play that sound: ${error}`,
    hidden: (n) => n === 1 ? "1 sound hidden: its file is missing or can't play here." : `${n} sounds hidden: their files are missing or can't play here.`,
    showHidden: "Show them",
    showHiddenTitle: "Put the hidden sounds back on the map",
    rescan: "Rescan…",
    rescanTitle: "Put a request to rescan your sample folders in the chat, to send when you like",
    tiles: "Tiles",
    tilesTitle: "Each sound as a square tile in its group's colour, near where the map put it",
    hand: "Move map",
    handTitle: "Drag to move the map instead of drawing a box. The middle button moves it too, and so does a trackpad's two-finger scroll."
  },
  measurements: {
    tab: "Measurements",
    nothing: "Nothing measured yet. When Kumi measures audio in your chat, its numbers show here as well.",
    tooMuch: "Kumi measured it, but there's too much to show here: the numbers are in its reply in the chat.",
    inReply: "Kumi measured it: the numbers are in its reply in the chat.",
    measured: (ago2) => `Measured ${ago2}`
  },
  script: {
    waiting: "Script waiting for your approval",
    deviceWaiting: "Device waiting for your approval",
    says: (purpose) => `Hermes says: ${purpose}`,
    python: "Python",
    lua: "Lua",
    device: "A Max for Live device (GenExpr)",
    controls: "The device’s controls",
    noControls: "No controls.",
    control: "Control",
    range: "Range",
    default: "Default",
    on: "on",
    off: "off",
    file: (file) => `New file: ${file}`,
    in: (language, daw) => `${language} in ${daw}`,
    lines: (count2) => count2 === 1 ? "1 line" : `${count2} lines`,
    sha: (short) => `SHA-256 ${short}…`,
    shaTitle: (sha) => `SHA-256 ${sha}: the hash of the code the approval names`,
    code: "The script’s code",
    deviceCode: "The device’s GenExpr code",
    codeTitle: "Every line of the code as it will run. Nothing runs from here.",
    reading: "Reading the code Kumi saved…",
    unreadable: "Kumi couldn't read the saved code, or it's no longer what the approval names: read it in the approval, or reject it.",
    unreadableSaved: "Kumi couldn't read the code this script ran: its saved copy is gone, or no longer matches the hash Kumi kept.",
    show: "Show code",
    hide: "Hide code",
    showTitle: "The code this script ran, as Kumi saved it when its approval asked"
  },
  listen: {
    measured: "measured",
    measuredTitle: "Measured on this computer by Kumi, from the audio file itself",
    captureTitle: "Kumi played this window and measured it; it isn't a change, so there's nothing to undo",
    waveform: (label) => `Waveform of ${label}`,
    play: (label) => `Play ${label}`,
    stop: (label) => `Stop ${label}`,
    audioGone: "The audio isn't available any more.",
    this: "This",
    reference: "Reference",
    difference: "Difference",
    bandsDifference: "Band balance, this minus reference (level-matched)",
    bandsLevel: "Band levels",
    measuredAudio: "Measured audio",
    loudness: "Loudness",
    truePeak: "True peak",
    brightness: "Brightness (centroid)",
    range: "Loudness range",
    crest: "Crest",
    tempo: "Tempo",
    key: "Key",
    peakToLoudness: "Peak to loudness",
    tilt: "Tilt",
    perOctave: (value) => `${value} dB/oct`,
    width: "Stereo width",
    lowEnd: "Low end",
    mono: "mono",
    notMono: "not mono",
    overlay: "Band levels, level-matched",
    overlayBand: (name, hz2, mine, reference, difference) => `${name} (${hz2} Hz): this ${mine} dB, reference ${reference} dB, ${difference} dB`,
    round: (number) => `Round ${number}`,
    roundTitle: "Each comparison with this reference since this window opened, oldest first: this minus reference",
    loudnessGap: (trend) => `loudness ${trend} LU`,
    tiltGap: (trend) => `tilt ${trend} dB/oct`,
    widthGap: (trend) => `width ${trend}`,
    interpretation: "Interpretation",
    noInterpretation: (why) => `No model interpretation: ${why}`,
    more: "More",
    less: "Less",
    beats: (from, to) => `Beats ${from}–${to}`,
    secondsMeasured: (seconds2) => `${seconds2} s measured`,
    through: (by) => `through ${by}`,
    ears: "Kumi Ears",
    offline: "an offline render",
    restored: "your project put back as it was",
    versus: (reference) => ` vs ${reference}`
  },
  settings: {
    intro: (daw) => `Kumi reads and changes your ${daw || "DAW"} project through typed changes you approve, keeps HISTORY with undo, measures audio on this computer, and maps the sample folders you allow.`,
    agentOff: "Kumi's Agent switch is off in this profile. Turn on the kumi plugin's Agent switch in Capabilities → Plugins and restart the gateway; its settings form appears under Agent settings.",
    setup: "Setup",
    state: "State",
    next: "Next",
    runtime: "Kumi runtime",
    installed: (version) => `Kumi ${version}`,
    notInstalled: "Not installed",
    needs: (version) => `needs ${version}`,
    download: "Download",
    unpacked: (size) => `(${size} unpacked)`,
    part: (daw) => `Kumi's part in ${daw}`,
    componentLine: (name, state) => `${name}: ${state}`,
    standalone: "Standalone Kumi",
    adopted: "adopted",
    leftAsIs: "left as it is",
    audioFormats: "Audio formats",
    connection: "Connection",
    now: "Now",
    chat: "Kumi's chat",
    chatHere: "this chat",
    chatElsewhere: "another chat",
    openMap: "Open the sound map",
    agentSettings: "Agent settings",
    thisWindow: "This window",
    openOnFind: "Open the sound map when Kumi finds sounds",
    petLines: "Let the pet say when Kumi finishes a change or a job",
    states: {
      ready: "Ready",
      needs_runtime: "Not set up yet",
      outdated_runtime: "Needs an update: run setup",
      damaged_runtime: "Needs reinstalling",
      needs_daw_component: "Needs Kumi's part in your DAW",
      no_pin: "This copy of the plugin can't set Kumi up",
      unsupported_platform: "Not available on this system",
      unknown: "Unknown"
    },
    platforms: {
      "aarch64-apple-darwin": "macOS (Apple silicon)",
      "x86_64-apple-darwin": "macOS (Intel)",
      "aarch64-pc-windows-msvc": "Windows (ARM64)",
      "x86_64-pc-windows-msvc": "Windows (x64)",
      "aarch64-unknown-linux-gnu": "Linux (ARM64)",
      "x86_64-unknown-linux-gnu": "Linux (x86-64)"
    },
    componentStates: {
      not_installed: "not installed",
      installed: "installed",
      outdated: "outdated",
      unknown: "unknown"
    },
    partInUse: "in use (Kumi is connected through it)"
  },
  generate: {
    generated: "generated",
    generatedTitle: "Made by Mirelo from your words. It isn't in your project until you place it.",
    soundTitle: "Generated sound",
    notesTitle: "Notes from audio",
    kinds: { video_to_sfx: "Sound for a video", extend: "Extended audio", inpaint: "Inpainted audio" },
    // CC-15: what a generation's billed seconds covered, never the returned file's length.
    window: {
      video_to_sfx: (first) => `Sound for video · first ${first}`,
      extend: (added, source) => `Extended by ${added} (source ${source})`,
      inpaint: (span2, source) => `Replaced ${span2} of ${source}`
    },
    credits: (n, shown) => n === 1 ? `${shown} credit` : `${shown} credits`,
    creditsTitle: "Mirelo API credits, not money. An estimate or a reservation is not a price.",
    cost: (credits2, estimate) => `${credits2} (est. ${estimate})`,
    reserved: (credits2) => `${credits2} reserved`,
    unknownCharge: (credits2) => `charge not known yet · ${credits2} reserved`,
    notSubmitted: "never sent to Mirelo · nothing charged",
    left: (left, cap) => `${left} of ${cap} credits left under your cap`,
    capTitle: "Kumi's own budget for paid generation, which approvals count against. It isn't the provider's bill.",
    frozen: "New paid generations are paused: a charge came in above its estimate",
    notes: (n, shown) => n === 1 ? `${shown} note` : `${shown} notes`,
    outputs: { midi: "MIDI file", notes: "Notes", musicxml: "MusicXML" },
    states: { running: "generating", failed: "failed", cancelled: "cancelled", interrupted: "interrupted" },
    place: "Place…",
    placeTitle: "Put a request to place this file in your project in the chat, to finish and send",
    planNotes: "Plan notes…",
    planNotesTitle: "Put a request in the chat to plan where these notes go, from your project's own timing, to finish and send"
  },
  pet: {
    undone: (change) => `Undone: ${change}`
  },
  palette: {
    map: "Kumi: open the sound map",
    undo: "Kumi: undo the last change",
    settings: "Kumi: settings"
  },
  // The sound palette under the map (not Desktop's command palette, above).
  dock: {
    title: "Palette",
    sounds: "Sounds in the palette",
    empty: "Nothing in your palette yet. Drag a box over sounds on the map (Shift adds another box) and press Add to palette.",
    none: "No palette yet: Add to palette makes one.",
    choose: "Choose a palette",
    switch: "Palette",
    newPalette: "New palette…",
    newName: "Name the new palette",
    rename: "Rename",
    renameTitle: "Rename this palette",
    forget: "Forget…",
    forgetTitle: (name) => `Forget ${name}: the palette goes, its sounds stay in your library`,
    forgetAsk: (name, n) => `Forget ${name} and the ${n === 1 ? "1 sound" : `${n} sounds`} in it? They stay in your library.`,
    forgetYes: "Forget",
    keep: "Keep",
    mode: "How Kumi uses the palette",
    strict: "Strict",
    strictTitle: "Kumi uses only these sounds when you ask for your palette",
    loose: "Loose",
    looseN: (k) => `Loose · ${k}`,
    looseTitle: (k) => `Kumi may also offer each sound's ${k} nearest, and asks before adding any`,
    count: (n) => n === 1 ? "1 sound" : `${n} sounds`,
    excluded: (n) => `${n} kept out`,
    more: (n) => `and ${n} more`,
    play: (name) => `Play ${name}`,
    stop: (name) => `Stop ${name}`,
    remove: (name) => `Take ${name} out of the palette`,
    offMap: "Not on this map: its file moved, or left your sample folders.",
    reorderHint: "Drag to move it, or Alt with ← or →",
    collapse: "Hide the palette",
    expand: "Show the palette",
    kit: "Use as kit…",
    kitTitle: "Put a request in the chat to load these sounds as a kit (a Drum Rack in Live), to send when you like",
    kitCopy: "Copy kit request",
    add: "Add to palette",
    addTitle: "Put this sound in your palette",
    addPicked: (n) => n === 1 ? "Add 1 to palette" : `Add ${n} to palette`,
    picked: (n) => n === 1 ? "1 picked" : `${n} picked`,
    clearPicks: (n) => `Clear (${n})`,
    clearPicksTitle: "Pick nothing (Esc, or a click on empty map, does it too)",
    cleared: "Nothing picked",
    hover: "Hover to play",
    hoverTitle: "Play each sound as the pointer comes onto it",
    firstOnly: (n) => `The first 10,000 of ${n} picked went in.`,
    pickAll: (n) => n === 1 ? "Pick it" : `Pick all ${n}`,
    pickAllTitle: "Pick every sound this search keeps",
    pickFoundTitle: "Pick every sound Kumi found",
    inPalette: "In palette",
    failed: (error) => `Kumi couldn't change the palette: ${error}`
  },
  // The Make panel: a new sound from the picks or the palette, by Kumi's sound design (cross, fuse, stack).
  make: {
    open: "Make…",
    openTitle: "Make a new sound from these: cross, fuse or stack them",
    title: "Make a sound",
    from: (n) => n === 1 ? "from 1 sound" : `from ${n} sounds`,
    close: "Close",
    sources: "The sounds it makes from",
    way: "How to make it",
    results: "Sounds made",
    none: "Sounds you make show here: play them, reroll them, and keep the ones you like.",
    ops: { cross: "Cross", fuse: "Fuse", stack: "Stack" },
    opHints: { cross: "one sound with the character of several", fuse: "something longer, made of them all", stack: "layered into one" },
    cards: {
      blend: { title: "Blend", hint: "One body morphed from them all" },
      attack: { title: "Attack", hint: "One's attack on the blend of the rest" },
      resonate: { title: "Resonate", hint: "Half ring through the other half" },
      modulate: { title: "Modulate", hint: "They modulate each other" },
      texture: { title: "Texture", hint: "A loop that moves between them" },
      loop: { title: "Loop", hint: "Their hits as a beat" },
      smear: { title: "Smear", hint: "A long granular stretch" },
      stack: { title: "Stack", hint: "Printed as one hit" }
    },
    options: { seconds: "Length", bpm: "Tempo", bars: "Bars", depth: "Depth", stretch: "Stretch", attack_from: "Attack from" },
    optionValues: {
      seconds: (value) => `Length ${value}`,
      bpm: (value) => `Tempo ${value} BPM`,
      bars: (n) => n === 1 ? "1 bar" : `${n} bars`,
      depth: (percent) => `Depth ${percent}%`,
      stretch: (times) => `Stretch ${times}×`
    },
    auto: "Auto",
    autoTitle: "Auto: each roll picks it. Move the slider to set it yourself.",
    make: "Make",
    makeTitle: (way) => `Make a sound with ${way}`,
    making: "Making…",
    needTwo: (n) => `Pick at least ${n} sounds to make one.`,
    moreSources: (n) => `+${n} more`,
    missing: "This Kumi for Hermes can't make sounds yet: update it, then try again.",
    failed: (error) => `Kumi couldn't make it: ${error}`,
    seedTitle: "Its seed: the same sounds, way and seed make the same sound again",
    variations: "Variations",
    previous: "Previous variation",
    next: "Next variation",
    variation: (i, n) => `${i} of ${n}`,
    play: "Play",
    stop: "Stop",
    reroll: "Reroll",
    rerolling: "Making…",
    rerollTitle: "Make another variation: the same sounds and way, a new roll",
    keep: "Keep",
    keepTitle: "Put it in your palette",
    kept: "In palette",
    use: "Use…",
    useTitle: "Put a request in the chat to use this sound on a pad or a track, to send when you like",
    copyUse: "Copy request"
  },
  notify: {
    nothingToUndo: "Kumi has no change it can undo right now.",
    undoUnsure: "Kumi isn't sure of its last change: ask Hermes to have it look at your project again first.",
    copied: "Copied the request: paste it into the chat to send it.",
    askInChat: (text) => `Ask in the chat: ${text}`,
    openChat: "Open a chat first, then try again.",
    notOpenHere: "Kumi's chat isn't open here, so the request is in your input, unsent. Send it from the chat Kumi is connected to."
  },
  dashboard: {
    agentOff: "Kumi's Agent switch is off in this profile.",
    agentOffHow: "Turn it on (the Plugins page, or `hermes plugins enable kumi`) and restart the dashboard to see Kumi here.",
    copyTitle: (text) => `Copy “${text}” to paste into a chat`,
    copied: "Copied",
    copyRequest: "Copy request",
    copyUndo: "Copy undo",
    mapEmpty: "No sound map yet: ask Hermes to add a sample folder and scan it.",
    askLook: "Ask Hermes in your chat to look again before another change."
  }
};
var ja = {
  common: {
    yourDaw: "DAW"
  },
  chip: {
    kumi: "Kumi",
    stopped: "Kumi は停止中",
    notAnswering: "応答なし",
    notConnected: (daw) => `Kumi · ${daw} 未接続`,
    label: (text) => `Kumi: ${text}`,
    openMap: "サンプルマップを開く",
    settings: "Kumi の設定"
  },
  step: {
    setup: (daw) => `${daw} 用に Kumi をセットアップ`,
    connect: (daw) => `${daw} に接続`
  },
  time: {
    now: "たった今",
    minutes: (n) => `${n} 分前`,
    hours: (n) => `${n} 時間前`,
    days: (n) => `${n} 日前`
  },
  pane: {
    title: "Kumi",
    agentOff: "このプロファイルでは Kumi の Agent スイッチがオフです。",
    agentOffHow: "「スキルとツール → プラグイン」で kumi プラグインの Agent スイッチをオンにし、ゲートウェイを再起動してください。オンになると、このペインに DAW の FOCUS と Kumi の HISTORY が表示されます。",
    reading: "Kumi の状態を読み込み中…",
    backendError: (error) => `Kumi のペインに Hermes が応答しませんでした: ${error}`,
    unsure: (op, reason) => `${op}のあと、プロジェクトが元のままか Kumi は確信できません${reason ? `（${reason}）` : ""}。`,
    unsureOp: "直前の操作",
    needsLook: "次の変更の前に、もう一度確認が必要です。",
    lookAgain: "Hermes にもう一度確認してもらう",
    focus: "FOCUS",
    jobs: "ジョブ",
    history: "HISTORY",
    undoable: (n) => `· ${n} 件取り消せます`,
    undo: "取り消す",
    undoTitle: "この変更の取り消しを Hermes に頼む",
    undoChangeTitle: (n) => `この変更の ${n} ステップすべての取り消しを、1 回の承認で Hermes に頼む`,
    undoLabel: (change) => `${change} を取り消す`,
    openMap: "サンプルマップを開く",
    undoNeedsConnection: "取り消すには Kumi の接続が必要です。"
  },
  hint: {
    sees: "Kumi が見ているもの:",
    lastSaw: "Kumi が最後に見たもの:",
    title: "Kumi が DAW で選択中と見ているもの。このチャットで「このトラック」などと言うと、これを指します。",
    titleUnsure: "Kumi が最後に DAW で選択中と見たもの。プロジェクトが元のままか確信できないので、もう一度確認するよう頼んでください。"
  },
  focus: {
    nothing: "まだ何もフォーカスされていません。接続すると、DAW で選択したものを Kumi が表示します。",
    nothingSelected: "何も選択されていません",
    project: "開いているプロジェクト",
    playing: "▶ 再生中",
    stopped: "■ 停止中",
    busy: "この間は DAW の応答が遅れるため、表示が実際より遅れることがあります",
    kumiStopped: "Kumi は停止中",
    notAnswering: "応答なし",
    since: "Kumi が前回見てから",
    more: (n) => `ほか ${n} 件`,
    lastHeard: (ago2) => `最終応答: ${ago2}。`
  },
  history: {
    empty: "まだ変更はありません。Kumi が DAW に加えた変更は、取り消しとともにここに表示されます。",
    applied: "適用済み",
    undone: "取り消し済み",
    kept: "維持（その後あなたが変更しました）",
    expired: "取り消せません: Kumi が再起動しました",
    partial: "一部のみ適用",
    unsure: "不確か",
    refused: "拒否",
    failed: "失敗",
    recorded: "記録",
    unconfirmed: "何が起きたか Kumi は確認できませんでした。",
    lookFirst: "次の変更の前に、Hermes にもう一度確認してもらってください。",
    restored: "その間に変えたものは Kumi が元に戻しました。",
    restartedSince: (daw) => `この変更のあと Kumi が再起動したため、安全に取り消せません。${daw} でまだ最後の操作なら ${daw} で取り消すか、Hermes に元に戻すよう頼んでください。`,
    restoreUnsure: "すべて元に戻せたか Kumi は確信できません。",
    dawUndo: (daw) => `${daw} の取り消しで次の操作（その後 ${daw} で何も変えていなければ）`,
    dawUndoTitle: (daw, keys) => `${daw} の取り消しで、この上に Kumi の後の変更はありません。その後 ${daw} で何も変えていなければ、この行の説明のとおり、すぐに ${keys} を押してください。`,
    byHand: (daw) => `${daw} で手動で削除するか、後の変更を先に取り消してください。`,
    byHandTitle: (daw) => `${daw} の取り消しは、その後の変更を先に戻してしまいます。手動で削除するか、後の変更を先に取り消してください。`,
    scriptUndo: (daw) => `Kumi はスクリプトを取り消せません。${daw} でまだ最後の操作なら、${daw} の取り消しで戻せることがあります。`,
    copyKept: (file) => `この前に、Kumi は最後に保存されたプロジェクトのコピーを隣に残しました: ${file}`
  },
  job: {
    scanning: "スキャン中",
    generating: "生成中",
    generation: "生成",
    scan: "スキャン",
    job: "ジョブ",
    running: (kind) => `${kind} 実行中`,
    of: (done, total) => `${done} / ${total}`,
    done: (noun) => `${noun}完了`,
    sounds: (_n, shown) => `${shown} 個のサウンド`,
    unreadable: (shown) => `${shown} 個は読み込めませんでした`,
    failed: (noun) => `${noun}失敗`,
    stopped: (noun) => `${noun}停止`,
    interrupted: (noun) => `${noun}中断`,
    interruptedWhy: "完了する前に Kumi が停止しました。",
    underWay: (title) => `${title}: 進行中`,
    percent: (title, percent) => `${title}: ${percent}%`
  },
  map: {
    tab: "サンプルマップ",
    canvas: "サンプルマップ",
    roledescription: "サンプルマップ",
    keys: "矢印キーでその方向の一番近いサウンドへ移動します。Shift と左右キーで全サウンドを順に移動、Shift と上下キーでズーム、Home でマップ全体を表示します。Tab で選んだサウンドの再生と使用に移れます。",
    agentOff: "このプロファイルでは Kumi の Agent スイッチがオフのため、表示できるマップがありません。",
    reading: "サンプルマップを読み込み中…",
    empty: "まだサンプルマップがありません。Hermes にサンプルフォルダの追加とスキャンを頼んでください。見つかったサウンドからマップが作られます。",
    search: "サウンドを検索",
    searchHint: "マップを検索: 名前・種類・フォルダ",
    count: (n) => `${n} 個のサウンド`,
    countOf: (shown, total) => `${total} 個中 ${shown} 個のサウンド`,
    revision: (revision) => `マップ ${revision}`,
    show: (group) => `${group} を表示`,
    hide: (group) => `${group} を隠す`,
    matches: "一致するサウンド",
    found: "Kumi が見つけたサウンド",
    indexed: (values) => `インデックス値 ${values}`,
    indexedTitle: "フォルダを最後にスキャンしたときに Kumi がインデックスしたテンポ、キー、ルート音です。オーディオそのもの、またはファイル名やフォルダ名から得ています。再スキャンで更新されます。",
    note: (note) => `ルート ${note}`,
    closeness: (value) => `近さ ${value}（このマップ内での相対値）`,
    closenessTitle: "尋ねたサウンドにどれだけ近いかを、このマップ上のほかのサウンドと比べて 0〜100 で表します。パーセントでも確信度でもありません。",
    near: (name) => `「${name}」に近いサウンド`,
    nearMany: (name, n) => `「${name}」ほか ${n} 個に近いサウンド`,
    more: (n) => `ほか ${n} 個はマップ上に`,
    play: "▶ 再生",
    stop: "■ 停止",
    playTitle: "ここで再生（DAW は何も変わりません）",
    use: "使う…",
    useTitle: "このサウンドを使う依頼をチャットに入れます。仕上げて送信してください",
    moreLike: "似たサウンド",
    moreLikeTitle: "これに似たサウンドを探す依頼をチャットに入れます。送信はご自身で",
    moreLikeThese: (n) => `これらに似たサウンド（${n}）`,
    moreLikeTheseTitle: "これらに似たサウンドを探す依頼をチャットに入れます。送信はご自身で",
    moreLikeTheseFirst: (most, n) => `選んだ ${n} 個のうち最初の ${most} 個に似たサウンドを探す依頼をチャットに入れます。送信はご自身で`,
    useOnPad: "パッドに使う…",
    useOnPadTitle: "ドラムパッドを選びます（空のパッドも可）。依頼はチャットに入り、送信はご自身で",
    pads: "ドラムパッド",
    pad: (name, note) => name === String(note) ? `ノート ${note} のパッド` : `パッド ${name}（ノート ${note}）`,
    playFailed: (error) => `Kumi はそのサウンドを再生できませんでした: ${error}`,
    hidden: (n) => `${n} 個のサウンドを隠しました。ファイルが見つからないか、ここでは再生できません。`,
    showHidden: "表示する",
    showHiddenTitle: "隠したサウンドをマップに戻します",
    rescan: "再スキャン…",
    rescanTitle: "サンプルフォルダを再スキャンする依頼をチャットに入れます。送信はご自身で",
    tiles: "タイル",
    tilesTitle: "各サウンドを、マップ上の位置の近くに、グループの色の四角いタイルとして表示します",
    hand: "マップを移動",
    handTitle: "ドラッグで囲む代わりに、ドラッグでマップを動かします。中ボタンのドラッグや、トラックパッドの2本指スクロールでも動かせます。"
  },
  measurements: {
    tab: "測定結果",
    nothing: "まだ測定はありません。Kumi がチャットでオーディオを測定すると、その数値がここにも表示されます。",
    tooMuch: "Kumi は測定しましたが、ここに表示するには多すぎます。数値はチャットの返答にあります。",
    inReply: "Kumi は測定しました。数値はチャットの返答にあります。",
    measured: (ago2) => `測定: ${ago2}`
  },
  script: {
    waiting: "承認待ちのスクリプト",
    deviceWaiting: "承認待ちのデバイス",
    says: (purpose) => `Hermes の説明: ${purpose}`,
    python: "Python",
    lua: "Lua",
    device: "Max for Live デバイス（GenExpr）",
    controls: "デバイスのコントロール",
    noControls: "コントロールはありません。",
    control: "コントロール",
    range: "範囲",
    default: "初期値",
    on: "オン",
    off: "オフ",
    file: (file) => `新しいファイル: ${file}`,
    in: (language, daw) => `${daw} の ${language}`,
    lines: (count2) => `${count2} 行`,
    sha: (short) => `SHA-256 ${short}…`,
    shaTitle: (sha) => `SHA-256 ${sha}（承認が示すコードのハッシュ）`,
    code: "スクリプトのコード",
    deviceCode: "デバイスの GenExpr コード",
    codeTitle: "実行されるとおりのコードの全行です。ここからは何も実行されません。",
    reading: "Kumi が保存したコードを読み込み中…",
    unreadable: "保存されたコードを読めないか、承認が示すコードではなくなっています。承認で確認するか、拒否してください。",
    unreadableSaved: "このスクリプトが実行したコードを読めませんでした。保存したコピーがないか、Kumi が控えたハッシュと一致しません。",
    show: "コードを表示",
    hide: "コードを隠す",
    showTitle: "このスクリプトが実行したコード（承認を求めたときに Kumi が保存したもの）"
  },
  listen: {
    measured: "測定値",
    measuredTitle: "Kumi がこのコンピュータ上で、オーディオファイルそのものから測定しました",
    captureTitle: "Kumi はこの区間を再生して測定しました。変更ではないので、取り消すものはありません",
    waveform: (label) => `${label} の波形`,
    play: (label) => `${label} を再生`,
    stop: (label) => `${label} を停止`,
    audioGone: "このオーディオはもう利用できません。",
    this: "これ",
    reference: "リファレンス",
    difference: "差",
    bandsDifference: "帯域バランス（これ − リファレンス、レベル合わせ済み）",
    bandsLevel: "帯域レベル",
    measuredAudio: "測定したオーディオ",
    loudness: "ラウドネス",
    truePeak: "トゥルーピーク",
    brightness: "明るさ（セントロイド）",
    range: "ラウドネスレンジ",
    crest: "クレスト",
    tempo: "テンポ",
    key: "キー",
    peakToLoudness: "ピーク対ラウドネス",
    tilt: "ティルト",
    perOctave: (value) => `${value} dB/oct`,
    width: "ステレオ幅",
    lowEnd: "低域",
    mono: "モノ",
    notMono: "モノではない",
    overlay: "帯域レベル（レベル合わせ済み）",
    overlayBand: (name, hz2, mine, reference, difference) => `${name}（${hz2} Hz）: これ ${mine} dB、リファレンス ${reference} dB、差 ${difference} dB`,
    round: (number) => `ラウンド ${number}`,
    roundTitle: "このウィンドウを開いてからの、このリファレンスとの各比較（古い順、これ − リファレンス）",
    loudnessGap: (trend) => `ラウドネス ${trend} LU`,
    tiltGap: (trend) => `ティルト ${trend} dB/oct`,
    widthGap: (trend) => `幅 ${trend}`,
    interpretation: "解釈",
    noInterpretation: (why) => `モデルによる解釈なし: ${why}`,
    more: "もっと見る",
    less: "閉じる",
    beats: (from, to) => `${from}–${to} 拍`,
    secondsMeasured: (seconds2) => `${seconds2} 秒を測定`,
    through: (by) => /[A-Za-z]$/.test(by) ? `${by} 経由` : `${by}経由`,
    ears: "Kumi Ears",
    offline: "オフラインレンダリング",
    restored: "プロジェクトは元に戻しました",
    versus: (reference) => `（${reference} と比較）`
  },
  settings: {
    intro: (daw) => `Kumi は、あなたが承認した型付きの変更で ${daw || "DAW"} のプロジェクトを読み書きし、HISTORY と取り消しを保ち、このコンピュータ上でオーディオを測定し、許可したサンプルフォルダをマップにします。`,
    agentOff: "このプロファイルでは Kumi の Agent スイッチがオフです。「スキルとツール → プラグイン」で kumi プラグインの Agent スイッチをオンにし、ゲートウェイを再起動してください。設定フォームは「エージェント設定」に表示されます。",
    setup: "セットアップ",
    state: "状態",
    next: "次にすること",
    runtime: "Kumi ランタイム",
    installed: (version) => `Kumi ${version}`,
    notInstalled: "未インストール",
    needs: (version) => `${version} が必要`,
    download: "ダウンロード",
    unpacked: (size) => `（展開後 ${size}）`,
    part: (daw) => `${daw} 用の Kumi の部品`,
    componentLine: (name, state) => `${name}：${state}`,
    standalone: "単体の Kumi",
    adopted: "引き継ぎ済み",
    leftAsIs: "そのまま",
    audioFormats: "オーディオ形式",
    connection: "接続",
    now: "現在",
    chat: "Kumi のチャット",
    chatHere: "このチャット",
    chatElsewhere: "別のチャット",
    openMap: "サンプルマップを開く",
    agentSettings: "エージェント設定",
    thisWindow: "このウィンドウ",
    openOnFind: "Kumi がサウンドを見つけたらサンプルマップを開く",
    petLines: "Kumi が変更やジョブを終えたら、ペットの吹き出しで知らせる",
    states: {
      ready: "準備完了",
      needs_runtime: "まだセットアップされていません",
      outdated_runtime: "更新が必要です: セットアップを実行してください",
      damaged_runtime: "再インストールが必要です",
      needs_daw_component: "DAW 用の Kumi の部品が必要です",
      no_pin: "このプラグインのコピーでは Kumi をセットアップできません",
      unsupported_platform: "このシステムでは利用できません",
      unknown: "不明"
    },
    platforms: {
      "aarch64-apple-darwin": "macOS（Apple シリコン）",
      "x86_64-apple-darwin": "macOS（Intel）",
      "aarch64-pc-windows-msvc": "Windows（ARM64）",
      "x86_64-pc-windows-msvc": "Windows（x64）",
      "aarch64-unknown-linux-gnu": "Linux（ARM64）",
      "x86_64-unknown-linux-gnu": "Linux（x86-64）"
    },
    componentStates: {
      not_installed: "未インストール",
      installed: "インストール済み",
      outdated: "古い版",
      unknown: "不明"
    },
    partInUse: "使用中（Kumi はこれを通じて接続しています）"
  },
  generate: {
    generated: "生成",
    generatedTitle: "Mirelo があなたの言葉から作りました。配置するまでプロジェクトには入りません。",
    soundTitle: "生成したサウンド",
    notesTitle: "オーディオから取り出したノート",
    kinds: { video_to_sfx: "映像の効果音", extend: "延長したオーディオ", inpaint: "一部を差し替えたオーディオ" },
    window: {
      video_to_sfx: (first) => `映像の効果音 · 最初の ${first}`,
      extend: (added, source) => `${added} 延長（元は ${source}）`,
      inpaint: (span2, source) => `${source} のうち ${span2} を差し替え`
    },
    credits: (_n, shown) => `${shown} クレジット`,
    creditsTitle: "Mirelo API のクレジットで、お金ではありません。見積や確保額は価格ではありません。",
    cost: (credits2, estimate) => `${credits2}（見積 ${estimate}）`,
    reserved: (credits2) => `${credits2} を確保`,
    unknownCharge: (credits2) => `請求額はまだ不明 · ${credits2} を確保`,
    notSubmitted: "Mirelo には送信されませんでした · 請求なし",
    left: (left, cap) => `上限 ${cap} クレジット中 残り ${left}`,
    capTitle: "有料生成のための Kumi 自身の予算で、承認ごとに差し引かれます。提供元の請求額ではありません。",
    frozen: "見積を超える請求があったため、新しい有料生成は止めています",
    notes: (_n, shown) => `${shown} ノート`,
    outputs: { midi: "MIDI ファイル", notes: "ノート", musicxml: "MusicXML" },
    states: { running: "生成中", failed: "失敗", cancelled: "キャンセル済み", interrupted: "中断" },
    place: "配置…",
    placeTitle: "このファイルをプロジェクトに配置する依頼をチャットに入れます。仕上げて送信してください",
    planNotes: "ノートの配置を計画…",
    planNotesTitle: "プロジェクト自身のタイミングから、これらのノートをどこに置くか計画する依頼をチャットに入れます。仕上げて送信してください"
  },
  pet: {
    undone: (change) => `取り消しました: ${change}`
  },
  palette: {
    map: "Kumi: サンプルマップを開く",
    undo: "Kumi: 最後の変更を取り消す",
    settings: "Kumi: 設定"
  },
  dock: {
    title: "パレット",
    sounds: "パレットのサウンド",
    empty: "パレットはまだ空です。マップ上でサウンドをドラッグで囲み（Shift で囲みを追加）、「パレットに追加」を押してください。",
    none: "パレットはまだありません。「パレットに追加」で作られます。",
    choose: "パレットを選ぶ",
    switch: "パレット",
    newPalette: "新しいパレット…",
    newName: "新しいパレットの名前",
    rename: "名前を変更",
    renameTitle: "このパレットの名前を変更",
    forget: "削除…",
    forgetTitle: (name) => `${name} を削除します。パレットはなくなりますが、サウンドはライブラリに残ります`,
    forgetAsk: (name, n) => `${name} と、その中の ${n} 個のサウンドを削除しますか？サウンドはライブラリに残ります。`,
    forgetYes: "削除",
    keep: "残す",
    mode: "Kumi のパレットの使い方",
    strict: "厳密",
    strictTitle: "パレットを頼まれたとき、Kumi はこのサウンドだけを使います",
    loose: "ゆるやか",
    looseN: (k) => `ゆるやか · ${k}`,
    looseTitle: (k) => `Kumi は各サウンドに近い ${k} 個も提案することがあり、追加する前に確認します`,
    count: (n) => `${n} 個のサウンド`,
    excluded: (n) => `${n} 個を除外`,
    more: (n) => `ほか ${n} 個`,
    play: (name) => `${name} を再生`,
    stop: (name) => `${name} を停止`,
    remove: (name) => `${name} をパレットから外す`,
    offMap: "このマップにはありません。ファイルが移動したか、サンプルフォルダの外に出ました。",
    reorderHint: "ドラッグ、または Alt と ← / → で移動",
    collapse: "パレットを隠す",
    expand: "パレットを表示",
    kit: "キットとして使う…",
    kitTitle: "これらのサウンドをキット（Live ではドラムラック）として読み込む依頼をチャットに入れます。送信はご自身で",
    kitCopy: "キットの依頼をコピー",
    add: "パレットに追加",
    addTitle: "このサウンドをパレットに入れます",
    addPicked: (n) => `${n} 個をパレットに追加`,
    picked: (n) => `${n} 個を選択中`,
    clearPicks: (n) => `選択を解除（${n}）`,
    clearPicksTitle: "何も選んでいない状態にします（Esc や、マップの空いた所のクリックでも）",
    cleared: "選択を解除しました",
    hover: "ホバーで再生",
    hoverTitle: "ポインタが乗ったサウンドをすぐに再生します",
    firstOnly: (n) => `選んだ ${n} 個のうち、最初の 10,000 個を入れました。`,
    pickAll: (n) => `${n} 個をすべて選ぶ`,
    pickAllTitle: "この検索で残ったサウンドをすべて選びます",
    pickFoundTitle: "Kumi が見つけたサウンドをすべて選びます",
    inPalette: "パレットにあり",
    failed: (error) => `Kumi はパレットを変更できませんでした: ${error}`
  },
  make: {
    open: "作る…",
    openTitle: "これらから新しいサウンドを作ります（クロス、フューズ、スタック）",
    title: "サウンドを作る",
    from: (n) => `${n} 個のサウンドから`,
    close: "閉じる",
    sources: "元にするサウンド",
    way: "作り方",
    results: "作ったサウンド",
    none: "作ったサウンドはここに並びます。再生し、リロールし、気に入ったものはキープしてパレットに入れられます。",
    ops: { cross: "クロス", fuse: "フューズ", stack: "スタック" },
    opHints: { cross: "複数のキャラクターを持つ 1 つの音", fuse: "すべてから作る長めの音", stack: "重ねて 1 つに" },
    cards: {
      blend: { title: "ブレンド", hint: "すべてを混ぜ合わせた 1 つのボディ" },
      attack: { title: "アタック", hint: "1 つのアタックを残りのブレンドに" },
      resonate: { title: "レゾネート", hint: "半分がもう半分を通して鳴る" },
      modulate: { title: "モジュレート", hint: "互いに変調し合う" },
      texture: { title: "テクスチャ", hint: "それらの間を動くループ" },
      loop: { title: "ループ", hint: "それらのヒットでビートを" },
      smear: { title: "スミア", hint: "長いグラニュラー・ストレッチ" },
      stack: { title: "スタック", hint: "1 つのヒットに書き出す" }
    },
    options: { seconds: "長さ", bpm: "テンポ", bars: "小節", depth: "深さ", stretch: "ストレッチ", attack_from: "アタックの元" },
    optionValues: {
      seconds: (value) => `長さ ${value}`,
      bpm: (value) => `テンポ ${value} BPM`,
      bars: (n) => `${n} 小節`,
      depth: (percent) => `深さ ${percent}%`,
      stretch: (times) => `ストレッチ ${times}×`
    },
    auto: "自動",
    autoTitle: "自動：作るたびに決まります。スライダーを動かすと自分で決められます。",
    make: "作る",
    makeTitle: (way) => `${way}でサウンドを作ります`,
    making: "作成中…",
    needTwo: (n) => `作るには ${n} 個以上のサウンドを選んでください。`,
    moreSources: (n) => `ほか ${n} 個`,
    missing: "この Kumi for Hermes はまだサウンドを作れません。更新してから、もう一度お試しください。",
    failed: (error) => `作れませんでした：${error}`,
    seedTitle: "シード：同じサウンド、同じ作り方、同じシードで同じ音がもう一度できます",
    variations: "バリエーション",
    previous: "前のバリエーション",
    next: "次のバリエーション",
    variation: (i, n) => `${i} / ${n}`,
    play: "再生",
    stop: "停止",
    reroll: "リロール",
    rerolling: "作成中…",
    rerollTitle: "別のバリエーションを作ります（同じサウンドと作り方で、新しいロール）",
    keep: "キープ",
    keepTitle: "パレットに入れます",
    kept: "パレットにあります",
    use: "使う…",
    useTitle: "このサウンドをパッドやトラックで使う依頼をチャットに入れます（送るのはあなたです）",
    copyUse: "依頼をコピー"
  },
  notify: {
    nothingToUndo: "今すぐ取り消せる変更はありません。",
    undoUnsure: "Kumi は直前の変更に確信がありません。まずプロジェクトをもう一度確認するよう Hermes に頼んでください。",
    copied: "依頼をコピーしました。チャットに貼り付けて送信してください。",
    askInChat: (text) => `チャットで頼んでください: ${text}`,
    openChat: "先にチャットを開いてから、もう一度試してください。",
    notOpenHere: "Kumi のチャットがここで開いていないため、依頼は未送信のまま入力欄にあります。Kumi が接続しているチャットから送信してください。"
  },
  dashboard: {
    agentOff: "このプロファイルでは Kumi の Agent スイッチがオフです。",
    agentOffHow: "オンにして（プラグインのページ、または `hermes plugins enable kumi`）、ダッシュボードを再起動すると、ここに Kumi が表示されます。",
    copyTitle: (text) => `「${text}」をコピーしてチャットに貼り付け`,
    copied: "コピーしました",
    copyRequest: "依頼をコピー",
    copyUndo: "取り消しをコピー",
    mapEmpty: "まだサンプルマップがありません。Hermes にサンプルフォルダの追加とスキャンを頼んでください。",
    askLook: "次の変更の前に、チャットで Hermes にもう一度確認するよう頼んでください。"
  }
};
var zh = {
  common: {
    yourDaw: "DAW"
  },
  chip: {
    kumi: "Kumi",
    stopped: "Kumi 已停止",
    notAnswering: "无响应",
    notConnected: (daw) => `Kumi · ${daw} 未连接`,
    label: (text) => `Kumi：${text}`,
    openMap: "打开采样地图",
    settings: "Kumi 设置"
  },
  step: {
    setup: (daw) => `为 ${daw} 设置 Kumi`,
    connect: (daw) => `连接到 ${daw}`
  },
  time: {
    now: "刚刚",
    minutes: (n) => `${n} 分钟前`,
    hours: (n) => `${n} 小时前`,
    days: (n) => `${n} 天前`
  },
  pane: {
    title: "Kumi",
    agentOff: "此配置中 Kumi 的 Agent 开关已关闭。",
    agentOffHow: "请在“技能与工具 → 插件”中打开 kumi 插件的 Agent 开关，然后重启网关。打开后，此面板会显示 DAW 的 FOCUS 和 Kumi 的 HISTORY。",
    reading: "正在读取 Kumi 的状态…",
    backendError: (error) => `Hermes 没有响应 Kumi 的面板：${error}`,
    unsure: (op, reason) => `在${op}之后，Kumi 不确定你的项目是否保持原样${reason ? `：${reason}` : ""}。`,
    unsureOp: "上一个操作",
    needsLook: "下一次改动之前，需要重新查看一次。",
    lookAgain: "让 Hermes 重新查看",
    focus: "FOCUS",
    jobs: "任务",
    history: "HISTORY",
    undoable: (n) => `· ${n} 项可撤销`,
    undo: "撤销",
    undoTitle: "让 Hermes 撤销这次改动",
    undoChangeTitle: (n) => `让 Hermes 用一次批准撤销这次改动的全部 ${n} 步`,
    undoLabel: (change) => `撤销 ${change}`,
    openMap: "打开采样地图",
    undoNeedsConnection: "撤销需要 Kumi 已连接。"
  },
  hint: {
    sees: "Kumi 看到：",
    lastSaw: "Kumi 上次看到：",
    title: "Kumi 在你的 DAW 中看到的选中内容：在这个对话里说“这条音轨”等，指的就是它。",
    titleUnsure: "Kumi 上次在你的 DAW 中看到的选中内容。它不确定项目是否保持原样：请让它重新查看。"
  },
  focus: {
    nothing: "还没有焦点。连接后，Kumi 会显示你在 DAW 中选中的内容。",
    nothingSelected: "未选中任何内容",
    project: "当前打开的项目",
    playing: "▶ 播放中",
    stopped: "■ 已停止",
    busy: "在此期间 DAW 响应较慢，显示的内容可能滞后",
    kumiStopped: "Kumi 已停止",
    notAnswering: "无响应",
    since: "自 Kumi 上次查看以来",
    more: (n) => `还有 ${n} 项`,
    lastHeard: (ago2) => `最后响应：${ago2}。`
  },
  history: {
    empty: "还没有改动。Kumi 在你的 DAW 中做的每次改动都会显示在这里，并可撤销。",
    applied: "已应用",
    undone: "已撤销",
    kept: "已保留（之后你改过它）",
    expired: "无法撤销：Kumi 已重启",
    partial: "部分应用",
    unsure: "不确定",
    refused: "已拒绝",
    failed: "失败",
    recorded: "已记录",
    unconfirmed: "Kumi 无法确认发生了什么。",
    lookFirst: "下一次改动之前，请让 Hermes 重新查看。",
    restored: "Kumi 已还原期间做出的改动。",
    restartedSince: (daw) => `此改动之后 Kumi 已重启，无法安全撤销：如果它仍是 ${daw} 中的最后一步，请在 ${daw} 中撤销，或让 Hermes 把它改回去。`,
    restoreUnsure: "Kumi 不确定是否已全部还原。",
    dawUndo: (daw) => `${daw} 撤销中的下一步，前提是之后那里没有任何改动`,
    dawUndoTitle: (daw, keys) => `在 ${daw} 的撤销中，它上面没有 Kumi 之后的改动。如果之后在 ${daw} 中没有任何改动，请按这一行的说明立即按 ${keys}。`,
    byHand: (daw) => `在 ${daw} 中手动删除，或先撤销之后的改动。`,
    byHandTitle: (daw) => `${daw} 的撤销会先撤回之后的改动。请手动删除，或先撤销之后的改动。`,
    scriptUndo: (daw) => `Kumi 无法撤销脚本：如果它仍是 ${daw} 中的最后一步，${daw} 自己的撤销也许可以。`,
    copyKept: (file) => `在此之前，Kumi 在它旁边保留了一份最后保存的项目副本：${file}`
  },
  job: {
    scanning: "正在扫描",
    generating: "正在生成",
    generation: "生成",
    scan: "扫描",
    job: "任务",
    running: (kind) => `${kind} 运行中`,
    of: (done, total) => `${done} / ${total}`,
    done: (noun) => `${noun}完成`,
    sounds: (_n, shown) => `${shown} 个声音`,
    unreadable: (shown) => `${shown} 个无法读取`,
    failed: (noun) => `${noun}失败`,
    stopped: (noun) => `${noun}已停止`,
    interrupted: (noun) => `${noun}中断`,
    interruptedWhy: "Kumi 在完成前停止了。",
    underWay: (title) => `${title}：进行中`,
    percent: (title, percent) => `${title}：${percent}%`
  },
  map: {
    tab: "采样地图",
    canvas: "采样地图",
    roledescription: "采样地图",
    keys: "方向键移到该方向最近的声音。Shift 加左右键按顺序经过每个声音，Shift 加上下键缩放，Home 显示整张地图。按 Tab 可到所选声音的播放和使用。",
    agentOff: "此配置中 Kumi 的 Agent 开关已关闭，没有可显示的地图。",
    reading: "正在读取采样地图…",
    empty: "还没有采样地图。请让 Hermes 添加一个采样文件夹并扫描；地图会根据找到的声音生成。",
    search: "搜索声音",
    searchHint: "搜索地图：名称、种类或文件夹",
    count: (n) => `${n} 个声音`,
    countOf: (shown, total) => `${total} 个声音中的 ${shown} 个`,
    revision: (revision) => `地图 ${revision}`,
    show: (group) => `显示 ${group}`,
    hide: (group) => `隐藏 ${group}`,
    matches: "匹配的声音",
    found: "Kumi 找到的声音",
    indexed: (values) => `索引值 ${values}`,
    indexedTitle: "Kumi 在上次扫描该文件夹时建立索引的速度、调性和根音：来自音频本身，或来自文件名或文件夹名。重新扫描会更新它们。",
    note: (note) => `根音 ${note}`,
    closeness: (value) => `接近度 ${value}（相对于这张地图）`,
    closenessTitle: "与你所问的声音有多接近，0 到 100，相对于这张地图上的其他声音：不是百分比，也不是置信度。",
    near: (name) => `与“${name}”相近的声音`,
    nearMany: (name, n) => `与“${name}”及另外 ${n} 个相近的声音`,
    more: (n) => `地图上还有 ${n} 个`,
    play: "▶ 播放",
    stop: "■ 停止",
    playTitle: "在这里播放（DAW 不会有任何改动）",
    use: "使用…",
    useTitle: "把使用这个声音的请求放进对话，补充完整后发送",
    moreLike: "更多类似的",
    moreLikeTitle: "把寻找类似声音的请求放进对话，由你决定何时发送",
    moreLikeThese: (n) => `更多类似这些的（${n}）`,
    moreLikeTheseTitle: "把寻找与这些声音类似的声音的请求放进对话，由你决定何时发送",
    moreLikeTheseFirst: (most, n) => `把寻找与你所选 ${n} 个中前 ${most} 个类似的声音的请求放进对话，由你决定何时发送`,
    useOnPad: "用在打击垫上…",
    useOnPadTitle: "选择一个打击垫（空的也可以）：请求放进对话，由你决定何时发送",
    pads: "打击垫",
    pad: (name, note) => name === String(note) ? `音符 ${note} 的打击垫` : `打击垫 ${name}（音符 ${note}）`,
    playFailed: (error) => `Kumi 无法播放这个声音：${error}`,
    hidden: (n) => `已隐藏 ${n} 个声音：文件不见了，或无法在这里播放。`,
    showHidden: "显示它们",
    showHiddenTitle: "把隐藏的声音放回地图",
    rescan: "重新扫描…",
    rescanTitle: "把重新扫描采样文件夹的请求放进对话，由你决定何时发送",
    tiles: "方块",
    tilesTitle: "把每个声音显示为它所在分组颜色的方块，放在地图上它原来位置的附近",
    hand: "移动地图",
    handTitle: "拖动来移动地图，而不是拉框选择。用中键拖动或触控板双指滚动也能移动地图。"
  },
  measurements: {
    tab: "测量结果",
    nothing: "还没有测量。Kumi 在对话中测量音频时，数值也会显示在这里。",
    tooMuch: "Kumi 已完成测量，但内容太多无法在此显示：数值在它在对话中的回复里。",
    inReply: "Kumi 已完成测量：数值在它在对话中的回复里。",
    measured: (ago2) => `测量于 ${ago2}`
  },
  script: {
    waiting: "等待你批准的脚本",
    deviceWaiting: "等待你批准的设备",
    says: (purpose) => `Hermes 说：${purpose}`,
    python: "Python",
    lua: "Lua",
    device: "Max for Live 设备（GenExpr）",
    controls: "设备的控件",
    noControls: "没有控件。",
    control: "控件",
    range: "范围",
    default: "默认值",
    on: "开",
    off: "关",
    file: (file) => `新文件：${file}`,
    in: (language, daw) => `${daw} 中的 ${language}`,
    lines: (count2) => `${count2} 行`,
    sha: (short) => `SHA-256 ${short}…`,
    shaTitle: (sha) => `SHA-256 ${sha}：批准所指代码的哈希`,
    code: "脚本代码",
    deviceCode: "设备的 GenExpr 代码",
    codeTitle: "将要运行的完整代码。这里不会运行任何东西。",
    reading: "正在读取 Kumi 保存的代码…",
    unreadable: "Kumi 无法读取保存的代码，或它已不是批准所指的代码：请在批准中查看，或拒绝它。",
    unreadableSaved: "无法读取这个脚本运行的代码：保存的副本已不存在，或与 Kumi 记下的哈希不再一致。",
    show: "显示代码",
    hide: "隐藏代码",
    showTitle: "这个脚本运行的代码（在请求批准时由 Kumi 保存）"
  },
  listen: {
    measured: "实测",
    measuredTitle: "由 Kumi 在这台电脑上根据音频文件本身测量",
    captureTitle: "Kumi 播放并测量了这个片段；这不是改动，所以没有需要撤销的内容",
    waveform: (label) => `${label} 的波形`,
    play: (label) => `播放 ${label}`,
    stop: (label) => `停止 ${label}`,
    audioGone: "这段音频已不可用。",
    this: "当前",
    reference: "参考",
    difference: "差值",
    bandsDifference: "频段平衡（当前减参考，已对齐电平）",
    bandsLevel: "频段电平",
    measuredAudio: "已测量的音频",
    loudness: "响度",
    truePeak: "真峰值",
    brightness: "明亮度（质心）",
    range: "响度范围",
    crest: "峰值因数",
    tempo: "速度",
    key: "调性",
    peakToLoudness: "峰值响度比",
    tilt: "倾斜",
    perOctave: (value) => `${value} dB/oct`,
    width: "立体声宽度",
    lowEnd: "低频",
    mono: "单声道",
    notMono: "非单声道",
    overlay: "频段电平（已对齐电平）",
    overlayBand: (name, hz2, mine, reference, difference) => `${name}（${hz2} Hz）：当前 ${mine} dB，参考 ${reference} dB，差 ${difference} dB`,
    round: (number) => `第 ${number} 轮`,
    roundTitle: "本窗口打开以来与此参考的每次比较（从旧到新，当前减参考）",
    loudnessGap: (trend) => `响度 ${trend} LU`,
    tiltGap: (trend) => `倾斜 ${trend} dB/oct`,
    widthGap: (trend) => `宽度 ${trend}`,
    interpretation: "解读",
    noInterpretation: (why) => `没有模型解读：${why}`,
    more: "展开",
    less: "收起",
    beats: (from, to) => `第 ${from}–${to} 拍`,
    secondsMeasured: (seconds2) => `测量了 ${seconds2} 秒`,
    through: (by) => /^[A-Za-z]/.test(by) ? `通过 ${by}` : `通过${by}`,
    ears: "Kumi Ears",
    offline: "离线渲染",
    restored: "项目已还原",
    versus: (reference) => `（对比 ${reference}）`
  },
  settings: {
    intro: (daw) => `Kumi 通过你批准的类型化改动读取和修改你的 ${daw || "DAW"} 项目，保留可撤销的 HISTORY，在这台电脑上测量音频，并为你允许的采样文件夹生成地图。`,
    agentOff: "此配置中 Kumi 的 Agent 开关已关闭。请在“技能与工具 → 插件”中打开 kumi 插件的 Agent 开关并重启网关；它的设置表单会出现在“智能体设置”中。",
    setup: "设置",
    state: "状态",
    next: "下一步",
    runtime: "Kumi 运行时",
    installed: (version) => `Kumi ${version}`,
    notInstalled: "未安装",
    needs: (version) => `需要 ${version}`,
    download: "下载",
    unpacked: (size) => `（解压后 ${size}）`,
    part: (daw) => `Kumi 的 ${daw} 组件`,
    componentLine: (name, state) => `${name}：${state}`,
    standalone: "独立的 Kumi",
    adopted: "已接管",
    leftAsIs: "保持不变",
    audioFormats: "音频格式",
    connection: "连接",
    now: "当前",
    chat: "Kumi 的对话",
    chatHere: "此对话",
    chatElsewhere: "另一个对话",
    openMap: "打开采样地图",
    agentSettings: "智能体设置",
    thisWindow: "此窗口",
    openOnFind: "Kumi 找到声音时打开采样地图",
    petLines: "Kumi 完成改动或任务时，让宠物在气泡里说一声",
    states: {
      ready: "就绪",
      needs_runtime: "尚未设置",
      outdated_runtime: "需要更新：请运行设置",
      damaged_runtime: "需要重新安装",
      needs_daw_component: "需要 Kumi 的 DAW 组件",
      no_pin: "这份插件无法设置 Kumi",
      unsupported_platform: "此系统上不可用",
      unknown: "未知"
    },
    platforms: {
      "aarch64-apple-darwin": "macOS（Apple 芯片）",
      "x86_64-apple-darwin": "macOS（Intel）",
      "aarch64-pc-windows-msvc": "Windows（ARM64）",
      "x86_64-pc-windows-msvc": "Windows（x64）",
      "aarch64-unknown-linux-gnu": "Linux（ARM64）",
      "x86_64-unknown-linux-gnu": "Linux（x86-64）"
    },
    componentStates: {
      not_installed: "未安装",
      installed: "已安装",
      outdated: "版本过旧",
      unknown: "未知"
    },
    partInUse: "使用中（Kumi 正通过它连接）"
  },
  generate: {
    generated: "生成",
    generatedTitle: "由 Mirelo 根据你的描述生成。放入项目之前，它不会出现在你的项目中。",
    soundTitle: "生成的声音",
    notesTitle: "从音频提取的音符",
    kinds: { video_to_sfx: "视频音效", extend: "延长的音频", inpaint: "局部替换的音频" },
    window: {
      video_to_sfx: (first) => `视频音效 · 前 ${first}`,
      extend: (added, source) => `延长 ${added}（原长 ${source}）`,
      inpaint: (span2, source) => `替换了 ${source} 中的 ${span2}`
    },
    credits: (_n, shown) => `${shown} 积分`,
    creditsTitle: "Mirelo API 积分，不是钱。预估或预留不是价格。",
    cost: (credits2, estimate) => `${credits2}（预估 ${estimate}）`,
    reserved: (credits2) => `已预留 ${credits2}`,
    unknownCharge: (credits2) => `费用尚不清楚 · 已预留 ${credits2}`,
    notSubmitted: "未发送给 Mirelo · 未收费",
    left: (left, cap) => `上限 ${cap} 积分，剩余 ${left}`,
    capTitle: "Kumi 自己的付费生成预算，批准时从中扣除。这不是服务商的账单。",
    frozen: "有一笔费用超出了预估，新的付费生成已暂停",
    notes: (_n, shown) => `${shown} 个音符`,
    outputs: { midi: "MIDI 文件", notes: "音符", musicxml: "MusicXML" },
    states: { running: "生成中", failed: "失败", cancelled: "已取消", interrupted: "已中断" },
    place: "放入…",
    placeTitle: "把放入这个文件的请求放进对话，补充完整后发送",
    planNotes: "规划音符…",
    planNotesTitle: "把根据项目自身的节拍规划这些音符放在哪里的请求放进对话，补充完整后发送"
  },
  pet: {
    undone: (change) => `已撤销：${change}`
  },
  palette: {
    map: "Kumi：打开采样地图",
    undo: "Kumi：撤销最近一次改动",
    settings: "Kumi：设置"
  },
  dock: {
    title: "调色板",
    sounds: "调色板里的声音",
    empty: "调色板还是空的。在地图上拖出一个框选中声音（按住 Shift 可再加一个框），然后按“加入调色板”。",
    none: "还没有调色板：“加入调色板”会新建一个。",
    choose: "选择调色板",
    switch: "调色板",
    newPalette: "新建调色板…",
    newName: "为新调色板命名",
    rename: "重命名",
    renameTitle: "重命名这个调色板",
    forget: "删除…",
    forgetTitle: (name) => `删除 ${name}：调色板会消失，声音仍留在你的音色库中`,
    forgetAsk: (name, n) => `删除 ${name} 及其中的 ${n} 个声音？这些声音仍留在你的音色库中。`,
    forgetYes: "删除",
    keep: "保留",
    mode: "Kumi 如何使用调色板",
    strict: "严格",
    strictTitle: "你要用调色板时，Kumi 只用这些声音",
    loose: "宽松",
    looseN: (k) => `宽松 · ${k}`,
    looseTitle: (k) => `Kumi 也可能推荐每个声音最接近的 ${k} 个，加入前会先问你`,
    count: (n) => `${n} 个声音`,
    excluded: (n) => `已排除 ${n} 个`,
    more: (n) => `还有 ${n} 个`,
    play: (name) => `播放 ${name}`,
    stop: (name) => `停止 ${name}`,
    remove: (name) => `把 ${name} 移出调色板`,
    offMap: "不在这张地图上：它的文件已移动，或已不在你的采样文件夹中。",
    reorderHint: "拖动来移动它，或按 Alt 加 ← / →",
    collapse: "隐藏调色板",
    expand: "显示调色板",
    kit: "用作套件…",
    kitTitle: "把用这些声音载入套件（在 Live 中是 Drum Rack）的请求放进对话，由你决定何时发送",
    kitCopy: "复制套件请求",
    add: "加入调色板",
    addTitle: "把这个声音放进你的调色板",
    addPicked: (n) => `把 ${n} 个加入调色板`,
    picked: (n) => `已选 ${n} 个`,
    clearPicks: (n) => `清除（${n}）`,
    clearPicksTitle: "取消所有选择（按 Esc 或点击地图空白处也可以）",
    cleared: "已取消选择",
    hover: "悬停播放",
    hoverTitle: "指针移到哪个声音上就立即播放它",
    firstOnly: (n) => `所选的 ${n} 个中，前 10,000 个已加入。`,
    pickAll: (n) => `选择全部 ${n} 个`,
    pickAllTitle: "选择这次搜索保留的所有声音",
    pickFoundTitle: "选择 Kumi 找到的所有声音",
    inPalette: "已在调色板中",
    failed: (error) => `Kumi 无法修改调色板：${error}`
  },
  make: {
    open: "制作…",
    openTitle: "用这些声音制作一个新声音：交叉、熔合或叠加",
    title: "制作声音",
    from: (n) => `来自 ${n} 个声音`,
    close: "关闭",
    sources: "用来制作的声音",
    way: "做法",
    results: "做出的声音",
    none: "做出的声音会列在这里：可以播放、重掷，把喜欢的保留在调色板里。",
    ops: { cross: "交叉", fuse: "熔合", stack: "叠加" },
    opHints: { cross: "一个声音，带着几个声音的特点", fuse: "用它们做成更长的声音", stack: "叠成一个" },
    cards: {
      blend: { title: "融合", hint: "把它们揉成一个声音主体" },
      attack: { title: "起音", hint: "一个声音的起音接上其余声音的混合" },
      resonate: { title: "共鸣", hint: "一半声音通过另一半鸣响" },
      modulate: { title: "调制", hint: "它们相互调制" },
      texture: { title: "纹理", hint: "在它们之间游走的循环" },
      loop: { title: "循环", hint: "用它们的击打组成节拍" },
      smear: { title: "涂抹", hint: "长长的颗粒拉伸" },
      stack: { title: "叠加", hint: "合成一个击打" }
    },
    options: { seconds: "长度", bpm: "速度", bars: "小节", depth: "深度", stretch: "拉伸", attack_from: "起音来自" },
    optionValues: {
      seconds: (value) => `长度 ${value}`,
      bpm: (value) => `速度 ${value} BPM`,
      bars: (n) => `${n} 小节`,
      depth: (percent) => `深度 ${percent}%`,
      stretch: (times) => `拉伸 ${times}×`
    },
    auto: "自动",
    autoTitle: "自动：每次制作时决定。拖动滑块即可自己设定。",
    make: "制作",
    makeTitle: (way) => `用“${way}”制作一个声音`,
    making: "制作中…",
    needTwo: (n) => `至少选择 ${n} 个声音才能制作。`,
    moreSources: (n) => `还有 ${n} 个`,
    missing: "这个版本的 Kumi for Hermes 还不能制作声音：请先更新，再试一次。",
    failed: (error) => `Kumi 没能做出来：${error}`,
    seedTitle: "种子：同样的声音、同样的做法和同样的种子会再次做出同样的声音",
    variations: "变体",
    previous: "上一个变体",
    next: "下一个变体",
    variation: (i, n) => `${i} / ${n}`,
    play: "播放",
    stop: "停止",
    reroll: "重掷",
    rerolling: "制作中…",
    rerollTitle: "再做一个变体：同样的声音和做法，换一个种子",
    keep: "保留",
    keepTitle: "放进调色板",
    kept: "已在调色板中",
    use: "使用…",
    useTitle: "把在打击垫或音轨上使用这个声音的请求放进对话，由你决定何时发送",
    copyUse: "复制请求"
  },
  notify: {
    nothingToUndo: "Kumi 现在没有可以撤销的改动。",
    undoUnsure: "Kumi 不确定它的上一次改动：请先让 Hermes 让它重新查看你的项目。",
    copied: "已复制请求：粘贴到对话中发送。",
    askInChat: (text) => `请在对话中提出：${text}`,
    openChat: "请先打开一个对话，然后重试。",
    notOpenHere: "Kumi 的对话没有在这里打开，所以请求还在你的输入框中，尚未发送。请在 Kumi 连接的对话中发送。"
  },
  dashboard: {
    agentOff: "此配置中 Kumi 的 Agent 开关已关闭。",
    agentOffHow: "打开它（插件页面，或 `hermes plugins enable kumi`）并重启仪表板，即可在这里看到 Kumi。",
    copyTitle: (text) => `复制“${text}”以粘贴到对话中`,
    copied: "已复制",
    copyRequest: "复制请求",
    copyUndo: "复制撤销",
    mapEmpty: "还没有采样地图：请让 Hermes 添加一个采样文件夹并扫描。",
    askLook: "下一次改动之前，请在对话中让 Hermes 重新查看。"
  }
};
var MESSAGES = { en, ja, zh };
function localeOf(raw) {
  const id = typeof raw === "string" ? raw.toLowerCase() : "";
  if (id.startsWith("ja")) return "ja";
  if (/^zh[-_](?:hant|tw|hk|mo)(?![a-z])/.test(id)) return "en";
  if (id.startsWith("zh")) return "zh";
  return "en";
}
function lookup(messages, key) {
  let value = messages;
  for (const part of key.split(".")) value = value && typeof value === "object" ? value[part] : void 0;
  return value;
}
function translator(locale) {
  const own = MESSAGES[localeOf(locale)];
  return (key, ...args) => {
    const message = lookup(own, key) ?? lookup(en, key);
    if (typeof message === "function") return String(message(...args));
    return typeof message === "string" ? message : key;
  };
}
var english = translator("en");

// src/shared/format.ts
var DAWS = { ableton: "Live", live: "Live", reaper: "REAPER", renoise: "Renoise", soundcraft: "SoundCraft" };
function knownDaw(daw) {
  return typeof daw === "string" && DAWS[daw.toLowerCase()] || null;
}
function dawName(daw, t = english) {
  return knownDaw(daw) ?? t("common.yourDaw");
}
function nameOf(value) {
  if (typeof value === "string") return value.trim() || void 0;
  if (value && typeof value === "object" && typeof value.name === "string") return value.name.trim() || void 0;
  return void 0;
}
function focusPath(focus) {
  const parts = focus?.focus;
  const inner = nameOf(parts?.device) ?? nameOf(parts?.item?.name) ?? nameOf(parts?.clip);
  return [nameOf(parts?.track), inner].filter((part) => Boolean(part)).map((part) => clean(part, 40));
}
function focusPlace(focus) {
  const parts = focus?.focus;
  const view = nameOf(parts?.mainView?.label) ?? nameOf(parts?.view);
  const section = nameOf(parts?.section?.name) ?? nameOf(parts?.scene);
  return [view && clean(view, 20), section && clean(section, 24)].filter((part) => Boolean(part));
}
function focusParameter(focus) {
  const parameter = focus?.focus?.parameter;
  const name = nameOf(parameter?.name);
  if (!name) return void 0;
  const value = nameOf(parameter?.value ?? void 0);
  return clean(value ? `${name} ${value}` : name, 60);
}
function catchUpLines(focus, most = 4) {
  const entries = Array.isArray(focus?.catch_up) ? focus.catch_up : [];
  const all = entries.flatMap((entry) => entry && Array.isArray(entry.lines) ? entry.lines.filter((line) => typeof line === "string") : []);
  const unlisted = entries.reduce((sum, entry) => sum + (entry && typeof entry.more === "number" && entry.more > 0 ? Math.floor(entry.more) : 0), 0);
  return { lines: all.slice(0, most).map((line) => clean(line, 120)), more: Math.max(0, all.length - most) + unlisted };
}
function colorOf(value) {
  const color = value && typeof value === "object" ? value.color : void 0;
  return typeof color === "string" && /^#[0-9a-f]{6}$/i.test(color) ? color : void 0;
}
function clean(text, max = 80) {
  if (typeof text !== "string") return "";
  const one = text.replace(/[\u0000-\u001f\u007f-\u009f\u2028\u2029]+/g, " ").replace(/[\u061c\u200b-\u200f\u202a-\u202e\u2060-\u2064\u2066-\u206f\ufeff\u{e0000}-\u{e007f}]/gu, "").replace(/\s+/g, " ").replace(/"/g, "”").trim();
  return one.length > max ? `${one.slice(0, max - 1)}…` : one;
}
var MINUS = "−";
function num(value, digits = 1) {
  const fixed = Math.abs(value).toLocaleString("en-US", { maximumFractionDigits: digits, minimumFractionDigits: 0 });
  return value < 0 && fixed !== "0" ? `${MINUS}${fixed}` : fixed;
}
function signed(value, digits = 1) {
  const text = num(value, digits);
  return value > 0 && text !== "0" ? `+${text}` : text;
}
function hz(value) {
  return value >= 1e4 ? `${num(value / 1e3, 1)} kHz` : `${num(value, 0)} Hz`;
}
function bpm(tempo) {
  return typeof tempo === "number" && tempo > 0 ? `${num(tempo, 1)} BPM` : void 0;
}
function ago(at, now = Date.now(), t = english) {
  if (typeof at !== "number") return "";
  const seconds2 = Math.max(0, (now - at) / 1e3);
  if (seconds2 < 45) return t("time.now");
  if (seconds2 < 3600) return t("time.minutes", Math.round(seconds2 / 60));
  if (seconds2 < 86400) return t("time.hours", Math.round(seconds2 / 3600));
  return t("time.days", Math.round(seconds2 / 86400));
}
function seconds(value) {
  if (typeof value !== "number" || !(value >= 0)) return "";
  return value < 10 ? `${num(value, 2)} s` : value < 60 ? `${num(value, 1)} s` : `${Math.floor(value / 60)}:${String(Math.round(value % 60)).padStart(2, "0")}`;
}
var JOB_KINDS = {
  scan: { running: "job.scanning", noun: "job.scan" },
  generate: { running: "job.generating", noun: "job.generation" }
};
function count(value) {
  if (Array.isArray(value)) return value.length;
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : null;
}
function jobWords(job, t = english) {
  const kind = typeof job.kind === "string" && job.kind ? job.kind : "";
  const known = JOB_KINDS[kind];
  const noun = known ? t(known.noun) : clean(kind, 24) || t("job.job");
  const running = known ? t(known.running) : t("job.running", noun);
  const progress = job.progress ?? {};
  const done = typeof progress.done === "number" && Number.isFinite(progress.done) ? progress.done : null;
  const total = typeof progress.total === "number" && Number.isFinite(progress.total) && progress.total > 0 ? progress.total : null;
  const message = typeof progress.message === "string" ? clean(progress.message, 60) : "";
  if (job.state === "running" || job.state === void 0) {
    const count2 = done === null ? "" : total === null ? num(done, 0) : t("job.of", num(done, 0), num(total, 0));
    return { title: running, detail: [message, count2].filter(Boolean).join(" · "), fraction: done !== null && total !== null ? Math.min(1, done / total) : null, tone: "muted" };
  }
  if (job.state === "done") {
    const result = job.result && typeof job.result === "object" ? job.result : {};
    const sounds = count(result.sounds);
    const failed = count(result.failed);
    const detail = [sounds === null ? "" : t("job.sounds", sounds, num(sounds, 0)), failed ? t("job.unreadable", num(failed, 0)) : ""];
    return { title: t("job.done", noun), detail: detail.filter(Boolean).join(" · "), fraction: 1, tone: failed ? "warn" : "good" };
  }
  if (job.state === "failed") return { title: t("job.failed", noun), detail: clean(job.error?.reason ?? "", 120), fraction: null, tone: "bad" };
  if (job.state === "cancelled") return { title: t("job.stopped", noun), detail: "", fraction: null, tone: "muted" };
  if (job.state === "interrupted") return { title: t("job.interrupted", noun), detail: t("job.interruptedWhy"), fraction: null, tone: "warn" };
  return { title: noun, detail: clean(job.state ?? "", 24), fraction: null, tone: "muted" };
}
var ID = /^[A-Za-z0-9][A-Za-z0-9_.:-]{0,63}$/;
function plainId(value) {
  return typeof value === "string" && ID.test(value) ? value : null;
}
function canUndo(status, item) {
  return Boolean(item && status?.connected && !status.stale && !status.unsure && item.undo_available === true && plainId(item.id));
}
function undoRequest(item) {
  return `Undo Kumi change ${plainId(item.id) ?? ""}`.trim();
}
function changesOf(items) {
  const byChange = /* @__PURE__ */ new Map();
  for (const item of items) {
    const key = typeof item.change_id === "string" && item.change_id ? `change:${item.change_id}` : `row:${item.id}`;
    const rows = byChange.get(key);
    if (rows) rows.push(item);
    else byChange.set(key, [item]);
  }
  return [...byChange.values()];
}
function undoRowOf(rows) {
  return rows.find((row2) => row2.state !== "undone");
}
function canUndoChange(status, rows) {
  return canUndo(status, undoRowOf(rows));
}
var QUOTE_MARKS = /["“”„‟″‶〃＂«»「」『』〝〞〟｢｣﹁﹂﹃﹄]/g;
function quotable(name, max = 80) {
  return clean(typeof name === "string" ? name.replace(QUOTE_MARKS, " ") : name, max).replace(QUOTE_MARKS, " ").replace(/\s+/g, " ").trim();
}
function placeRequest(artifactId, name) {
  const id = plainId(artifactId);
  if (!id) return null;
  const said = quotable(name);
  return said ? `Place “${said}” (artifact ${id}) in my project` : `Place artifact ${id} in my project`;
}
function planNotesRequest(artifactId, count2) {
  const id = plainId(artifactId);
  if (!id) return null;
  const notes = typeof count2 === "number" && Number.isInteger(count2) && count2 >= 0 ? `the ${count2} notes` : "the notes";
  return `Plan putting ${notes} from artifact ${id} (timing in seconds) in my project: read the project's tempo and where they'd go first, and show me before changing anything`;
}
function soundRequest(soundId, name, revision) {
  const id = plainId(soundId);
  if (!id) return null;
  const from = revision ? plainId(revision) : null;
  const said = quotable(name);
  const which = `sound ${id}${from ? ` from map ${from}` : ""}`;
  return said ? `Use “${said}” (${which}) on the selected track` : `Use ${which} on the selected track`;
}
var MOST_NEAR = 16;
var NEAR_NAMED = 3;
function nearRequest(sounds, revision) {
  const kept = sounds.flatMap((sound) => {
    const id = plainId(sound.id);
    return id ? [{ id, said: quotable(sound.name) }] : [];
  }).slice(0, MOST_NEAR);
  if (kept.length === 0) return null;
  const from = revision ? plainId(revision) : null;
  const which = `${kept.length === 1 ? "sound" : "sounds"} ${kept.map((sound) => sound.id).join(", ")}${from ? ` from map ${from}` : ""}`;
  const names = kept.flatMap((sound) => sound.said ? [`“${sound.said}”`] : []).slice(0, NEAR_NAMED);
  if (names.length === 0) return `Find more sounds like ${which}`;
  const rest = kept.length - names.length;
  const listed = rest > 0 ? `${names.join(", ")} and ${rest} more` : names.length > 1 ? `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}` : names[0];
  return `Find more sounds like ${listed} (${which})`;
}
var NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
function liveNote(note) {
  return `${NOTE_NAMES[note % 12]}${Math.floor(note / 12) - 2}`;
}
function padRequest(soundId, note, daw, name, revision) {
  const id = plainId(soundId);
  if (!id || !Number.isInteger(note) || note < 0 || note > 127) return null;
  const from = revision ? plainId(revision) : null;
  const said = quotable(name);
  const which = `sound ${id}${from ? ` from map ${from}` : ""}`;
  const pad = daw === "ableton" ? `the Drum Rack pad ${liveNote(note)} (note ${note})` : `the drum pad for note ${note}`;
  return `Put ${said ? `“${said}” (${which})` : which} on ${pad} on the selected track`;
}
function setupRequest(daw) {
  return `Set up Kumi for ${dawName(daw)}`;
}
function connectRequest(daw) {
  return `Connect Kumi to ${dawName(daw)}`;
}
var LOOK_AGAIN_REQUEST = "Kumi, look at my project again before changing anything else";
var RESCAN_REQUEST = "Kumi, rescan my sample folders";
function cleanupWords(cleanup, t = english) {
  if (cleanup === "restored") return t("history.restored");
  if (cleanup === "unsure") return t("history.restoreUnsure");
  return "";
}
function chipWords(status, tempo, t = english) {
  if (!status) return { tone: "muted", text: t("chip.kumi") };
  const daw = dawName(status.daw, t);
  const project = clean(status.project?.name ?? "", 40);
  if (status.connected) {
    const parts = [daw, project, bpm(tempo)].filter(Boolean);
    return { tone: "good", text: parts.join(" · ") };
  }
  if (status.stale && status.running === false) return { tone: "muted", text: [daw, project, t("chip.stopped")].filter(Boolean).join(" · ") };
  if (status.stale && status.owner) return { tone: "warn", text: [daw, project, t("chip.notAnswering")].filter(Boolean).join(" · ") };
  return { tone: "muted", text: t("chip.notConnected", daw) };
}

// src/shared/hooks.ts
import { useCallback, useEffect, useRef, useSyncExternalStore } from "react";

// src/shared/listen.ts
var NOT_MEASURED = { preview: true, refused: true, cancelled: true, unsure: true };
function parseListen(result) {
  let value = result;
  if (typeof value === "string") {
    try {
      value = JSON.parse(value);
    } catch {
      return null;
    }
  }
  if (!value || typeof value !== "object" || !("metrics" in value) || !value.metrics || typeof value.metrics !== "object") return null;
  const state = "state" in value ? value.state : void 0;
  if (typeof state === "string" && NOT_MEASURED[state]) return null;
  return value;
}
var CAPTURED_BY = { ears: "listen.ears", offline: "listen.offline" };
function captureWords(capture, t = english) {
  if (!capture || typeof capture !== "object") return void 0;
  const parts = [];
  const from = capture.from_beat;
  const beats = capture.beats;
  if (finite(from) && finite(beats)) parts.push(t("listen.beats", num(from, 2), num(from + beats, 2)));
  if (finite(capture.seconds)) parts.push(t("listen.secondsMeasured", num(capture.seconds, 1)));
  const method = typeof capture.method === "string" ? capture.method : "";
  const by = method ? CAPTURED_BY[method] ? t(CAPTURED_BY[method]) : clean(method, 20) : "";
  const said = [parts.join(" · "), by && t("listen.through", by)].filter(Boolean).join(", ");
  if (!said) return void 0;
  return capture.cleanup === "restored" ? `${said} · ${t("listen.restored")}` : said;
}
var finite = (value) => typeof value === "number" && Number.isFinite(value);
function fileName(analysis, fallback) {
  const file = typeof analysis?.file === "string" ? analysis.file.split(/[\\/]/).pop() : void 0;
  return clean(file || fallback, 60);
}
function row(label, mine, reference, show, delta, deltaShow) {
  if (!finite(mine) && !finite(reference)) return null;
  const out = { label, mine: finite(mine) ? show(mine) : "—", direction: 0 };
  if (reference !== void 0) {
    out.reference = finite(reference) ? show(reference) : "—";
    const difference = finite(delta) ? delta : finite(mine) && finite(reference) ? mine - reference : void 0;
    if (difference !== void 0) {
      out.delta = (deltaShow ?? show)(difference);
      out.direction = difference < 0 ? -1 : difference > 0 ? 1 : 0;
    }
  }
  return out;
}
function listenView(result, t = english, referenceName2) {
  const mine = result.metrics ?? {};
  const compare = result.compare;
  const reference = compare?.metrics;
  const differences = compare?.differences;
  const theirs = (value) => compare ? value ?? null : void 0;
  const rows = [
    row(t("listen.loudness"), mine.loudness?.integratedLufs, theirs(reference?.loudness?.integratedLufs), (v) => `${num(v)} LUFS`, differences?.loudness?.differenceLu, (v) => `${signed(v)} LU`),
    row(t("listen.truePeak"), mine.loudness?.truePeakDbtp, theirs(reference?.loudness?.truePeakDbtp), (v) => `${num(v)} dBTP`, void 0, (v) => `${signed(v)} dB`),
    row(t("listen.range"), mine.loudness?.rangeLu, theirs(reference?.loudness?.rangeLu), (v) => `${num(v)} LU`, void 0, (v) => `${signed(v)} LU`),
    row(t("listen.crest"), mine.dynamics?.crestDb, theirs(reference?.dynamics?.crestDb), (v) => `${num(v)} dB`, void 0, (v) => `${signed(v)} dB`),
    row(t("listen.peakToLoudness"), mine.dynamics?.peakToLoudnessDb, theirs(reference?.dynamics?.peakToLoudnessDb), (v) => `${num(v)} dB`, void 0, (v) => `${signed(v)} dB`),
    row(t("listen.tilt"), mine.balance?.tiltDbPerOctave, theirs(reference?.balance?.tiltDbPerOctave), (v) => t("listen.perOctave", num(v)), void 0, (v) => t("listen.perOctave", signed(v))),
    row(t("listen.brightness"), mine.balance?.centroidHz, theirs(reference?.balance?.centroidHz), hz, void 0, (v) => `${signed(v, 0)} Hz`),
    row(t("listen.width"), mine.stereo?.width, theirs(reference?.stereo?.width), (v) => num(v, 2), void 0, (v) => signed(v, 2)),
    lowEnd(t, mine.stereo?.lowEndMono, compare ? reference?.stereo?.lowEndMono ?? null : void 0),
    row(t("listen.tempo"), mine.tempo?.bpm, theirs(reference?.tempo?.bpm), (v) => `${num(v)} BPM`, void 0, (v) => `${signed(v)} BPM`)
  ].filter((value) => value !== null);
  const key = mine.sound ? void 0 : mine.key?.name;
  if (typeof key === "string" && key) {
    const otherKey = reference?.key?.name;
    rows.push({ label: t("listen.key"), mine: clean(key, 20), ...compare ? { reference: typeof otherKey === "string" && otherKey ? clean(otherKey, 20) : "—" } : {}, direction: 0 });
  }
  const differenceBands = (Array.isArray(differences?.balance) ? differences.balance : []).flatMap(
    (band) => band && finite(band.difference) ? [{ name: clean(band.band ?? band.hz ?? "", 16), value: band.difference }] : []
  );
  const levelBands = (Array.isArray(mine.balance?.bands) ? mine.balance.bands : []).flatMap(
    (band) => band && finite(band.db) ? [{ name: clean(band.name ?? band.hz ?? "", 16), value: band.db }] : []
  );
  const headlines = Array.isArray(differences?.headlines) ? differences.headlines.filter((line) => typeof line === "string").map((line) => clean(line, 140)) : [];
  const said = typeof result.verdict_text === "string" ? headlines.length ? result.verdict_text.split("\n")[0] : result.verdict_text : "";
  const view = {
    title: fileName(mine, t("listen.measuredAudio")),
    mineId: typeof result.artifact_id === "string" ? result.artifact_id : void 0,
    rows,
    bands: differenceBands.length ? differenceBands : levelBands,
    bandsAre: differenceBands.length ? "difference" : "level",
    overlay: compare ? overlayOf(mine, reference) : [],
    headlines,
    verdict: said.trim() ? clean(said, 400) : void 0,
    capture: captureWords(result.capture, t),
    // CC-2 A3: one line of at most 200 characters, shown as it is and never read for a decision.
    captureNote: typeof result.capture?.note === "string" && result.capture.note.trim() ? clean(result.capture.note, 200) : void 0
  };
  if (compare) {
    view.referenceTitle = compare.name ? displayReferenceName(compare) : referenceName2 ? clean(referenceName2, 60) : fileName(reference, t("listen.reference"));
    view.referenceId = typeof compare.artifact_id === "string" ? compare.artifact_id : void 0;
  }
  const interpretation = result.interpretation;
  if (interpretation && typeof interpretation.text === "string" && interpretation.text.trim()) {
    const model = typeof interpretation.model === "string" && interpretation.model ? ` · ${clean(interpretation.model, 40)}` : "";
    view.interpretation = { label: `${t("listen.interpretation")}${model}`, text: interpretation.text.trim().slice(0, 4e3) };
  } else if (typeof result.interpretation_error === "string" && result.interpretation_error) {
    view.interpretationError = clean(result.interpretation_error, 200);
  }
  return view;
}
function overlayOf(mine, reference) {
  const ours = Array.isArray(mine.balance?.bands) ? mine.balance.bands : [];
  const theirs = Array.isArray(reference?.balance?.bands) ? reference.balance.bands : [];
  if (!ours.length || ours.length !== theirs.length) return [];
  const overlay = [];
  for (const [index, band] of ours.entries()) {
    const other = theirs[index];
    if (!band || !other || band.name !== other.name || !finite(band.db) || !finite(other.db)) return [];
    overlay.push({ name: clean(band.name ?? "", 16), hz: clean(band.hz ?? "", 16), mine: band.db, reference: other.db });
  }
  return overlay;
}
function lowEnd(t, mine, reference) {
  const words = (value) => value === true ? t("listen.mono") : value === false ? t("listen.notMono") : "—";
  if (typeof mine !== "boolean" && typeof reference !== "boolean") return null;
  return { label: t("listen.lowEnd"), mine: words(mine), ...reference === void 0 ? {} : { reference: words(reference) }, direction: 0 };
}
function displayReferenceName(reference) {
  return clean((reference.origin?.name ?? reference.name ?? "").split(/[\\/]/).pop() || "Reference", 160);
}
function windowWords(window2) {
  if (!window2 || !finite(window2.sample_rate) || window2.sample_rate <= 0 || !finite(window2.frame_count) || !finite(window2.start_frame)) return void 0;
  const start = window2.start_frame / window2.sample_rate;
  const duration = window2.frame_count / window2.sample_rate;
  const requested = window2.requested ? ` · requested ${num(window2.requested.from, 6)} + ${num(window2.requested.seconds, 6)} s` : "";
  return `Reference file seconds ${num(start, 6)}–${num(start + duration, 6)} · ${num(duration, 6)} s measured (${window2.frame_count} frames at ${window2.sample_rate} Hz)${requested}`;
}
function stable(value) {
  if (Array.isArray(value)) return `[${value.map(stable).join(",")}]`;
  if (value && typeof value === "object") return `{${Object.entries(value).sort(([a], [b]) => a.localeCompare(b)).map(([key, item]) => `${JSON.stringify(key)}:${stable(item)}`).join(",")}}`;
  return JSON.stringify(value) ?? "null";
}
function memberKey(reference) {
  const window2 = reference.window;
  const origin = reference.origin;
  const replay = origin?.window && window2 && window2.start_frame === 0 && window2.frame_count === origin.window.frame_count && window2.sample_rate === origin.window.sample_rate && window2.channels === origin.window.channels;
  const selected = replay ? origin : reference;
  return stable({
    identity: selected.identity ?? { artifact_id: reference.artifact_id },
    window: selected.window ? {
      start_frame: selected.window.start_frame,
      frame_count: selected.window.frame_count,
      sample_rate: selected.window.sample_rate,
      channels: selected.window.channels
    } : null
  });
}
function hasReferenceFacts(result) {
  return Array.isArray(result.references) || !!(result.compare?.identity || result.compare?.window || result.compare?.origin);
}
function referenceKey(result) {
  if (Array.isArray(result.references)) return `profile:${stable(result.references.map(memberKey).sort())}`;
  if (hasReferenceFacts(result) && result.compare) return `excerpt:${memberKey(result.compare)}`;
  const reference = result.compare?.metrics;
  if (!reference) return null;
  const lufs = reference.loudness?.integratedLufs;
  return [reference.file ?? "", finite(lufs) ? lufs.toFixed(1) : "", finite(reference.seconds) ? reference.seconds.toFixed(1) : ""].join("|");
}
function keptBy(result, rounds) {
  const kept = result.compare?.artifact_id;
  if (typeof kept !== "string" || !kept) return null;
  for (const [key, each] of Object.entries(rounds)) {
    const round = each.find((other) => other.kept === kept);
    if (round) return [key, round];
  }
  return null;
}
function roundKey(result, rounds) {
  return hasReferenceFacts(result) ? referenceKey(result) : keptBy(result, rounds)?.[0] ?? referenceKey(result);
}
function referenceName(result, rounds) {
  return keptBy(result, rounds)?.[1].name || void 0;
}
function roundOf(result, id, rounds) {
  const kept = result.compare?.artifact_id;
  return { id, gaps: gapsOf(result), kept: typeof kept === "string" && kept ? kept : null, name: result.compare?.name ? displayReferenceName(result.compare) : referenceName(result, rounds) ?? fileName(result.compare?.metrics, "") };
}
function gapsOf(result) {
  const mine = result.metrics ?? {};
  const reference = result.compare?.metrics;
  if (!reference) return {};
  const gap = (a, b) => finite(a) && finite(b) ? Math.round((a - b) * 100) / 100 : void 0;
  const loudness = finite(result.compare?.differences?.loudness?.differenceLu) ? result.compare.differences.loudness.differenceLu : gap(mine.loudness?.integratedLufs, reference.loudness?.integratedLufs);
  const out = {};
  if (loudness !== void 0) out.loudness = loudness;
  const tilt = gap(mine.balance?.tiltDbPerOctave, reference.balance?.tiltDbPerOctave);
  if (tilt !== void 0) out.tilt = tilt;
  const width = gap(mine.stereo?.width, reference.stereo?.width);
  if (width !== void 0) out.width = width;
  return out;
}
function roundWords(number, rounds, t = english) {
  if (number < 2 || rounds.length < 2) return void 0;
  const trend = (pick2, digits) => {
    const values = rounds.map(pick2);
    return values.every(finite) ? values.map((value) => signed(value, digits)).join(" → ") : void 0;
  };
  const parts = [
    [trend((gaps) => gaps.loudness, 1), (text) => t("listen.loudnessGap", text)],
    [trend((gaps) => gaps.tilt, 1), (text) => t("listen.tiltGap", text)],
    [trend((gaps) => gaps.width, 2), (text) => t("listen.widthGap", text)]
  ];
  const said = parts.flatMap(([text, words]) => text ? [words(text)] : []);
  return [t("listen.round", number), ...said].join(" · ");
}

// src/shared/design.ts
var CARDS = [
  { key: "blend", op: "cross", mode: "blend", options: ["seconds"] },
  { key: "attack", op: "cross", mode: "attack", options: ["attack_from"] },
  { key: "resonate", op: "cross", mode: "resonate", options: ["depth"] },
  { key: "modulate", op: "cross", mode: "modulate", options: ["depth"] },
  { key: "texture", op: "fuse", mode: "texture", options: ["seconds", "depth"] },
  { key: "loop", op: "fuse", mode: "loop", options: ["bpm", "bars"] },
  { key: "smear", op: "fuse", mode: "smear", options: ["stretch", "depth"] },
  { key: "stack", op: "stack", options: [] }
];
var RANGES = {
  seconds: { min: 0.5, max: 30, step: 0.5 },
  bpm: { min: 40, max: 240, step: 1 },
  bars: { min: 1, max: 4, step: 1 },
  depth: { min: 0, max: 1, step: 0.05 },
  stretch: { min: 4, max: 16, step: 1 }
};
function cardNote(count2) {
  return count2 < 2 ? { key: "make.needTwo", least: 2 } : null;
}
function requestFor(card, sources, options = {}, seed) {
  const kept = {};
  for (const key of card.options) {
    const value = options[key];
    if (key === "attack_from") {
      if (typeof value === "number" && Number.isInteger(value) && value >= 0 && value < sources.length) kept.attack_from = value;
    } else if (key !== "split" && typeof value === "number" && Number.isFinite(value)) {
      const range2 = RANGES[key];
      kept[key] = Math.min(range2.max, Math.max(range2.min, value));
    }
  }
  return {
    op: card.op,
    ...card.mode ? { mode: card.mode } : {},
    sources,
    ...seed === void 0 ? {} : { seed },
    ...Object.keys(kept).length ? { options: kept } : {}
  };
}
function reroll(request) {
  const { seed: _seed, ...rest } = request;
  return rest;
}
function takeKey(result) {
  return JSON.stringify([result.op, result.mode ?? null, result.sources.map((source) => source.sound_id ?? source.artifact_id ?? "")]);
}
function requestOf(result) {
  return { op: result.op, ...result.mode ? { mode: result.mode } : {}, sources: result.sources.map((source) => ({ ...source })) };
}
function resultName(result) {
  const lead = quotable(result.recipe.split(/[:;]/, 1)[0] ?? "", 60) || quotable(result.mode ?? result.op, 60).replace(/^\w/, (letter) => letter.toUpperCase());
  return Number.isInteger(result.seed) ? `${lead}, seed ${result.seed}` : lead;
}
function useRequest(result) {
  const id = plainId(result.artifact_id);
  if (!id) return null;
  const said = resultName(result);
  return said ? `Put “${said}” (artifact ${id}) on a pad or track in my project` : `Put artifact ${id} on a pad or track in my project`;
}
function sourcesOf(items, revision) {
  const out = [];
  for (const item of items) {
    const name = quotable(item.name ?? "", 120) || void 0;
    if (item.sound_id && revision) out.push({ sound_id: item.sound_id, map_revision: revision, ...name ? { name } : {} });
    else if (item.artifact_id) out.push({ artifact_id: item.artifact_id, ...name ? { name } : {} });
  }
  return out;
}
function resultOf(value) {
  if (!value || typeof value !== "object") return null;
  const raw = value;
  const id = typeof raw.artifact_id === "string" ? plainId(raw.artifact_id) : null;
  const op = raw.op === "cross" || raw.op === "fuse" || raw.op === "stack" ? raw.op : null;
  if (!id || !op || typeof raw.seed !== "number" || !Number.isInteger(raw.seed)) return null;
  const peaks = Array.isArray(raw.peaks) ? raw.peaks.map((peak) => typeof peak === "number" && Number.isFinite(peak) ? Math.min(1, Math.max(0, peak)) : 0) : [];
  const sources = Array.isArray(raw.sources) ? raw.sources.filter((source) => source && typeof source === "object") : [];
  const modes = CARDS.map((card) => card.mode);
  return {
    artifact_id: id,
    op,
    ...typeof raw.mode === "string" && modes.includes(raw.mode) ? { mode: raw.mode } : {},
    seed: raw.seed,
    sources,
    duration: typeof raw.duration === "number" && Number.isFinite(raw.duration) ? raw.duration : 0,
    sample_rate: typeof raw.sample_rate === "number" ? raw.sample_rate : 48e3,
    channels: typeof raw.channels === "number" ? raw.channels : 2,
    peaks,
    recipe: typeof raw.recipe === "string" ? raw.recipe : "",
    ...typeof raw.at === "number" ? { at: raw.at } : {}
  };
}

// src/shared/store.ts
var MOST_REASONS = 4;
var REASON_CHARACTERS = 60;
function foundMatches(entries, ids) {
  const out = /* @__PURE__ */ Object.create(null);
  if (!Array.isArray(entries)) return out;
  for (const entry of entries) {
    if (!entry || typeof entry !== "object") continue;
    const value = entry;
    const id = typeof value.sound_id === "string" ? value.sound_id : "";
    if (!ids.has(id) || Object.hasOwn(out, id)) continue;
    const why = (Array.isArray(value.why) ? value.why : []).map((line) => clean(line, REASON_CHARACTERS)).filter(Boolean).slice(0, MOST_REASONS);
    const closeness = value.closeness;
    out[id] = { why, ...typeof closeness === "number" && Number.isInteger(closeness) && closeness >= 0 && closeness <= 100 ? { closeness } : {} };
  }
  return out;
}
var JOB_KEPT_MS = 6e4;
var MOST_JOBS = 20;
var MOST_ROUNDS = 6;
var MOST_REFERENCES = 8;
var EMPTY = { available: null, error: null, status: null, focus: null, history: null, map: null, setup: null, found: null, jobs: {}, measured: null, rounds: {}, script: null, leaving: /* @__PURE__ */ new Set(), palette: null, paletteSaid: null, designs: null };
var HIDE_BATCH_MS = 1200;
var HIDE_MOST = 500;
var SHA256 = /^[0-9a-f]{64}$/;
function pendingScriptOf(payload) {
  if (!payload || typeof payload !== "object") return null;
  const value = payload;
  const id = plainId(value.tool_call_id);
  const language = value.language === "python" || value.language === "lua" || value.language === "device" ? value.language : null;
  if (!id || !language || typeof value.sha256 !== "string" || !SHA256.test(value.sha256)) return null;
  const text = (key) => typeof value[key] === "string" ? value[key] : void 0;
  const lines = typeof value.lines === "number" && Number.isInteger(value.lines) && value.lines >= 0 ? value.lines : void 0;
  const file = language === "device" && typeof value.file === "string" && /^[^\p{C}]{1,300}$/u.test(value.file) ? value.file : void 0;
  return { tool_call_id: id, language, sha256: value.sha256, purpose: text("purpose"), daw: text("daw"), lines, warning: text("warning"), file };
}
function finished(job) {
  return typeof job.state === "string" && job.state !== "running";
}
function statusOf(error) {
  const text = error instanceof Error ? error.message : String(error ?? "");
  const match = /^(\d{3})\b/.exec(text.trim()) ?? /\b(\d{3})\b/.exec(text);
  return match ? Number(match[1]) : 0;
}
function errorText(error) {
  const text = error instanceof Error ? error.message : String(error ?? "");
  const json = text.indexOf("{");
  if (json >= 0) {
    try {
      const body = JSON.parse(text.slice(json));
      if (body && typeof body === "object" && "detail" in body && typeof body.detail === "string") return body.detail;
    } catch {
    }
  }
  return text || "no answer";
}
function notLive(state) {
  return {
    status: state.status && { ...state.status, connected: false, stale: true },
    focus: state.focus && { ...state.focus, connected: false, stale: true },
    history: state.history && {
      ...state.history,
      connected: false,
      stale: true,
      items: state.history.items.map((item) => ({ ...item, undo_available: false }))
    }
  };
}
function heard(reply) {
  if (!reply || typeof reply !== "object" || !("age_ms" in reply)) return reply;
  const age = reply.age_ms;
  return { ...reply, heard_at: typeof age === "number" && Number.isFinite(age) ? Date.now() - age : null };
}
var KumiStore = class {
  constructor(fetch, historyLimit = 50) {
    this.fetch = fetch;
    this.historyLimit = historyLimit;
  }
  fetch;
  historyLimit;
  state = EMPTY;
  listeners = /* @__PURE__ */ new Set();
  reads = /* @__PURE__ */ new Map();
  wanted = /* @__PURE__ */ new Map();
  generation = 0;
  /** `generate.updated` cards for jobs not read yet, by job id; kept on the job once it's read. */
  cards = /* @__PURE__ */ new Map();
  /** Jobs a call started, or an event reported running, not read yet: news even when the first read finds them
   *  finished (a scan of folders already indexed ends within a second, before the pane's read). */
  started = /* @__PURE__ */ new Set();
  /** Failed plays not reported yet, by sound id, and the timer that reports them. */
  toHide = /* @__PURE__ */ new Map();
  hideTimer = null;
  /** `/status`'s count of hidden-sound edits at the last map read: another window's report reads the map again. */
  hiddenSeq = null;
  get = () => this.state;
  subscribe = (fn) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };
  set(patch) {
    this.state = { ...this.state, ...patch };
    for (const fn of [...this.listeners]) fn();
  }
  /** A view that shows `part` asks for it while mounted: the map isn't read until something shows it. */
  want(part) {
    this.wanted.set(part, (this.wanted.get(part) ?? 0) + 1);
    if (!this.state[part]) void this.refresh(part);
    return () => this.wanted.set(part, Math.max(0, (this.wanted.get(part) ?? 1) - 1));
  }
  /** Forget everything (another profile or connection): what's shown must be that profile's. */
  reset() {
    this.generation += 1;
    this.reads.clear();
    this.cards.clear();
    this.started.clear();
    this.toHide.clear();
    clearTimeout(this.hideTimer ?? void 0);
    this.hideTimer = null;
    this.hiddenSeq = null;
    this.state = EMPTY;
    for (const fn of [...this.listeners]) fn();
  }
  /** Run read `key`, or, while one is running, run it once more after it: a read never answers with what an
   *  older one, begun before the change that asked for it, saw. */
  once(key, run) {
    const running = this.reads.get(key);
    if (running) {
      running.again = true;
      return running.promise;
    }
    const generation = this.generation;
    const entry = { again: false, promise: Promise.resolve() };
    entry.promise = (async () => {
      do {
        entry.again = false;
        await run().catch(() => void 0);
      } while (entry.again && generation === this.generation);
    })().finally(() => {
      if (this.reads.get(key) === entry) this.reads.delete(key);
    });
    this.reads.set(key, entry);
    return entry.promise;
  }
  /** Read /status, then whatever its revisions say moved. */
  refreshStatus() {
    return this.once("status", async () => {
      const generation = this.generation;
      let status;
      try {
        status = await this.fetch("/status");
      } catch (error) {
        if (generation !== this.generation) return;
        if (statusOf(error) === 404) this.set({ available: false, error: null });
        else this.set({ ...notLive(this.state), error: error instanceof Error ? error.message : String(error) });
        return;
      }
      if (generation !== this.generation) return;
      const before = this.state.status;
      const after = status.revisions ?? {};
      const first = this.state.available !== true;
      const liveness = !before || before.connected !== status.connected || before.stale !== status.stale || before.running !== status.running;
      this.set({ available: true, error: null, status: heard(status) });
      const held = { focus: this.state.focus?.seq ?? null, history: this.state.history?.seq ?? null, map: this.state.map?.revision ?? null, palette: this.state.palette?.seq ?? null };
      const moved2 = (part) => this.state[part] === null || (after[part] ?? null) !== held[part];
      const hidden = typeof after.hidden === "number" ? after.hidden : 0;
      const hiddenMoved = this.hiddenSeq !== null && hidden !== this.hiddenSeq;
      this.hiddenSeq = hidden;
      const reads = [];
      if (liveness || moved2("focus")) reads.push(this.refresh("focus"));
      if (liveness || moved2("history")) reads.push(this.refresh("history"));
      if ((this.wanted.get("map") ?? 0) > 0 && (moved2("map") || hiddenMoved)) reads.push(this.refresh("map"));
      if ((this.wanted.get("palette") ?? 0) > 0 && (moved2("palette") || this.state.map !== null && moved2("map"))) reads.push(this.refresh("palette"));
      if (first || liveness || !status.connected || (this.wanted.get("setup") ?? 0) > 0) reads.push(this.refresh("setup"));
      await Promise.all(reads);
    });
  }
  refresh(part) {
    return this.once(part, async () => {
      const generation = this.generation;
      const path = part === "history" ? `/history?limit=${this.historyLimit}` : `/${part}`;
      try {
        const value = await this.fetch(path);
        if (generation !== this.generation) return;
        this.set({ [part]: heard(value) });
        if (part === "map") this.mapRead();
      } catch (error) {
        if (generation !== this.generation) return;
        if (statusOf(error) === 404 && part !== "setup") this.set({ available: false });
      }
    });
  }
  /** A map read: sounds it left out (hidden, or gone in a new revision) are done leaving. One still on it waits there,
   *  off the map, while its report is pending or on its way. */
  mapRead() {
    if (this.state.leaving.size === 0) return;
    const on = new Set((this.state.map?.points ?? []).map((point) => point[0]));
    const leaving = new Set([...this.state.leaving].filter((id) => on.has(id) && (this.toHide.has(id) || this.hideTimer !== null)));
    if (leaving.size !== this.state.leaving.size) this.set({ leaving });
  }
  /** A sound of the map in hand couldn't play here (`playFailure`): off the map now, and reported with the others that
   *  fail within `HIDE_BATCH_MS`, so the next map read leaves them out and counts them. Never a notice per sound. */
  hide(id, why) {
    const map = this.state.map;
    if (!map?.revision || this.state.leaving.has(id) || !map.points.some((point) => point[0] === id)) return;
    this.toHide.set(id, why);
    this.set({ leaving: /* @__PURE__ */ new Set([...this.state.leaving, id]) });
    if (this.hideTimer === null) this.hideTimer = setTimeout(() => void this.reportHidden(), HIDE_BATCH_MS);
  }
  async reportHidden() {
    this.hideTimer = null;
    const revision = this.state.map?.revision;
    const batch = [...this.toHide].map(([id, why]) => ({ id, why }));
    this.toHide.clear();
    if (!revision || batch.length === 0) return;
    const generation = this.generation;
    for (let at = 0; at < batch.length; at += HIDE_MOST) {
      try {
        await this.fetch("/map/hidden", { method: "POST", body: { revision, ids: batch.slice(at, at + HIDE_MOST) } });
      } catch {
      }
      if (generation !== this.generation) return;
    }
    await this.refresh("map");
  }
  /** "Show them": every hidden sound goes back on the map. */
  async showHidden() {
    const generation = this.generation;
    this.toHide.clear();
    clearTimeout(this.hideTimer ?? void 0);
    this.hideTimer = null;
    try {
      await this.fetch("/map/hidden", { method: "POST", body: { action: "clear" } });
    } catch {
      return;
    }
    if (generation !== this.generation) return;
    this.set({ leaving: /* @__PURE__ */ new Set() });
    await this.refresh("map");
  }
  /** One palette edit (`POST /palette`): the palette as the backend now has it, at once, and its sentence (with `also`
   *  after it) as `paletteSaid`; a refusal's reason there instead. True when the edit was made. */
  async paletteDo(action, also) {
    const generation = this.generation;
    try {
      const answer = await this.fetch("/palette", { method: "POST", body: action });
      if (generation !== this.generation) return false;
      const said = [typeof answer?.said === "string" ? clean(answer.said, 200) : "", also ?? ""].filter(Boolean).join(" ");
      this.set({ ...answer?.palette ? { palette: answer.palette } : {}, paletteSaid: said ? { text: said, failed: false, at: Date.now() } : null });
      return true;
    } catch (error) {
      if (generation === this.generation) this.set({ paletteSaid: { text: clean(errorText(error), 200), failed: true, at: Date.now() } });
      return false;
    }
  }
  /** The engine's recent results, newest first (`GET /designs`), for the Make panel's history. None when the backend
   *  has no sound design (an older plugin) or can't read them. */
  async readDesigns() {
    const generation = this.generation;
    try {
      const answer = await this.fetch("/designs");
      if (generation !== this.generation) return;
      const designs = (Array.isArray(answer?.designs) ? answer.designs : []).map(resultOf).filter((result) => result !== null);
      this.set({ designs });
    } catch {
      if (generation === this.generation && this.state.designs === null) this.set({ designs: [] });
    }
  }
  /** Make a sound (`POST /design`): the result goes first in `designs`. Renders are quick (well under a second for
   *  one-shots, a few for a long smear), so this waits for the answer. */
  async design(request) {
    const generation = this.generation;
    try {
      const answer = await this.fetch("/design", { method: "POST", body: request });
      const result = resultOf(answer);
      if (!result) return { error: "Kumi sent no sound back.", status: 0 };
      if (generation === this.generation) this.set({ designs: [result, ...(this.state.designs ?? []).filter((other) => other.artifact_id !== result.artifact_id)] });
      return { result };
    } catch (error) {
      const status = error && typeof error === "object" && "status" in error && typeof error.status === "number" ? error.status : statusOf(error);
      return { error: clean(errorText(error), 300), status };
    }
  }
  /** The sounds a `kumi_sounds` call found, to light up on the map of the same revision, with what it said of each
   *  (CC-23: an event's `matches` or a result's `items`) and a near search's seeds (one id, or a list of them). */
  found(revision, ids, said = {}) {
    const kept = ids.slice(0, 200);
    const seeds = Array.isArray(said.near) ? said.near : [said.near];
    const near = seeds.filter((seed) => typeof seed === "string" && seed !== "").slice(0, 16);
    this.set({ found: { revision, ids: kept, at: Date.now(), near, matches: foundMatches(said.matches, new Set(kept)) } });
  }
  /** A `kumi_listen` result to show in the Measurements tab (CC-9: the transcript showed none). */
  measure(toolCallId, result) {
    this.set({ measured: { tool_call_id: toolCallId, result: result ?? null, at: Date.now() } });
  }
  /** A `kumi_listen` result as it arrived (its call's `tool.complete` or `listen.done`): one against a reference is
   *  that reference's next round, whether it named the reference's file or the copy Kumi kept of it. The same audio
   *  twice (both signals for one call) counts once. */
  noteListen(result) {
    const parsed = parseListen(result);
    const key = parsed ? roundKey(parsed, this.state.rounds) : null;
    const id = typeof parsed?.artifact_id === "string" ? parsed.artifact_id : null;
    if (!parsed || !key || !id) return;
    const before = this.state.rounds[key] ?? [];
    if (before.some((round) => round.id === id)) return;
    const rounds = { ...this.state.rounds, [key]: [...before, roundOf(parsed, id, this.state.rounds)].slice(-MOST_ROUNDS) };
    const keys = Object.keys(rounds);
    if (keys.length > MOST_REFERENCES) delete rounds[keys[0]];
    this.set({ rounds });
  }
  /** A `script.pending` event for the chat in front (CC-19): show it, then read its saved code. An event that names
   *  no plain call id, exact SHA-256 or known language shows nothing. */
  scriptPending(payload) {
    const pending = pendingScriptOf(payload);
    if (!pending) return;
    this.set({ script: { pending, code: null } });
    const generation = this.generation;
    void this.readScript(pending.sha256, pending.language).then((code) => {
      if (generation !== this.generation || this.state.script?.pending !== pending) return;
      this.set({ script: { pending, code: code ?? false } });
    });
  }
  /** The pending script's approval settled (`script.settled`, or its call's `tool.complete`): it isn't waiting now. */
  scriptSettled(toolCallId) {
    const id = plainId(toolCallId);
    if (id && this.state.script?.pending.tool_call_id === id) this.set({ script: null });
  }
  /** A saved script's code by its SHA-256 (`/script`, which re-hashes the file): for the pending script, and for a
   *  script row's "Show code". Null when it can't be read, or when what came back isn't that code. */
  async readScript(sha256, language) {
    if (!SHA256.test(sha256)) return null;
    try {
      const read = await this.fetch(`/script?sha=${sha256}&language=${language}`);
      return read && read.sha256 === sha256 && typeof read.code === "string" ? read : null;
    } catch {
      return null;
    }
  }
  /** Follow job `id` (from a tool's result or a `job.progress` event): read it now, then while it runs. `started` when
   *  the source says it was running: a call that started it, or an event that reported it running. */
  trackJob(id, started = false) {
    const jobId = plainId(id);
    if (!jobId || this.state.jobs[jobId] || Object.keys(this.state.jobs).length >= MOST_JOBS) return;
    if (started) {
      this.started.add(jobId);
      if (this.started.size > MOST_JOBS) this.started.delete(this.started.values().next().value);
    }
    void this.refreshJob(jobId);
  }
  /** A `generate.updated` card (CC-13): keep it on its job, since its ledger is reconciled and the raw `/job` snapshot
   *  has none, and read the job as news. Unlike a job first met finished elsewhere, a generation the event reports
   *  shows even when it has already ended: with tool progress off, this row is where it shows. */
  noteGeneration(card) {
    const id = jobOf(card);
    if (!id) return;
    const before = this.state.jobs[id];
    if (before) this.set({ jobs: { ...this.state.jobs, [id]: { ...before, card } } });
    else if (Object.keys(this.state.jobs).length >= MOST_JOBS) return;
    else {
      this.cards.delete(id);
      this.cards.set(id, card);
      if (this.cards.size > MOST_JOBS) this.cards.delete(this.cards.keys().next().value);
    }
    void this.refreshJob(id);
  }
  /** Read every job still running and forget the finished ones kept long enough. While the runtime is quiet nothing
   *  moves, so nothing's read; once it has stopped, each job is read a last time, and one still "running" then ended
   *  with it. */
  refreshJobs(now = Date.now()) {
    const kept = Object.entries(this.state.jobs).filter(([, job]) => job.ended_at === null || now - job.ended_at < JOB_KEPT_MS);
    if (kept.length !== Object.keys(this.state.jobs).length) this.set({ jobs: Object.fromEntries(kept) });
    const stopped = this.state.status?.stale === true && this.state.status.running === false;
    if (this.state.status?.stale && !stopped) return Promise.resolve();
    return Promise.all(kept.filter(([, job]) => job.ended_at === null).map(([id]) => this.refreshJob(id, stopped))).then(() => void 0);
  }
  refreshJob(id, stopped = false) {
    return this.once(`job:${id}`, async () => {
      const generation = this.generation;
      try {
        const read = await this.fetch(`/job?id=${encodeURIComponent(id)}`);
        if (generation !== this.generation) return;
        const before = this.state.jobs[id];
        const news = this.started.delete(id) || this.cards.has(id);
        if (!before && finished(read) && !news) return;
        const card = before?.card ?? this.cards.get(id);
        this.cards.delete(id);
        const job = stopped && !finished(read) ? { ...read, state: "interrupted" } : read;
        const ended_at = finished(job) ? before?.ended_at ?? Date.now() : null;
        this.set({ jobs: { ...this.state.jobs, [id]: { ...job, job_id: id, ended_at, ...card === void 0 ? {} : { card } } } });
      } catch (error) {
        if (generation !== this.generation || statusOf(error) !== 404) return;
        this.cards.delete(id);
        if (!this.state.jobs[id]) return;
        const { [id]: _gone, ...rest } = this.state.jobs;
        this.set({ jobs: rest });
      }
    });
  }
};
function dawPartWorks(state) {
  const setup = state.setup;
  const onlyDawPart = setup?.state === "needs_daw_component" || setup?.state === "ready" && setup.next_step === "install_daw_component";
  if (!onlyDawPart || !state.status?.connected) return false;
  const component = state.status.setup?.component;
  if (!component) return true;
  return component.installed === true && typeof component.version === "string" && component.version === component.bundled;
}
function jobOf(result) {
  let value = result;
  if (typeof value === "string") {
    try {
      value = JSON.parse(value);
    } catch {
      return null;
    }
  }
  return value && typeof value === "object" && "job_id" in value ? plainId(value.job_id) : null;
}
function foundSounds(result) {
  let value = result;
  if (typeof value === "string") {
    try {
      value = JSON.parse(value);
    } catch {
      return null;
    }
  }
  if (!value || typeof value !== "object" || !("items" in value) || !Array.isArray(value.items)) return null;
  const revision = "map_revision" in value && typeof value.map_revision === "string" ? value.map_revision : null;
  const ids = value.items.flatMap(
    (item) => item && typeof item === "object" && "sound_id" in item && typeof item.sound_id === "string" ? [item.sound_id] : []
  );
  return { revision, ids, near: "near" in value ? value.near : null, matches: value.items };
}

// src/shared/hooks.ts
function useKumi(store) {
  return useSyncExternalStore(store.subscribe, store.get, store.get);
}
function useWant(store, part) {
  useEffect(() => store.want(part), [store, part]);
}
var AUDITION_CACHE_CHARS = 48 * 1024 * 1024;
var PREVIEW_SECONDS = 2;
var PREVIEW_FADE_SECONDS = 0.2;
var PREVIEW_LOADS = 2;
var Audition = class {
  constructor(load, limit = AUDITION_CACHE_CHARS) {
    this.load = load;
    this.limit = limit;
  }
  load;
  limit;
  audio = null;
  cache = /* @__PURE__ */ new Map();
  cached = 0;
  loading = /* @__PURE__ */ new Map();
  generation = 0;
  /** Hover previews loading now, and the plays waiting for one of them to end (`PREVIEW_LOADS`). */
  previewLoads = 0;
  waiting = [];
  /** Moves on with every play and stop: a play whose turn has passed never starts its sound. */
  turn = 0;
  /** Whether what plays is a hover's preview: leaving the map stops that, never a sound the producer clicked. */
  previewing = false;
  listeners = /* @__PURE__ */ new Set();
  playing = null;
  subscribe = (fn) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };
  changed(playing) {
    this.playing = playing;
    for (const fn of [...this.listeners]) fn();
  }
  /** The data URL for `id`: from the cache, the load already under way, or the backend. */
  url(id) {
    const cached = this.cache.get(id);
    if (cached) {
      this.cache.delete(id);
      this.cache.set(id, cached);
      return Promise.resolve(cached);
    }
    const loading = this.loading.get(id);
    if (loading) return loading;
    const generation = this.generation;
    const promise = this.load(id).then((reply) => {
      if (typeof reply?.data_url !== "string" || !reply.data_url.startsWith("data:")) throw new Error("Kumi sent no audio for that sound.");
      if (generation === this.generation) this.keep(id, reply.data_url);
      return reply.data_url;
    }).finally(() => {
      if (this.loading.get(id) === promise) this.loading.delete(id);
    });
    this.loading.set(id, promise);
    return promise;
  }
  keep(id, url) {
    if (url.length > this.limit) return;
    this.cache.set(id, url);
    this.cached += url.length;
    for (const [oldest, kept] of this.cache) {
      if (this.cached <= this.limit) break;
      this.cache.delete(oldest);
      this.cached -= kept.length;
    }
  }
  /** Another profile or connection: stop, and forget every sound of the last one. */
  reset() {
    this.stop();
    this.generation += 1;
    this.cache.clear();
    this.cached = 0;
    this.loading.clear();
  }
  /** Play `id` (stopping whatever plays). A later `play` or `stop` takes over even while this one still loads, so
   *  there's never more than one sound playing, and never one that can't be stopped. A play taken over that way ends
   *  quietly, whatever happened to it: only the sound still wanted can fail. `preview` plays a hover's preview: the
   *  first `preview` seconds, faded out, loaded as `previewUrl` allows. */
  async play(id, options = {}) {
    this.stop();
    const turn = this.turn;
    this.previewing = options.preview !== void 0;
    this.changed(id);
    try {
      const url = options.preview === void 0 ? await this.url(id) : await this.previewUrl(id, turn);
      if (url === null || turn !== this.turn) return;
      const audio = new Audio(url);
      this.audio = audio;
      audio.onended = () => {
        if (this.audio === audio) this.stop();
      };
      if (options.preview !== void 0) this.cut(audio, turn, options.preview);
      await audio.play();
    } catch (error) {
      if (turn !== this.turn) return;
      this.stop();
      throw error;
    }
  }
  /** A preview's audio: at once when it's kept or already loading; else loaded once fewer than `PREVIEW_LOADS`
   *  previews are loading. A play whose turn passes while it waits loads nothing (null): the sound the pointer only
   *  crossed. */
  async previewUrl(id, turn) {
    while (!this.cache.has(id) && !this.loading.has(id) && this.previewLoads >= PREVIEW_LOADS) {
      const free = Promise.withResolvers();
      this.waiting.push(free.resolve);
      await free.promise;
      if (turn !== this.turn) return null;
    }
    if (this.cache.has(id) || this.loading.has(id)) return this.url(id);
    this.previewLoads += 1;
    try {
      return await this.url(id);
    } finally {
      this.previewLoads -= 1;
      this.wake();
    }
  }
  /** Every waiting preview looks again: the one still wanted takes the free load, the rest end. */
  wake() {
    const waiting = this.waiting;
    this.waiting = [];
    for (const resolve2 of waiting) resolve2();
  }
  /** A preview's end: past `seconds` less the fade, the volume falls to nothing over the fade, then it stops. */
  cut(audio, turn, seconds2) {
    const fade = PREVIEW_FADE_SECONDS * 1e3;
    setTimeout(() => {
      const start = performance.now();
      const step2 = () => {
        if (turn !== this.turn || this.audio !== audio) return;
        const left = 1 - (performance.now() - start) / fade;
        if (left <= 0) return this.stop();
        audio.volume = left;
        setTimeout(step2, 16);
      };
      step2();
    }, Math.max(0, seconds2 * 1e3 - fade));
  }
  stop() {
    this.turn += 1;
    this.previewing = false;
    if (this.audio) {
      this.audio.pause();
      this.audio.onended = null;
      this.audio = null;
    }
    if (this.waiting.length) this.wake();
    if (this.playing !== null) this.changed(null);
  }
  /** The pointer left the map: a hover's preview stops; a sound the producer clicked plays on. */
  stopPreview() {
    if (this.previewing) this.stop();
  }
  dispose() {
    this.reset();
    this.listeners.clear();
  }
};
function usePlaying(audition) {
  return useSyncExternalStore(audition.subscribe, () => audition.playing, () => audition.playing);
}
function useHoverPlay(audition, enabled, play) {
  const played = useRef(play);
  played.current = play;
  useEffect(() => {
    if (!enabled) audition.stopPreview();
  }, [audition, enabled]);
  return useCallback(
    (id) => {
      if (!enabled) return;
      if (id === null) audition.stopPreview();
      else played.current(id);
    },
    [audition, enabled]
  );
}
function playFailure(error) {
  const name = error instanceof Error ? error.name : "";
  if (name === "NotSupportedError") return "unplayable";
  if (name === "NotAllowedError") return "blocked";
  const status = error && typeof error === "object" && "status" in error ? error.status : statusOf(error);
  return status === 404 ? "gone" : "other";
}

// src/desktop/theme.ts
var T = {
  text: "var(--ui-text-primary)",
  secondary: "var(--ui-text-secondary)",
  muted: "var(--ui-text-tertiary)",
  faint: "var(--ui-text-quaternary)",
  border: "var(--ui-stroke-secondary)",
  card: "var(--ui-bg-card)",
  // An opaque surface for what floats over other views (the Make panel over the map): the card colour is see-through.
  raised: "var(--ui-bg-elevated)",
  chip: "var(--ui-bg-tertiary)",
  accent: "var(--ui-accent)",
  good: "var(--ui-green)",
  warn: "var(--ui-yellow)",
  bad: "var(--ui-red)",
  // Desktop draws no focus rings of its own (styles.css), but its few that do use the theme's primary colour.
  focus: "var(--ui-accent)"
};
var CANVAS = {
  text: "--ui-text-secondary",
  faint: "--ui-text-quaternary",
  border: "--ui-stroke-secondary",
  accent: "--ui-accent",
  strong: "--ui-text-primary"
};

// src/shared/ui.tsx
import { createContext, useContext } from "react";
import { jsx, jsxs } from "react/jsx-runtime";
var I18n = createContext(english);
function useT() {
  return useContext(I18n);
}
var S = {
  section: { display: "flex", flexDirection: "column", gap: 6, padding: "10px 12px", borderBottom: `1px solid ${T.border}` },
  heading: { display: "flex", alignItems: "center", gap: 6, fontSize: 11, letterSpacing: "0.06em", textTransform: "uppercase", color: T.muted },
  row: { display: "flex", alignItems: "center", gap: 6, minWidth: 0 },
  // A message can hold a URL or a path with no place to break: it wraps anywhere rather than widen the pane.
  text: { fontSize: 12.5, color: T.text, lineHeight: 1.45, overflowWrap: "anywhere" },
  muted: { fontSize: 12, color: T.muted, lineHeight: 1.45, overflowWrap: "anywhere" },
  faint: { fontSize: 11.5, color: T.faint },
  ellipsis: { overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", minWidth: 0 },
  // Read by a screen reader, not drawn: hints and live announcements.
  hidden: { position: "absolute", width: 1, height: 1, margin: -1, padding: 0, border: 0, overflow: "hidden", clip: "rect(0 0 0 0)", whiteSpace: "nowrap" }
};
var FOCUSABLE = "kumi-focusable";
function FocusStyle() {
  return /* @__PURE__ */ jsx("style", { children: `.${FOCUSABLE}:focus-visible{outline:2px solid ${T.focus};outline-offset:2px;border-radius:3px}@keyframes kumi-ants{to{stroke-dashoffset:-18}}.kumi-ants{animation:kumi-ants .7s linear infinite}@keyframes kumi-pop{0%{transform:scale(.55) translateY(6px);opacity:0}70%{transform:scale(1.06);opacity:1}100%{transform:none;opacity:1}}.kumi-pop{animation:kumi-pop .34s cubic-bezier(.2,.8,.3,1) both}@keyframes kumi-spin{to{transform:rotate(360deg)}}.kumi-spin{display:inline-block;animation:kumi-spin .8s linear infinite}@media (prefers-reduced-motion:reduce){.kumi-ants,.kumi-pop{animation:none}.kumi-spin{animation-duration:2.4s}}` });
}
function Chip({ children, title, tone }) {
  const color = tone === "accent" ? T.accent : tone === "good" ? T.good : tone === "warn" ? T.warn : tone === "bad" ? T.bad : T.secondary;
  return /* @__PURE__ */ jsx("span", { title, style: { fontSize: 11, lineHeight: "16px", padding: "0 6px", borderRadius: 3, background: T.chip, color, whiteSpace: "nowrap" }, children });
}
function Dot({ color, size = 7, title }) {
  return /* @__PURE__ */ jsx("span", { "aria-hidden": !title, title, style: { display: "inline-block", width: size, height: size, borderRadius: "50%", background: color, flex: "none" } });
}
function TextButton({
  children,
  onClick,
  title,
  label,
  pressed,
  disabled,
  strong
}) {
  return /* @__PURE__ */ jsx(
    "button",
    {
      "aria-label": label,
      "aria-pressed": pressed,
      className: FOCUSABLE,
      disabled,
      onClick,
      title,
      type: "button",
      style: {
        font: "inherit",
        fontSize: 11.5,
        lineHeight: "18px",
        padding: "0 7px",
        borderRadius: 3,
        // A toggle that's on reads as on, not only to a screen reader (hover to play, Select, Strict or Loose).
        border: `1px solid ${strong || pressed ? T.accent : T.border}`,
        background: pressed ? `color-mix(in srgb, ${T.accent} 14%, transparent)` : "transparent",
        color: disabled ? T.faint : strong || pressed ? T.accent : T.secondary,
        cursor: disabled ? "default" : "pointer",
        whiteSpace: "nowrap",
        flex: "none",
        transition: "background-color 120ms, border-color 120ms, color 120ms"
      },
      children
    }
  );
}
function Section({ title, right, children }) {
  return /* @__PURE__ */ jsxs("section", { style: S.section, children: [
    /* @__PURE__ */ jsxs("div", { style: { ...S.heading, justifyContent: "space-between" }, children: [
      /* @__PURE__ */ jsx("h3", { style: { ...S.row, gap: 6, margin: 0, fontSize: "inherit", fontWeight: "inherit" }, children: title }),
      right
    ] }),
    children
  ] });
}

// src/desktop/actions.ts
import { host } from "@hermes/plugin-sdk";
function wordsOf(ctx) {
  return ctx.i18n?.t ?? english;
}
var OPEN_WAIT_MS = 1e3;
var OPEN_POLL_MS = 100;
var NEW_CHAT = "/";
async function ask(ctx, text, options) {
  const t = wordsOf(ctx);
  const composer = host.composer;
  const session = options.session ?? null;
  if (!composer) {
    const copied = await ctx.os?.writeClipboard?.(text) ?? false;
    host.notify({
      kind: copied ? "info" : "warning",
      message: copied ? t("notify.copied") : t("notify.askInChat", text)
    });
    return { outcome: copied ? "copied" : "none" };
  }
  if (!session) {
    if (await composer.insertText(null, text) || await inNewChat(ctx, composer, text)) return { outcome: "drafted" };
    host.notify({ kind: "warning", message: t("notify.openChat") });
    return { outcome: "none" };
  }
  const deliver = async () => options.send ? composer.submit(session, text) : composer.insertText(session, text);
  if (await deliver()) return { outcome: options.send ? "sent" : "drafted" };
  if (typeof host.openSession === "function") {
    try {
      await host.openSession(session);
      for (let waited = 0; waited <= OPEN_WAIT_MS; waited += OPEN_POLL_MS) {
        if (await deliver()) return { outcome: options.send ? "sent" : "drafted" };
        const tick = Promise.withResolvers();
        ctx.setTimeout(tick.resolve, OPEN_POLL_MS);
        await tick.promise;
      }
    } catch {
    }
  }
  if (await composer.insertText(null, text)) {
    host.notify({ kind: "warning", message: t("notify.notOpenHere") });
    return { outcome: "drafted" };
  }
  host.notify({ kind: "warning", message: t("notify.openChat") });
  return { outcome: "none" };
}
async function inNewChat(ctx, composer, text) {
  try {
    host.navigate(NEW_CHAT);
  } catch {
    return false;
  }
  for (let waited = 0; waited < OPEN_WAIT_MS; waited += OPEN_POLL_MS) {
    const tick = Promise.withResolvers();
    ctx.setTimeout(tick.resolve, OPEN_POLL_MS);
    await tick.promise;
    if (await composer.insertText(null, text)) return true;
  }
  return false;
}

// src/desktop/card.tsx
import { useEffect as useEffect2, useState } from "react";
import { Fragment, jsx as jsx2, jsxs as jsxs2 } from "react/jsx-runtime";
async function peaksOf(dataUrl, buckets = 96) {
  const comma = dataUrl.indexOf(",");
  const binary = atob(dataUrl.slice(comma + 1));
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  const decoded = await new OfflineAudioContext(1, 1, 44100).decodeAudioData(bytes.buffer);
  const samples = decoded.getChannelData(0);
  const step2 = Math.max(1, Math.floor(samples.length / buckets));
  const peaks = [];
  for (let start = 0; start < samples.length && peaks.length < buckets; start += step2) {
    let peak = 0;
    for (let i = start; i < Math.min(samples.length, start + step2); i++) peak = Math.max(peak, Math.abs(samples[i]));
    peaks.push(peak);
  }
  const top = Math.max(...peaks, 1e-6);
  return peaks.map((peak) => peak / top);
}
function Wave({ audition, id, label, caption = label }) {
  const t = useT();
  const [peaks, setPeaks] = useState(null);
  const [failed, setFailed] = useState(false);
  const playing = usePlaying(audition) === id;
  useEffect2(() => {
    let live = true;
    audition.url(id).then(peaksOf).then((found) => live && setPeaks(found)).catch(() => live && setFailed(true));
    return () => {
      live = false;
    };
  }, [audition, id]);
  return /* @__PURE__ */ jsxs2("div", { style: { display: "flex", flexDirection: "column", gap: 3, flex: "1 1 160px", minWidth: 140 }, children: [
    /* @__PURE__ */ jsxs2("div", { style: { ...S.row, justifyContent: "space-between" }, children: [
      /* @__PURE__ */ jsx2("span", { style: { ...S.faint, ...S.ellipsis }, children: caption }),
      /* @__PURE__ */ jsx2(
        TextButton,
        {
          label: playing ? t("listen.stop", label) : t("listen.play", label),
          onClick: () => playing ? audition.stop() : void audition.play(id).catch(() => setFailed(true)),
          pressed: playing,
          children: playing ? "■" : "▶"
        }
      )
    ] }),
    /* @__PURE__ */ jsxs2("svg", { "aria-label": t("listen.waveform", label), height: 36, preserveAspectRatio: "none", role: "img", viewBox: "0 0 96 36", width: "100%", children: [
      (peaks ?? []).map((peak, index) => /* @__PURE__ */ jsx2("rect", { fill: playing ? T.accent : T.secondary, height: Math.max(1, peak * 34), opacity: 0.85, width: 0.7, x: index + 0.15, y: 18 - Math.max(0.5, peak * 17) }, index)),
      !peaks && /* @__PURE__ */ jsx2("line", { stroke: T.border, x1: 0, x2: 96, y1: 18, y2: 18 })
    ] }),
    failed && /* @__PURE__ */ jsx2("span", { style: S.faint, children: t("listen.audioGone") })
  ] });
}
function Rows({ view }) {
  const t = useT();
  const compared = view.referenceTitle !== void 0;
  const cell = { padding: "2px 6px", textAlign: "right", fontVariantNumeric: "tabular-nums", whiteSpace: "nowrap" };
  return (
    // The card sits in the chat's markdown, whose prose styles reach any table in it: a 2em margin above and below,
    // a taller line, and a bright line under each row and the header. The table sets its own, so only its rows' quiet
    // lines show. In a narrow chat (the map docked beside it) the columns don't fit: the table scrolls sideways in its
    // own region, focusable so the keyboard can scroll it, rather than spilling past the card, where the chat cut off
    // the reference and the differences.
    /* @__PURE__ */ jsx2("div", { "aria-label": t("measurements.tab"), className: FOCUSABLE, role: "region", style: { overflowX: "auto", maxWidth: "100%" }, tabIndex: 0, children: /* @__PURE__ */ jsxs2("table", { style: { borderCollapse: "collapse", fontSize: 12, lineHeight: 1.45, color: T.text, width: "100%", margin: 0 }, children: [
      compared && /* @__PURE__ */ jsx2("thead", { style: { border: "none" }, children: /* @__PURE__ */ jsxs2("tr", { style: { color: T.muted, fontSize: 11, border: "none" }, children: [
        /* @__PURE__ */ jsx2("th", { style: { ...cell, textAlign: "left", fontWeight: 400 } }),
        /* @__PURE__ */ jsx2("th", { style: { ...cell, fontWeight: 400 }, children: t("listen.this") }),
        /* @__PURE__ */ jsx2("th", { style: { ...cell, fontWeight: 400 }, children: t("listen.reference") }),
        /* @__PURE__ */ jsx2("th", { style: { ...cell, fontWeight: 400 }, children: t("listen.difference") })
      ] }) }),
      /* @__PURE__ */ jsx2("tbody", { children: view.rows.map((row2) => /* @__PURE__ */ jsxs2("tr", { style: { border: "none", borderTop: `1px solid ${T.border}` }, children: [
        /* @__PURE__ */ jsx2("td", { style: { ...cell, textAlign: "left", color: T.muted }, children: row2.label }),
        /* @__PURE__ */ jsx2("td", { style: cell, children: row2.mine }),
        compared && /* @__PURE__ */ jsx2("td", { style: { ...cell, color: T.muted }, children: row2.reference ?? "—" }),
        compared && /* @__PURE__ */ jsx2("td", { style: { ...cell, color: row2.direction === 0 ? T.muted : T.text, fontWeight: 600 }, children: row2.delta ?? "—" })
      ] }, row2.label)) })
    ] }) })
  );
}
function Overlay({ view }) {
  const t = useT();
  const values = view.overlay.flatMap((band) => [band.mine, band.reference]);
  const low = Math.min(...values) - 3;
  const high = Math.max(...values) + 1;
  const at = (value) => `${Math.max(2, (value - low) / (high - low) * 100)}%`;
  return /* @__PURE__ */ jsxs2("div", { style: { display: "flex", flexDirection: "column", gap: 3 }, children: [
    /* @__PURE__ */ jsxs2("span", { style: { ...S.row, gap: 10, flexWrap: "wrap" }, children: [
      /* @__PURE__ */ jsx2("span", { style: S.faint, children: t("listen.overlay") }),
      /* @__PURE__ */ jsxs2("span", { style: { ...S.row, gap: 4, ...S.faint }, children: [
        /* @__PURE__ */ jsx2("span", { "aria-hidden": true, style: { width: 8, height: 8, background: T.accent, opacity: 0.8 } }),
        t("listen.this")
      ] }),
      /* @__PURE__ */ jsxs2("span", { style: { ...S.row, gap: 4, ...S.faint }, children: [
        /* @__PURE__ */ jsx2("span", { "aria-hidden": true, style: { width: 10, borderTop: `2px solid ${T.text}` } }),
        t("listen.reference")
      ] })
    ] }),
    /* @__PURE__ */ jsx2("div", { style: { display: "flex", alignItems: "stretch", gap: 3, height: 64 }, children: view.overlay.map((band) => {
      const label = t("listen.overlayBand", band.name, band.hz, num(band.mine), num(band.reference), signed(band.mine - band.reference));
      return /* @__PURE__ */ jsxs2("div", { role: "img", "aria-label": label, title: label, style: { flex: 1, display: "flex", flexDirection: "column", alignItems: "center", gap: 2, minWidth: 0 }, children: [
        /* @__PURE__ */ jsxs2("div", { style: { position: "relative", flex: 1, width: "100%" }, children: [
          /* @__PURE__ */ jsx2("div", { style: { position: "absolute", left: "18%", right: "18%", bottom: 0, height: at(band.mine), background: T.accent, opacity: 0.8 } }),
          /* @__PURE__ */ jsx2("div", { style: { position: "absolute", left: 0, right: 0, bottom: at(band.reference), borderTop: `2px solid ${T.text}` } })
        ] }),
        /* @__PURE__ */ jsx2("span", { style: { fontSize: 9, color: T.faint, ...S.ellipsis, maxWidth: "100%" }, children: band.name })
      ] }, band.name);
    }) })
  ] });
}
function Bands({ view }) {
  const t = useT();
  if (view.overlay.length) return /* @__PURE__ */ jsx2(Overlay, { view });
  if (!view.bands.length) return null;
  const top = Math.max(...view.bands.map((band) => Math.abs(band.value)), view.bandsAre === "difference" ? 3 : 1);
  return /* @__PURE__ */ jsxs2("div", { style: { display: "flex", flexDirection: "column", gap: 3 }, children: [
    /* @__PURE__ */ jsx2("span", { style: S.faint, children: view.bandsAre === "difference" ? t("listen.bandsDifference") : t("listen.bandsLevel") }),
    /* @__PURE__ */ jsx2("div", { style: { display: "flex", alignItems: "stretch", gap: 3, height: 54 }, children: view.bands.map((band) => {
      const share = Math.min(1, Math.abs(band.value) / top);
      const label = view.bandsAre === "difference" ? `${band.name}: ${signed(band.value)} dB` : `${band.name}: ${num(band.value)} dB`;
      return /* @__PURE__ */ jsxs2("div", { style: { flex: 1, display: "flex", flexDirection: "column", alignItems: "center", gap: 2, minWidth: 0 }, title: label, children: [
        /* @__PURE__ */ jsxs2("div", { style: { position: "relative", flex: 1, width: "100%" }, children: [
          view.bandsAre === "difference" ? /* @__PURE__ */ jsx2(
            "div",
            {
              style: {
                position: "absolute",
                left: "15%",
                right: "15%",
                background: band.value < 0 ? T.secondary : T.accent,
                opacity: 0.8,
                ...band.value < 0 ? { top: "50%", height: `${share * 50}%` } : { bottom: "50%", height: `${share * 50}%` }
              }
            }
          ) : /* @__PURE__ */ jsx2("div", { style: { position: "absolute", left: "15%", right: "15%", bottom: 0, height: `${Math.max(4, (1 - share) * 100)}%`, background: T.secondary, opacity: 0.8 } }),
          view.bandsAre === "difference" && /* @__PURE__ */ jsx2("div", { style: { position: "absolute", left: 0, right: 0, top: "50%", borderTop: `1px solid ${T.border}` } })
        ] }),
        /* @__PURE__ */ jsx2("span", { style: { fontSize: 9, color: T.faint, ...S.ellipsis, maxWidth: "100%" }, children: band.name })
      ] }, band.name);
    }) })
  ] });
}
function Interpretation({ view }) {
  const t = useT();
  const [open, setOpen] = useState(false);
  if (view.interpretationError) return /* @__PURE__ */ jsx2("span", { style: S.faint, children: t("listen.noInterpretation", view.interpretationError) });
  if (!view.interpretation) return null;
  const text = view.interpretation.text;
  const long = text.length > 320;
  return /* @__PURE__ */ jsxs2("div", { style: { display: "flex", flexDirection: "column", gap: 3, borderLeft: `2px solid ${T.border}`, paddingLeft: 8 }, children: [
    /* @__PURE__ */ jsx2("span", { style: S.faint, children: view.interpretation.label }),
    /* @__PURE__ */ jsx2("span", { style: { ...S.muted, whiteSpace: "pre-wrap" }, children: long && !open ? `${text.slice(0, 320)}…` : text }),
    long && /* @__PURE__ */ jsx2("span", { children: /* @__PURE__ */ jsx2(TextButton, { onClick: () => setOpen(!open), children: open ? t("listen.less") : t("listen.more") }) })
  ] });
}
function ReferenceFacts({ reference }) {
  const identity = reference.origin?.identity ?? reference.identity;
  const file = identity?.file_identity;
  const facts = file && typeof file === "object" ? Object.entries(file).filter(
    ([key, value]) => ["dev", "ino", "size", "mtime_ns"].includes(key) && (typeof value === "number" || typeof value === "string")
  ).map(([key, value]) => `${key}=${clean(String(value), 60)}`).join(" · ") : "";
  const original = reference.origin?.window;
  const measured = windowWords(reference.window);
  return /* @__PURE__ */ jsxs2("div", { style: { display: "flex", flexDirection: "column", gap: 3, overflowWrap: "anywhere" }, children: [
    identity && /* @__PURE__ */ jsxs2("span", { style: S.faint, children: [
      "Identity: ",
      clean(identity.kind ?? "", 20),
      " ",
      facts,
      identity.sound_id && ` · sound ${clean(identity.sound_id.split(/[\\/]/).pop() ?? "", 80)}`,
      identity.map_revision && ` · map ${clean(identity.map_revision, 80)}`,
      identity.artifact_id && ` · artifact ${clean(identity.artifact_id, 80)}`
    ] }),
    original && /* @__PURE__ */ jsxs2("span", { style: S.faint, children: [
      "Original provenance · ",
      windowWords(original)
    ] }),
    reference.artifact_id && /* @__PURE__ */ jsxs2("span", { style: S.faint, children: [
      "Retained artifact: ",
      clean(reference.artifact_id, 100)
    ] }),
    measured && /* @__PURE__ */ jsxs2("span", { style: S.faint, children: [
      original ? "Artifact measured window · " : "",
      measured
    ] }),
    reference.window && /* @__PURE__ */ jsxs2("span", { style: S.faint, children: [
      "Retained artifact playback starts at ",
      num(reference.window.replay_from, 6),
      " s"
    ] })
  ] });
}
function ReferenceProfile({ result, audition }) {
  const t = useT();
  const profile = result.reference_profile;
  if (!profile) return null;
  const cell = { padding: "5px 6px", verticalAlign: "top", textAlign: "left" };
  return /* @__PURE__ */ jsxs2("section", { "aria-label": "Measured reference profile", style: { display: "flex", flexDirection: "column", gap: 9 }, children: [
    /* @__PURE__ */ jsx2("strong", { style: S.text, children: "Reference target and spread" }),
    /* @__PURE__ */ jsxs2("span", { style: S.muted, children: [
      profile.usable_references,
      "/",
      profile.references,
      " usable references · equal per-reference weighting"
    ] }),
    /* @__PURE__ */ jsx2("span", { style: S.faint, children: clean(profile.basis, 500) }),
    profile.source_window && /* @__PURE__ */ jsxs2("span", { style: S.faint, children: [
      "Source measured window · ",
      windowWords(profile.source_window)?.replace("Reference file seconds", "seconds")
    ] }),
    /* @__PURE__ */ jsx2("div", { style: { overflowX: "auto" }, children: /* @__PURE__ */ jsxs2("table", { style: { borderCollapse: "collapse", fontSize: 12, lineHeight: 1.45, width: "100%", margin: 0 }, children: [
      /* @__PURE__ */ jsx2("thead", { children: /* @__PURE__ */ jsx2("tr", { style: { color: T.muted }, children: ["Quantity / basis", "Measured source", "Target [spread]", "Source − target", "Contributors / total"].map((label) => /* @__PURE__ */ jsx2("th", { style: { ...cell, fontWeight: 400 }, children: label }, label)) }) }),
      /* @__PURE__ */ jsx2("tbody", { children: profile.quantities.map((quantity) => {
        const value = (number) => typeof number === "number" && Number.isFinite(number) ? `${num(number, 2)} ${clean(quantity.unit, 30)}` : "—";
        return /* @__PURE__ */ jsxs2("tr", { style: { borderTop: `1px solid ${T.border}` }, children: [
          /* @__PURE__ */ jsxs2("th", { scope: "row", style: { ...cell, fontWeight: 400 }, children: [
            clean(quantity.quantity, 80),
            /* @__PURE__ */ jsx2("div", { style: S.faint, children: clean(quantity.basis, 300) })
          ] }),
          /* @__PURE__ */ jsxs2("td", { style: cell, children: [
            value(quantity.source),
            quantity.source === null && /* @__PURE__ */ jsx2("div", { style: S.faint, children: clean(quantity.source_reason_detail ?? quantity.source_reason ?? "not measured", 300) })
          ] }),
          /* @__PURE__ */ jsx2("td", { style: cell, children: quantity.target ? /* @__PURE__ */ jsxs2(Fragment, { children: [
            value(quantity.target.mid),
            /* @__PURE__ */ jsxs2("div", { style: S.faint, children: [
              "[",
              value(quantity.target.low),
              " … ",
              value(quantity.target.high),
              "]"
            ] })
          ] }) : /* @__PURE__ */ jsxs2(Fragment, { children: [
            "—",
            /* @__PURE__ */ jsx2("div", { style: S.faint, children: clean(quantity.target_reason ?? "not measured", 100) })
          ] }) }),
          /* @__PURE__ */ jsxs2("td", { style: cell, children: [
            quantity.deviation !== null && Number.isFinite(quantity.deviation) ? `${signed(quantity.deviation, 2)} ${quantity.unit === "LUFS" ? "LU" : clean(quantity.unit, 30)}` : "—",
            quantity.inside_spread !== null && /* @__PURE__ */ jsx2("div", { style: S.faint, children: quantity.inside_spread ? "Inside spread" : "Outside spread" })
          ] }),
          /* @__PURE__ */ jsxs2("td", { style: cell, children: [
            quantity.contributors,
            "/",
            quantity.references
          ] })
        ] }, quantity.quantity);
      }) })
    ] }) }),
    /* @__PURE__ */ jsx2("span", { style: S.faint, children: clean(profile.range_note, 500) }),
    /* @__PURE__ */ jsx2("span", { style: S.faint, children: clean(profile.peak_note, 500) }),
    (result.references ?? []).map((reference, index) => {
      const name = displayReferenceName(reference);
      const availability = reference.availability;
      const canonical = listenView({ metrics: reference.metrics }, t);
      const unavailable = /* @__PURE__ */ new Map();
      for (const quantity of reference.profile_quantities ?? []) {
        if (quantity.value !== null) continue;
        const reason = clean(quantity.reason ?? quantity.reason_code ?? "not measured", 300);
        const quantities = unavailable.get(reason) ?? [];
        quantities.push(clean(quantity.quantity, 80));
        unavailable.set(reason, quantities);
      }
      return /* @__PURE__ */ jsxs2("section", { "aria-label": `Reference ${index + 1}: ${name}`, style: { display: "flex", flexDirection: "column", gap: 5, borderTop: `1px solid ${T.border}`, paddingTop: 8 }, children: [
        /* @__PURE__ */ jsxs2("strong", { style: S.text, children: [
          index + 1,
          ". ",
          name
        ] }),
        /* @__PURE__ */ jsx2(ReferenceFacts, { reference }),
        reference.artifact_id && /* @__PURE__ */ jsx2(Wave, { audition, id: reference.artifact_id, label: name }),
        unavailable.size > 0 && /* @__PURE__ */ jsx2("ul", { "aria-label": `${name}: unavailable profile quantities`, style: { margin: 0, paddingLeft: 16, ...S.faint }, children: [...unavailable].map(([reason, quantities]) => /* @__PURE__ */ jsxs2("li", { children: [
          quantities.join(", "),
          ": — · ",
          reason
        ] }, reason)) }),
        reference.profile_quantities && /* @__PURE__ */ jsxs2("details", { children: [
          /* @__PURE__ */ jsxs2("summary", { className: FOCUSABLE, style: { ...S.muted, cursor: "pointer" }, children: [
            t("listen.more"),
            " · Each reference's measurements"
          ] }),
          /* @__PURE__ */ jsx2("div", { style: { overflowX: "auto", marginTop: 5 }, children: /* @__PURE__ */ jsxs2("table", { "aria-label": `${name}: reference measurements`, style: { borderCollapse: "collapse", fontSize: 12, lineHeight: 1.45, width: "100%", margin: 0 }, children: [
            /* @__PURE__ */ jsx2("caption", { style: { ...S.faint, textAlign: "left" }, children: "Each reference's measurements · only covered regions contribute to the target" }),
            /* @__PURE__ */ jsx2("thead", { children: /* @__PURE__ */ jsx2("tr", { style: { color: T.muted }, children: ["Quantity / basis", "Measured reference", "Unavailable reason"].map((label) => /* @__PURE__ */ jsx2("th", { style: { ...cell, fontWeight: 400 }, children: label }, label)) }) }),
            /* @__PURE__ */ jsx2("tbody", { children: reference.profile_quantities.map((quantity) => /* @__PURE__ */ jsxs2("tr", { style: { borderTop: `1px solid ${T.border}` }, children: [
              /* @__PURE__ */ jsxs2("th", { scope: "row", style: { ...cell, fontWeight: 400 }, children: [
                clean(quantity.quantity, 80),
                /* @__PURE__ */ jsx2("div", { style: S.faint, children: clean(quantity.basis, 300) })
              ] }),
              /* @__PURE__ */ jsx2("td", { style: cell, children: typeof quantity.value === "number" && Number.isFinite(quantity.value) ? `${num(quantity.value, 2)} ${clean(quantity.unit, 30)}` : "—" }),
              /* @__PURE__ */ jsx2("td", { style: { ...cell, color: T.muted }, children: quantity.value === null ? clean(quantity.reason ?? "not measured", 300) : "—" })
            ] }, quantity.quantity)) })
          ] }) })
        ] }),
        /* @__PURE__ */ jsx2("span", { style: S.faint, children: "Standard measurements · a different meter from the reference target" }),
        canonical.rows.length ? /* @__PURE__ */ jsx2(Rows, { view: canonical }) : /* @__PURE__ */ jsx2("span", { style: S.faint, children: "Standard measurements unavailable" }),
        availability && /* @__PURE__ */ jsxs2("span", { style: S.faint, children: [
          "Profile availability: ",
          availability.profile_reason ? clean(availability.profile_reason, 100) : `${availability.loud_frame_count} loud frames`,
          availability.energy_zero && " · zero energy",
          availability.fixed_region_tolerance && " · fixed region tolerance, not observed variation",
          !reference.profile_quantities && availability.unavailable.map((missing) => ` · ${clean(missing.quantity, 60)}: ${clean(missing.reason, 100)}`).join("")
        ] })
      ] }, reference.artifact_id ?? index);
    })
  ] });
}
function ListenCard({ media, audition, store }) {
  if (media.file?.kind !== "audio") return media.fallback;
  return /* @__PURE__ */ jsx2(ListenResultCard, { audition, fallback: media.fallback, result: media.result, store });
}
function ListenResultCard({ result, audition, fallback, store }) {
  const t = useT();
  const { rounds } = useKumi(store);
  const parsed = parseListen(result);
  if (!parsed) return fallback;
  const view = listenView(parsed, t, referenceName(parsed, rounds));
  const key = roundKey(parsed, rounds);
  const seen = key ? rounds[key] ?? [] : [];
  const index = seen.findIndex((round2) => round2.id === parsed.artifact_id);
  const round = index < 0 ? void 0 : roundWords(index + 1, seen.slice(Math.max(0, index - 2), index + 1).map((each) => each.gaps), t);
  return /* @__PURE__ */ jsxs2(
    "div",
    {
      "data-kumi-card": "listen",
      style: { display: "flex", flexDirection: "column", gap: 9, maxWidth: 560, padding: "10px 12px", borderRadius: 6, border: `1px solid ${T.border}`, background: T.card, color: T.text },
      children: [
        /* @__PURE__ */ jsxs2("div", { style: { ...S.row, justifyContent: "space-between", alignItems: "flex-start", gap: 8 }, children: [
          /* @__PURE__ */ jsxs2("span", { style: { ...S.text, fontWeight: 600, minWidth: 0 }, children: [
            /* @__PURE__ */ jsx2("span", { style: { color: T.accent }, children: "♪" }),
            " ",
            view.title,
            view.referenceTitle && /* @__PURE__ */ jsx2("span", { style: { color: T.muted, fontWeight: 400 }, children: t("listen.versus", view.referenceTitle) })
          ] }),
          /* @__PURE__ */ jsx2(Chip, { title: t("listen.measuredTitle"), children: t("listen.measured") })
        ] }),
        view.capture && /* @__PURE__ */ jsx2("span", { style: S.faint, title: t("listen.captureTitle"), children: view.capture }),
        view.captureNote && /* @__PURE__ */ jsx2("span", { style: { ...S.muted, whiteSpace: "normal" }, children: view.captureNote }),
        view.verdict && /* @__PURE__ */ jsx2("span", { style: S.text, children: view.verdict }),
        round && /* @__PURE__ */ jsx2("span", { style: { ...S.muted, fontVariantNumeric: "tabular-nums" }, title: t("listen.roundTitle"), children: round }),
        view.mineId ? /* @__PURE__ */ jsxs2("div", { style: { ...S.row, gap: 12, flexWrap: "wrap", alignItems: "flex-start" }, children: [
          /* @__PURE__ */ jsx2(Wave, { audition, id: view.mineId, label: view.title }),
          view.referenceId && /* @__PURE__ */ jsx2(Wave, { audition, id: view.referenceId, label: view.referenceTitle ?? t("listen.reference") })
        ] }) : fallback,
        parsed.compare && (parsed.compare.window || parsed.compare.origin || parsed.compare.identity) && /* @__PURE__ */ jsx2(ReferenceFacts, { reference: parsed.compare }),
        parsed.reference_profile && /* @__PURE__ */ jsx2("span", { style: S.faint, children: "Source standard measurements · a different meter from the reference target" }),
        /* @__PURE__ */ jsx2(Rows, { view }),
        /* @__PURE__ */ jsx2(Bands, { view }),
        /* @__PURE__ */ jsx2(ReferenceProfile, { result: parsed, audition }),
        view.headlines.length > 0 && /* @__PURE__ */ jsx2("ul", { style: { margin: 0, paddingLeft: 16, display: "flex", flexDirection: "column", gap: 1 }, children: view.headlines.map((line, index2) => /* @__PURE__ */ jsx2("li", { style: S.muted, children: line }, index2)) }),
        /* @__PURE__ */ jsx2(Interpretation, { view })
      ]
    }
  );
}

// src/shared/generate.ts
var WINDOWED = /* @__PURE__ */ new Set(["video_to_sfx", "extend", "inpaint"]);
function timing(value) {
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) return null;
  return Math.abs(value * 1e3 - Math.round(value * 1e3)) < 1e-6 ? value : null;
}
var ms = (value) => Math.round(value * 1e3);
function windowOf(kind, result) {
  const source = timing(result.source_seconds);
  const billed = timing(result.seconds);
  if (source === null || source === 0 || billed === null || billed === 0) return null;
  if (kind === "video_to_sfx") return billed <= source ? { kind, seconds: billed, source } : null;
  if (kind === "extend") {
    const append = timing(result.append_seconds);
    return append !== null && ms(append) === ms(billed) ? { kind, append, source } : null;
  }
  if (kind === "inpaint") {
    const start = timing(result.region_start_seconds);
    const end = timing(result.region_end_seconds);
    if (start === null || end === null || !(start < end) || end > source || ms(end) - ms(start) !== ms(billed)) return null;
    return { kind, start, end, source };
  }
  return null;
}
var STATES = /* @__PURE__ */ new Set(["running", "done", "failed", "cancelled", "interrupted"]);
function objectOf(value) {
  let found = value;
  if (typeof found === "string") {
    try {
      found = JSON.parse(found);
    } catch {
      return null;
    }
  }
  return found && typeof found === "object" && !Array.isArray(found) ? found : null;
}
var credits = (value) => typeof value === "number" && Number.isInteger(value) && value >= 0 ? value : null;
var positive = (value) => typeof value === "number" && Number.isFinite(value) && value > 0 ? value : null;
function ledgerOf(value) {
  const ledger = objectOf(value);
  if (!ledger) return null;
  const [cap, spent, reserved, left] = [ledger.cap, ledger.spent, ledger.reserved, ledger.left].map(credits);
  if (cap === null || spent === null || reserved === null || left === null) return null;
  return { cap, spent, reserved, left, frozen: ledger.frozen === true };
}
function parseGeneration(value) {
  const outer = objectOf(value);
  if (!outer || plainId(outer.job_id) === null) return null;
  const envelope = outer.kind === "generate";
  const result = envelope ? objectOf(outer.result) ?? {} : outer;
  const state = typeof outer.state === "string" && STATES.has(outer.state) ? outer.state : null;
  if (!state || !envelope && (state !== "running" || plainId(outer.quote_id) === null)) return null;
  const spent = objectOf(result.credits);
  const estimate = objectOf(result.estimate);
  const error = objectOf(outer.error);
  const progress = objectOf(outer.progress);
  const notes = result.note_count;
  const kind = typeof result.kind === "string" ? clean(result.kind, 24) : "";
  return {
    state,
    kind,
    prompt: typeof result.prompt === "string" ? clean(result.prompt, 200) : "",
    artifactId: plainId(result.artifact_id),
    file: typeof result.file === "string" ? clean(result.file.split(/[\\/]/).pop() ?? "", 80) : "",
    outputType: typeof result.output_type === "string" ? clean(result.output_type, 16) : null,
    seconds: positive(result.seconds),
    window: WINDOWED.has(kind) ? windowOf(kind, result) : null,
    noteCount: typeof notes === "number" && Number.isInteger(notes) && notes >= 0 ? notes : null,
    estimate: credits(spent?.estimate ?? estimate?.credits),
    reserved: credits(spent?.reserved ?? estimate?.max_credits),
    actual: credits(spent?.actual),
    // A raw job snapshot has no ledger: then none is shown, rather than an older one.
    ledger: ledgerOf(result.ledger ?? outer.ledger),
    error: error ? { code: typeof error.code === "string" ? clean(error.code, 40) : "", reason: typeof error.reason === "string" ? clean(error.reason, 200) : "" } : null,
    submission: typeof result.submission === "string" ? clean(result.submission, 24) : "",
    message: typeof progress?.message === "string" ? clean(progress.message, 60) : ""
  };
}
function neverSent(made) {
  if (made.state === "running") return false;
  return made.submission ? made.submission === "not_submitted" : made.error?.code === "not_submitted";
}
function costWords(made, t = english) {
  const amount = (n) => t("generate.credits", n, num(n, 0));
  if (made.actual !== null) {
    return made.estimate === null || made.estimate === made.actual ? amount(made.actual) : t("generate.cost", amount(made.actual), num(made.estimate, 0));
  }
  if (neverSent(made)) return t("generate.notSubmitted");
  const held = made.reserved ?? made.estimate;
  if (held === null) return "";
  return made.state === "running" ? t("generate.reserved", amount(held)) : t("generate.unknownCharge", amount(held));
}
function capWords(made, t = english) {
  return made.ledger ? t("generate.left", num(made.ledger.left, 0), num(made.ledger.cap, 0)) : "";
}
var NOTES = /* @__PURE__ */ new Set(["midi", "notes", "musicxml"]);
function outputKind(made) {
  if (made.outputType === null) return made.kind === "sfx" || WINDOWED.has(made.kind) ? "audio" : made.kind === "audio_to_midi" ? "notes" : null;
  if (made.outputType === "audio") return "audio";
  return NOTES.has(made.outputType) ? "notes" : null;
}
function generationTitle(made, t = english) {
  if (made.prompt) return made.prompt;
  if (WINDOWED.has(made.kind)) return t(`generate.kinds.${made.kind}`);
  return outputKind(made) === "notes" ? t("generate.notesTitle") : t("generate.soundTitle");
}
var clock = (value) => `${Math.floor(value / 60)}:${String(Math.round(value % 60)).padStart(2, "0")}`;
var span = (from, to) => to < 60 ? `${num(from, 2)}–${seconds(to)}` : `${clock(from)}–${clock(to)}`;
function lengthWords(made, t = english) {
  const covered = made.window;
  if (covered?.kind === "video_to_sfx") return t("generate.window.video_to_sfx", seconds(covered.seconds));
  if (covered?.kind === "extend") return t("generate.window.extend", seconds(covered.append), seconds(covered.source));
  if (covered?.kind === "inpaint") return t("generate.window.inpaint", span(covered.start, covered.end), seconds(covered.source));
  return WINDOWED.has(made.kind) || made.seconds === null ? "" : seconds(made.seconds);
}
function generationJobWords(job, t = english) {
  const words = jobWords(job, t);
  const made = parseGeneration({ ...job, kind: "generate" });
  if (!made) return words;
  const frozen = (made.ledger ?? parseGeneration(job.card)?.ledger)?.frozen === true;
  const over = made.actual !== null && made.reserved !== null && made.actual > made.reserved;
  const reason = made.state === "failed" || made.state === "interrupted" ? made.error?.reason ?? "" : "";
  const covered = made.window ? lengthWords(made, t) : "";
  const detail = [made.state === "running" ? made.message : made.file, covered, reason, costWords(made, t), frozen ? t("generate.frozen") : ""];
  return { ...words, detail: detail.filter(Boolean).join(" · "), tone: (frozen || over) && words.tone === "good" ? "warn" : words.tone };
}
function generationRequest(made, t = english) {
  if (made.state !== "done" || !made.artifactId) return null;
  const kind = outputKind(made);
  if (kind === "notes") {
    const request2 = planNotesRequest(made.artifactId, made.noteCount);
    return request2 ? { label: t("generate.planNotes"), title: t("generate.planNotesTitle"), request: request2 } : null;
  }
  if (kind !== "audio") return null;
  const request = placeRequest(made.artifactId, made.prompt);
  return request ? { label: t("generate.place"), title: t("generate.placeTitle"), request } : null;
}

// src/desktop/pane.tsx
import { useEffect as useEffect4, useState as useState3 } from "react";

// src/shared/HistoryView.tsx
import { jsx as jsx3, jsxs as jsxs3 } from "react/jsx-runtime";
var STATE = {
  applied: { mark: "✓", tone: "good", word: "history.applied" },
  undone: { mark: "↺", tone: "muted", word: "history.undone" },
  kept: { mark: "✓", tone: "muted", word: "history.kept" },
  expired: { mark: "·", tone: "muted", word: "history.expired" },
  partial: { mark: "◐", tone: "warn", word: "history.partial" },
  unsure: { mark: "?", tone: "warn", word: "history.unsure" },
  refused: { mark: "✕", tone: "muted", word: "history.refused" },
  failed: { mark: "✕", tone: "bad", word: "history.failed" }
};
function restartedSince(item, live) {
  if (item.note || item.family === "script") return false;
  return item.state === "expired" || live && item.state === "applied" && item.undo_available === false;
}
function stateOf(item, t = english, live = false) {
  const known = STATE[restartedSince(item, live) ? "expired" : item.state ?? ""];
  if (known) return { ...known, word: t(known.word) };
  return { mark: "·", tone: "muted", word: item.state ? clean(item.state, 20) : t("history.recorded") };
}
var TONE = { good: T.good, warn: T.warn, bad: T.bad, muted: T.faint };
function FocusView({ focus, now }) {
  const t = useT();
  const catchUp = catchUpLines(focus);
  if (!focus || !focus.focus && !focus.project && !catchUp.lines.length) {
    return /* @__PURE__ */ jsx3("span", { style: S.muted, children: t("focus.nothing") });
  }
  const transport = focus.transport;
  const path = focusPath(focus);
  const parameter = focusParameter(focus);
  const busy = nameOf(focus.focus?.busy ?? void 0);
  const meter = nameOf(transport?.meter ?? void 0);
  return /* @__PURE__ */ jsxs3("div", { style: { display: "flex", flexDirection: "column", gap: 6, opacity: focus.stale ? 0.6 : 1 }, children: [
    /* @__PURE__ */ jsxs3("div", { style: { ...S.row, gap: 7 }, children: [
      /* @__PURE__ */ jsx3(Dot, { color: colorOf(focus.focus?.track) ?? T.faint, size: 9 }),
      /* @__PURE__ */ jsx3("span", { style: { ...S.text, ...S.ellipsis, fontWeight: 600 }, children: path.length ? path.join(" › ") : t("focus.nothingSelected") })
    ] }),
    parameter && /* @__PURE__ */ jsx3("span", { style: { ...S.muted, ...S.ellipsis }, children: parameter }),
    /* @__PURE__ */ jsxs3("div", { style: { ...S.row, gap: 4, flexWrap: "wrap" }, children: [
      focus.project?.name && /* @__PURE__ */ jsx3(Chip, { title: t("focus.project"), children: clean(focus.project.name, 40) }),
      focusPlace(focus).map((place, index) => /* @__PURE__ */ jsx3(Chip, { children: place }, index)),
      typeof transport?.playing === "boolean" && /* @__PURE__ */ jsx3(Chip, { tone: transport.playing ? "good" : void 0, children: transport.playing ? t("focus.playing") : t("focus.stopped") }),
      bpm(transport?.tempo) && /* @__PURE__ */ jsx3(Chip, { children: bpm(transport?.tempo) }),
      meter && /* @__PURE__ */ jsx3(Chip, { children: clean(meter, 12) }),
      busy && /* @__PURE__ */ jsx3(Chip, { tone: "warn", title: t("focus.busy"), children: clean(busy, 40) }),
      focus.stale && (focus.running === false ? /* @__PURE__ */ jsx3(Chip, { children: t("focus.kumiStopped") }) : /* @__PURE__ */ jsx3(Chip, { tone: "warn", children: t("focus.notAnswering") }))
    ] }),
    catchUp.lines.length > 0 && /* @__PURE__ */ jsxs3("div", { style: { display: "flex", flexDirection: "column", gap: 2 }, children: [
      /* @__PURE__ */ jsx3("span", { style: S.faint, children: t("focus.since") }),
      catchUp.lines.map((line, index) => /* @__PURE__ */ jsx3("span", { style: { ...S.muted, ...S.ellipsis }, children: line }, index)),
      catchUp.more > 0 && /* @__PURE__ */ jsx3("span", { style: S.faint, children: t("focus.more", catchUp.more) })
    ] }),
    focus.stale && typeof focus.heard_at === "number" && /* @__PURE__ */ jsx3("span", { style: S.faint, children: t("focus.lastHeard", ago(focus.heard_at, now, t)) })
  ] });
}
function undoKeys(target) {
  const on = typeof target === "string" && target ? target : globalThis.navigator?.userAgent ?? "";
  return /apple-darwin|Mac/.test(on) ? "Cmd+Z" : "Ctrl+Z";
}
function DawUndo({ daw, how, keys }) {
  const t = useT();
  const app = dawName(daw, t);
  if (how === "buried") {
    return /* @__PURE__ */ jsx3("span", { title: t("history.byHandTitle", app), style: { ...S.muted, color: T.warn, whiteSpace: "normal" }, children: t("history.byHand", app) });
  }
  return /* @__PURE__ */ jsx3("span", { title: t("history.dawUndoTitle", app, keys), style: { ...S.muted, whiteSpace: "normal" }, children: t("history.dawUndo", app) });
}
function HistoryView({
  history,
  now,
  action,
  detail,
  live = false,
  daw,
  target
}) {
  const t = useT();
  const items = history?.items ?? [];
  if (!items.length) {
    return /* @__PURE__ */ jsx3("span", { style: S.muted, children: t("history.empty") });
  }
  return /* @__PURE__ */ jsx3("ol", { style: { listStyle: "none", margin: 0, padding: 0, display: "flex", flexDirection: "column", gap: 2 }, children: items.map((item) => {
    const state = stateOf(item, t, live);
    const track = nameOf(item.track);
    const dawUndo = item.state === "applied" && (item.daw_undo === "next" || item.daw_undo === "buried") ? item.daw_undo : null;
    return /* @__PURE__ */ jsxs3("li", { style: { ...S.row, gap: 7, padding: "4px 0", borderTop: `1px solid ${T.border}`, opacity: item.state === "undone" ? 0.7 : 1 }, children: [
      /* @__PURE__ */ jsx3("span", { "aria-label": state.word, title: state.word, style: { width: 14, textAlign: "center", color: TONE[state.tone], flex: "none" }, children: state.mark }),
      /* @__PURE__ */ jsxs3("span", { style: { display: "flex", flexDirection: "column", minWidth: 0, flex: 1 }, children: [
        /* @__PURE__ */ jsx3("span", { style: { ...S.text, ...S.ellipsis, textDecoration: item.state === "undone" ? "line-through" : void 0 }, title: clean(item.title ?? "", 300), children: clean(item.title ?? item.label ?? item.id, 120) }),
        /* @__PURE__ */ jsxs3("span", { style: { ...S.row, gap: 5 }, children: [
          track && /* @__PURE__ */ jsx3(Dot, { color: colorOf(item.track) ?? T.faint, size: 6 }),
          /* @__PURE__ */ jsx3("span", { style: { ...S.faint, ...S.ellipsis }, children: [track && clean(track, 24), item.family && clean(item.family, 16), item.label && clean(item.label, 12), ago(item.at, now, t)].filter(Boolean).join(" · ") })
        ] }),
        item.state === "unsure" || item.state === "partial" ? /* @__PURE__ */ jsx3("span", { style: { ...S.muted, color: T.warn, whiteSpace: "normal" }, children: [clean(item.reason ?? item.note ?? t("history.unconfirmed"), 200), cleanupWords(item.cleanup, t), t("history.lookFirst")].filter(Boolean).join(" ") }) : (
          // The runtime's own words for any other row (why an undo didn't happen, say), why a script's can't (Kumi
          // never undoes one, CC-19), or why a restart's can't.
          item.note ? /* @__PURE__ */ jsx3("span", { style: { ...S.muted, whiteSpace: "normal" }, children: clean(item.note, 400) }) : item.family === "script" ? /* @__PURE__ */ jsx3("span", { style: { ...S.muted, whiteSpace: "normal" }, children: t("history.scriptUndo", dawName(daw, t)) }) : restartedSince(item, live) && /* @__PURE__ */ jsx3("span", { style: { ...S.muted, whiteSpace: "normal" }, children: t("history.restartedSince", dawName(daw, t)) })
        ),
        item.copy?.file ? /* @__PURE__ */ jsx3("span", { style: { ...S.muted, whiteSpace: "normal" }, children: t("history.copyKept", clean(item.copy.file.split(/[/\\]/).pop() ?? "", 120)) }) : null,
        dawUndo && /* @__PURE__ */ jsx3(DawUndo, { daw, how: dawUndo, keys: undoKeys(target) }),
        detail?.(item)
      ] }),
      dawUndo ? null : action?.(item)
    ] }, item.id);
  }) });
}

// src/desktop/jobs.tsx
import { jsx as jsx4, jsxs as jsxs4 } from "react/jsx-runtime";
var TONE2 = { good: T.good, warn: T.warn, bad: T.bad, muted: T.faint };
function JobsView({ jobs, onRequest }) {
  const t = useT();
  const shown = Object.entries(jobs);
  if (!shown.length) return null;
  return /* @__PURE__ */ jsx4("ul", { style: { listStyle: "none", margin: 0, padding: 0, display: "flex", flexDirection: "column", gap: 6 }, children: shown.map(([id, job]) => {
    const generation = job.kind === "generate" ? parseGeneration(job) : null;
    const words = generation ? generationJobWords(job, t) : jobWords(job, t);
    const offer = generation && onRequest ? generationRequest(generation, t) : null;
    return /* @__PURE__ */ jsxs4("li", { style: { display: "flex", flexDirection: "column", gap: 3 }, children: [
      /* @__PURE__ */ jsxs4("span", { style: { ...S.row, gap: 6 }, children: [
        /* @__PURE__ */ jsx4("span", { style: { ...S.text, color: job.ended_at === null ? T.text : TONE2[words.tone] }, children: words.title }),
        words.detail && // A generation's detail names credits: the tooltip says they're Mirelo API credits, not money (13d).
        /* @__PURE__ */ jsx4("span", { style: { ...S.faint, ...S.ellipsis }, title: generation ? t("generate.creditsTitle") : void 0, children: words.detail })
      ] }),
      job.ended_at === null && /* @__PURE__ */ jsx4(
        "span",
        {
          "aria-label": words.fraction === null ? t("job.underWay", words.title) : t("job.percent", words.title, Math.round(words.fraction * 100)),
          "aria-valuemax": 100,
          "aria-valuemin": 0,
          "aria-valuenow": words.fraction === null ? void 0 : Math.round(words.fraction * 100),
          role: "progressbar",
          style: { display: "block", height: 3, borderRadius: 2, background: T.border, overflow: "hidden" },
          children: /* @__PURE__ */ jsx4(
            "span",
            {
              style: {
                display: "block",
                height: "100%",
                width: words.fraction === null ? "30%" : `${Math.max(2, Math.round(words.fraction * 100))}%`,
                background: T.accent,
                opacity: words.fraction === null ? 0.5 : 1
              }
            }
          )
        }
      ),
      offer && /* @__PURE__ */ jsx4("span", { children: /* @__PURE__ */ jsx4(TextButton, { onClick: () => onRequest?.(offer.request), title: offer.title, children: offer.label }) })
    ] }, id);
  }) });
}

// src/desktop/script.tsx
import { useEffect as useEffect3, useState as useState2 } from "react";

// src/shared/Code.tsx
import { jsx as jsx5, jsxs as jsxs5 } from "react/jsx-runtime";
var INVISIBLE = /[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F-\u009F\u061C\u200B-\u200F\u202A-\u202E\u2060\u2066-\u2069\uFEFF]/;
function visibleLine(line) {
  const parts = [];
  let text = "";
  for (const char of line) {
    if (!INVISIBLE.test(char)) {
      text += char;
      continue;
    }
    if (text) parts.push(text);
    text = "";
    const code = `U+${char.codePointAt(0).toString(16).toUpperCase().padStart(4, "0")}`;
    parts.push(
      /* @__PURE__ */ jsx5("mark", { "data-invisible": code, style: { background: "transparent", color: T.bad, border: `1px solid ${T.bad}`, borderRadius: 3, padding: "0 2px", fontSize: "0.85em" }, children: code }, parts.length)
    );
  }
  if (text || !parts.length) parts.push(text);
  return parts;
}
function CodeBlock({ code, label }) {
  const t = useT();
  const lines = code.split(/\r?\n/);
  if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
  return (
    // Focusable, so the keyboard can scroll it.
    /* @__PURE__ */ jsx5(
      "div",
      {
        "aria-label": label,
        className: FOCUSABLE,
        role: "region",
        style: { maxHeight: "45vh", overflow: "auto", border: `1px solid ${T.border}`, borderRadius: 4, background: T.card },
        tabIndex: 0,
        title: t("script.codeTitle"),
        children: /* @__PURE__ */ jsx5("pre", { style: { margin: 0, padding: "6px 0", fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace", fontSize: 11.5, lineHeight: 1.5, color: T.text }, children: lines.map((line, index) => /* @__PURE__ */ jsxs5("div", { style: { display: "flex" }, children: [
          /* @__PURE__ */ jsx5("span", { "aria-hidden": true, style: { flex: "none", width: "3.2em", paddingRight: 8, textAlign: "right", color: T.faint, userSelect: "none" }, children: index + 1 }),
          /* @__PURE__ */ jsx5("span", { style: { whiteSpace: "pre", paddingRight: 8 }, children: visibleLine(line) })
        ] }, index)) })
      }
    )
  );
}

// src/desktop/script.tsx
import { Fragment as Fragment2, jsx as jsx6, jsxs as jsxs6 } from "react/jsx-runtime";
var LANGUAGE = { python: "script.python", lua: "script.lua", device: "script.device" };
function deviceSpecOf(text) {
  try {
    const value = JSON.parse(text);
    if (!value || typeof value !== "object" || typeof value.code !== "string") return null;
    const controls = value.controls;
    return controls === void 0 || Array.isArray(controls) ? value : null;
  } catch {
    return null;
  }
}
function unitOf(control) {
  return typeof control.unit === "string" && control.unit ? ` ${clean(control.unit, 12)}` : "";
}
function range(control, t) {
  if (control.type === "switch") return `${t("script.on")}/${t("script.off")}`;
  if (control.type === "choice" && Array.isArray(control.options)) return control.options.map((option) => clean(String(option), 20)).join("/");
  return typeof control.min === "number" && typeof control.max === "number" ? `${control.min}–${control.max}${unitOf(control)}` : "—";
}
function defaultOf(control, t) {
  if (control.default === void 0) return "—";
  if (control.type === "switch" && typeof control.default === "boolean") return t(control.default ? "script.on" : "script.off");
  return `${clean(String(control.default), 20)}${control.type === "choice" || control.type === "switch" ? "" : unitOf(control)}`;
}
function Controls({ spec }) {
  const t = useT();
  const controls = (spec.controls ?? []).slice(0, 16);
  if (!controls.length) return /* @__PURE__ */ jsx6("span", { style: S.faint, children: t("script.noControls") });
  const cell = { padding: "2px 6px", textAlign: "left", whiteSpace: "nowrap" };
  return /* @__PURE__ */ jsxs6("table", { "aria-label": t("script.controls"), style: { borderCollapse: "collapse", fontSize: 12, lineHeight: 1.45, color: T.text, margin: 0 }, children: [
    /* @__PURE__ */ jsx6("thead", { children: /* @__PURE__ */ jsxs6("tr", { style: { color: T.muted, fontSize: 11 }, children: [
      /* @__PURE__ */ jsx6("th", { style: { ...cell, fontWeight: 400 }, children: t("script.control") }),
      /* @__PURE__ */ jsx6("th", { style: { ...cell, fontWeight: 400 }, children: t("script.range") }),
      /* @__PURE__ */ jsx6("th", { style: { ...cell, fontWeight: 400 }, children: t("script.default") })
    ] }) }),
    /* @__PURE__ */ jsx6("tbody", { children: controls.map((control, index) => /* @__PURE__ */ jsxs6("tr", { style: { borderTop: `1px solid ${T.border}` }, children: [
      /* @__PURE__ */ jsx6("td", { style: cell, children: clean(String(control.name ?? ""), 32) }),
      /* @__PURE__ */ jsx6("td", { style: { ...cell, color: T.muted }, children: range(control, t) }),
      /* @__PURE__ */ jsx6("td", { style: { ...cell, color: T.muted }, children: defaultOf(control, t) })
    ] }, index)) })
  ] });
}
function PendingScriptView({ script, daw }) {
  const t = useT();
  const { pending, code } = script;
  const where = pending.daw ?? daw;
  const facts = [
    t("script.in", t(LANGUAGE[pending.language]), dawName(where, t)),
    typeof pending.lines === "number" ? t("script.lines", pending.lines) : "",
    t("script.sha", pending.sha256.slice(0, 12))
  ].filter(Boolean);
  const heading = t(pending.language === "device" ? "script.deviceWaiting" : "script.waiting");
  return /* @__PURE__ */ jsxs6("section", { "aria-label": heading, style: { ...S.section, borderLeft: `2px solid ${T.warn}` }, children: [
    /* @__PURE__ */ jsx6("span", { style: S.heading, children: heading }),
    pending.purpose && /* @__PURE__ */ jsx6("span", { style: S.text, children: t("script.says", clean(pending.purpose, 200)) }),
    pending.file && /* @__PURE__ */ jsx6("span", { style: S.muted, children: t("script.file", clean(pending.file, 200)) }),
    /* @__PURE__ */ jsx6("span", { style: S.faint, title: t("script.shaTitle", pending.sha256), children: facts.join(" · ") }),
    pending.warning && /* @__PURE__ */ jsx6("span", { style: { ...S.muted, color: T.warn }, children: clean(pending.warning, 300) }),
    code ? pending.language === "device" ? (() => {
      const spec = deviceSpecOf(code.code);
      return spec ? /* @__PURE__ */ jsxs6(Fragment2, { children: [
        spec.name && /* @__PURE__ */ jsx6("span", { style: { ...S.text, fontWeight: 600 }, children: clean(spec.name, 32) }),
        /* @__PURE__ */ jsx6(Controls, { spec }),
        /* @__PURE__ */ jsx6(CodeBlock, { code: spec.code, label: t("script.deviceCode") })
      ] }) : /* @__PURE__ */ jsx6("span", { style: S.muted, children: t("script.unreadable") });
    })() : /* @__PURE__ */ jsx6(CodeBlock, { code: code.code, label: t("script.code") }) : code === false ? /* @__PURE__ */ jsx6("span", { style: S.muted, children: t("script.unreadable") }) : /* @__PURE__ */ jsx6("span", { style: S.faint, children: t("script.reading") })
  ] });
}
function SavedScript({ item, store }) {
  const t = useT();
  const sha = item.script?.sha256;
  const language = item.script?.language === "python" || item.script?.language === "lua" ? item.script.language : null;
  const [open, setOpen] = useState2(false);
  const [code, setCode] = useState2(null);
  useEffect3(() => {
    if (!open || code !== null || !sha || !language) return;
    let current = true;
    void store.readScript(sha, language).then((read) => {
      if (current) setCode(read ?? false);
    });
    return () => {
      current = false;
    };
  }, [open, code, sha, language, store]);
  if (!sha || !language) return null;
  return /* @__PURE__ */ jsxs6("span", { style: { display: "flex", flexDirection: "column", gap: 4, marginTop: 2 }, children: [
    /* @__PURE__ */ jsx6("span", { children: /* @__PURE__ */ jsx6(TextButton, { onClick: () => setOpen(!open), pressed: open, title: t("script.showTitle"), children: open ? t("script.hide") : t("script.show") }) }),
    open && (code ? /* @__PURE__ */ jsx6(CodeBlock, { code: code.code, label: t("script.code") }) : code === false ? /* @__PURE__ */ jsx6("span", { style: S.muted, children: t("script.unreadableSaved") }) : /* @__PURE__ */ jsx6("span", { style: S.faint, children: t("script.reading") }))
  ] });
}

// src/desktop/pane.tsx
import { Fragment as Fragment3, jsx as jsx7, jsxs as jsxs7 } from "react/jsx-runtime";
function useNow(every = 3e4) {
  const [now, setNow] = useState3(() => Date.now());
  useEffect4(() => {
    const timer = setInterval(() => setNow(Date.now()), every);
    return () => clearInterval(timer);
  }, [every]);
  return now;
}
function ownerSession(state) {
  const session = state.status?.owner?.session_id;
  return typeof session === "string" && session ? session : null;
}
var NEEDS_SETUP = { needs_runtime: true, outdated_runtime: true, damaged_runtime: true, needs_daw_component: true };
var IN_DAW = { choose_control_surface: true, restart_daw: true, enable_tool: true };
var SETUP_DAWS = ["ableton", "reaper", "renoise"];
function dawChosen(state) {
  return state.setup?.daw_chosen !== false;
}
function nextSteps(state, t = english) {
  const setup = state.setup;
  const daw = state.status?.daw ?? setup?.daw;
  if (setup && !dawPartWorks(state) && (NEEDS_SETUP[setup.state ?? ""] || setup.next_step === "install_runtime" || setup.next_step === "install_daw_component")) {
    return (dawChosen(state) ? [daw] : SETUP_DAWS).map((each) => ({ label: t("step.setup", dawName(each, t)), request: setupRequest(each) }));
  }
  if (setup?.state === "ready" && !state.status?.connected) return [{ label: t("step.connect", dawName(daw, t)), request: connectRequest(daw) }];
  return [];
}
function setupWords(state) {
  const setup = state.setup;
  const said = setup?.state;
  if (typeof setup?.message !== "string" || !setup.message || !said || said === "unknown" || dawPartWorks(state)) return null;
  if (said === "ready") return !state.status?.connected && IN_DAW[setup.next_step ?? ""] ? setup.message : null;
  if (said === "needs_daw_component" && !dawChosen(state)) return null;
  return setup.message;
}
var TONE3 = { good: T.good, warn: T.warn, bad: T.bad, muted: T.faint };
function KumiPane({ ctx, store, onOpenMap }) {
  const t = useT();
  const state = useKumi(store);
  const now = useNow();
  if (state.available === false) {
    return /* @__PURE__ */ jsxs7("div", { style: { padding: 14, display: "flex", flexDirection: "column", gap: 8 }, children: [
      /* @__PURE__ */ jsx7("span", { style: S.text, children: t("pane.agentOff") }),
      /* @__PURE__ */ jsx7("span", { style: S.muted, children: t("pane.agentOffHow") })
    ] });
  }
  if (state.available === null) {
    return /* @__PURE__ */ jsx7("div", { style: { ...S.muted, padding: 14 }, children: t("pane.reading") });
  }
  const words = chipWords(state.status, state.focus?.transport?.tempo, t);
  const steps = nextSteps(state, t);
  const session = ownerSession(state);
  const connected = Boolean(state.status?.connected);
  const setupMessage = setupWords(state);
  const changes = changesOf(state.history?.items ?? []);
  const changeOf = new Map(changes.flatMap((rows) => rows.map((row2) => [row2, rows])));
  const undoable = changes.filter((rows) => canUndoChange(state.status, rows)).length;
  return /* @__PURE__ */ jsxs7("div", { style: { height: "100%", overflowY: "auto", overflowX: "hidden", color: T.text }, children: [
    state.script && /* @__PURE__ */ jsx7(PendingScriptView, { daw: state.status?.daw, script: state.script }),
    /* @__PURE__ */ jsxs7("div", { style: { ...S.section, gap: 8 }, children: [
      /* @__PURE__ */ jsxs7("div", { style: { ...S.row, gap: 8 }, children: [
        /* @__PURE__ */ jsx7(Dot, { color: TONE3[words.tone], size: 8 }),
        /* @__PURE__ */ jsx7("span", { style: { ...S.text, ...S.ellipsis, fontWeight: 600, flex: 1 }, children: words.text }),
        state.status?.runtime?.version && /* @__PURE__ */ jsxs7("span", { style: S.faint, children: [
          "Kumi ",
          state.status.runtime.version
        ] })
      ] }),
      setupMessage && /* @__PURE__ */ jsx7("span", { style: S.muted, children: setupMessage }),
      state.error && /* @__PURE__ */ jsx7("span", { style: { ...S.muted, color: T.warn }, children: t("pane.backendError", state.error) }),
      state.status?.unsure && /* @__PURE__ */ jsxs7("div", { style: { display: "flex", flexDirection: "column", gap: 4 }, children: [
        /* @__PURE__ */ jsx7("span", { style: { ...S.muted, color: T.warn }, children: [
          t("pane.unsure", clean(state.status.unsure.op ?? t("pane.unsureOp"), 40), state.status.unsure.reason ? clean(state.status.unsure.reason, 160) : ""),
          cleanupWords(state.status.unsure.cleanup, t),
          t("pane.needsLook")
        ].filter(Boolean).join(" ") }),
        /* @__PURE__ */ jsx7("div", { style: S.row, children: /* @__PURE__ */ jsx7(TextButton, { onClick: () => void ask(ctx, LOOK_AGAIN_REQUEST, { send: true, session }), children: t("pane.lookAgain") }) })
      ] }),
      steps.length > 0 && /* @__PURE__ */ jsx7("div", { style: { ...S.row, gap: 6, flexWrap: "wrap" }, children: steps.map((step2) => /* @__PURE__ */ jsx7(TextButton, { onClick: () => void ask(ctx, step2.request, { send: true, session: connected ? session : null }), strong: steps.length === 1, children: step2.label }, step2.request)) })
    ] }),
    /* @__PURE__ */ jsx7(Section, { title: t("pane.focus"), children: /* @__PURE__ */ jsx7(FocusView, { focus: state.focus, now }) }),
    Object.keys(state.jobs).length > 0 && /* @__PURE__ */ jsx7(Section, { title: t("pane.jobs"), children: /* @__PURE__ */ jsx7(JobsView, { jobs: state.jobs, onRequest: (request) => void ask(ctx, request, { send: false, session }) }) }),
    /* @__PURE__ */ jsxs7(
      Section,
      {
        title: /* @__PURE__ */ jsxs7(Fragment3, { children: [
          t("pane.history"),
          " ",
          /* @__PURE__ */ jsx7("span", { style: { textTransform: "none", letterSpacing: 0 }, children: undoable ? t("pane.undoable", undoable) : "" })
        ] }),
        children: [
          /* @__PURE__ */ jsx7(
            HistoryView,
            {
              daw: state.status?.daw,
              history: state.history,
              live: Boolean(state.status?.connected) && state.status?.stale !== true,
              now,
              target: state.setup?.target,
              detail: (item) => item.script ? /* @__PURE__ */ jsx7(SavedScript, { item, store }) : null,
              action: (item) => {
                const rows = changeOf.get(item);
                if (!rows || undoRowOf(rows) !== item || !canUndoChange(state.status, rows)) return null;
                const left = rows.filter((row2) => row2.state !== "undone").length;
                return /* @__PURE__ */ jsx7(
                  TextButton,
                  {
                    label: t("pane.undoLabel", clean(item.title ?? item.label ?? item.id, 120)),
                    onClick: () => void ask(ctx, undoRequest(item), { send: true, session }),
                    title: left > 1 ? t("pane.undoChangeTitle", left) : t("pane.undoTitle"),
                    children: t("pane.undo")
                  }
                );
              }
            }
          ),
          !connected && state.status?.owner && (state.history?.items?.length ?? 0) > 0 && /* @__PURE__ */ jsx7("span", { style: S.faint, children: t("pane.undoNeedsConnection") })
        ]
      }
    ),
    /* @__PURE__ */ jsx7("div", { style: { ...S.row, padding: "10px 12px", gap: 8 }, children: /* @__PURE__ */ jsx7(TextButton, { onClick: onOpenMap, children: t("pane.openMap") }) })
  ] });
}

// src/desktop/generate.tsx
import { jsx as jsx8, jsxs as jsxs8 } from "react/jsx-runtime";
var OUTPUTS = /* @__PURE__ */ new Set(["midi", "notes", "musicxml"]);
var ENDED = /* @__PURE__ */ new Set(["failed", "cancelled", "interrupted"]);
function GenerateCard({ media, audition, ctx, store }) {
  const t = useT();
  const made = parseGeneration(media.result);
  if (!made) return media.fallback;
  const title = generationTitle(made, t);
  const kind = outputKind(made);
  const cost = costWords(made, t);
  const left = capWords(made, t);
  const length = lengthWords(made, t);
  const offer = generationRequest(made, t);
  return /* @__PURE__ */ jsxs8(
    "div",
    {
      "data-kumi-card": "generate",
      style: { display: "flex", flexDirection: "column", gap: 8, maxWidth: 560, padding: "10px 12px", borderRadius: 6, border: `1px solid ${T.border}`, background: T.card, color: T.text },
      children: [
        /* @__PURE__ */ jsxs8("div", { style: { ...S.row, justifyContent: "space-between", alignItems: "flex-start", gap: 8 }, children: [
          /* @__PURE__ */ jsxs8("span", { style: { ...S.text, fontWeight: 600, minWidth: 0, whiteSpace: "normal" }, children: [
            /* @__PURE__ */ jsx8("span", { style: { color: T.accent }, children: kind === "notes" ? "♫" : "♪" }),
            " ",
            made.prompt ? `“${title}”` : title
          ] }),
          /* @__PURE__ */ jsxs8("span", { style: { ...S.row, gap: 4, flexShrink: 0 }, children: [
            made.outputType && OUTPUTS.has(made.outputType) && /* @__PURE__ */ jsx8(Chip, { children: t(`generate.outputs.${made.outputType}`) }),
            made.noteCount !== null && /* @__PURE__ */ jsx8(Chip, { children: t("generate.notes", made.noteCount, String(made.noteCount)) }),
            length && /* @__PURE__ */ jsx8(Chip, { children: length }),
            ENDED.has(made.state) ? /* @__PURE__ */ jsx8(Chip, { tone: "bad", children: t(`generate.states.${made.state}`) }) : /* @__PURE__ */ jsx8(Chip, { title: t("generate.generatedTitle"), tone: "accent", children: t("generate.generated") })
          ] })
        ] }),
        made.file && /* @__PURE__ */ jsx8("span", { style: { ...S.faint, ...S.ellipsis }, children: made.file }),
        kind === "audio" && made.artifactId && media.file?.kind === "audio" ? (
          // In a row, as in the listen card: the waveform's flex basis is a width there, not a height.
          /* @__PURE__ */ jsx8("div", { style: { display: "flex" }, children: /* @__PURE__ */ jsx8(Wave, { audition, caption: "", id: made.artifactId, label: title }) })
        ) : (
          // Notes, MIDI and MusicXML: Desktop's own file chip, to open or save it.
          media.fallback
        ),
        ENDED.has(made.state) && made.error?.reason && /* @__PURE__ */ jsx8("span", { style: { ...S.muted, whiteSpace: "normal" }, children: made.error.reason }),
        (cost || left) && /* @__PURE__ */ jsxs8("span", { style: { ...S.row, gap: 6, flexWrap: "wrap" }, children: [
          cost && /* @__PURE__ */ jsx8(Chip, { title: t("generate.creditsTitle"), children: cost }),
          left && /* @__PURE__ */ jsx8("span", { style: S.faint, title: t("generate.capTitle"), children: left })
        ] }),
        made.ledger?.frozen && /* @__PURE__ */ jsx8("span", { style: { ...S.text, color: T.warn, whiteSpace: "normal" }, children: t("generate.frozen") }),
        offer && /* @__PURE__ */ jsx8("span", { children: /* @__PURE__ */ jsx8(TextButton, { onClick: () => void ask(ctx, offer.request, { send: false, session: ownerSession(store.get()) }), strong: true, title: offer.title, children: offer.label }) })
      ]
    }
  );
}

// src/desktop/composer.tsx
import { host as host2 } from "@hermes/plugin-sdk";
import { useCallback as useCallback2, useSyncExternalStore as useSyncExternalStore2 } from "react";
import { jsx as jsx9, jsxs as jsxs9 } from "react/jsx-runtime";
var noChange = () => () => {
};
function useFocusedChat() {
  const atom = host2.state.focusedStoredSessionId;
  const subscribe = useCallback2(
    (fn) => atom ? atom.listen ? atom.listen(() => fn()) : atom.subscribe(() => fn()) : noChange(),
    [atom]
  );
  const read = () => atom?.get() ?? null;
  return useSyncExternalStore2(subscribe, read, read);
}
function FocusHint({ store }) {
  const t = useT();
  useWant(store, "focus");
  const state = useKumi(store);
  const chat = useFocusedChat();
  const owner = ownerSession(state);
  if (!state.status?.connected || !owner || chat !== owner) return null;
  const path = focusPath(state.focus);
  if (!path.length) return null;
  const tempo = bpm(state.focus?.transport?.tempo);
  const unsure = Boolean(state.status?.unsure);
  return /* @__PURE__ */ jsxs9(
    "div",
    {
      "data-unsure": unsure || void 0,
      style: { ...S.row, gap: 6, padding: "2px 10px", maxWidth: "100%", opacity: unsure ? 0.5 : 1 },
      title: unsure ? t("hint.titleUnsure") : t("hint.title"),
      children: [
        /* @__PURE__ */ jsx9(Dot, { color: unsure ? T.faint : colorOf(state.focus?.focus?.track) ?? T.faint, size: 7 }),
        /* @__PURE__ */ jsx9("span", { style: { ...S.faint, ...S.ellipsis }, children: `${unsure ? t("hint.lastSaw") : t("hint.sees")} ${path.join(" › ")}${tempo ? ` · ${tempo}` : ""}` })
      ]
    }
  );
}

// src/desktop/chip.tsx
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger, StatusDot } from "@hermes/plugin-sdk";
import { jsx as jsx10, jsxs as jsxs10 } from "react/jsx-runtime";
function StatusChip({
  ctx,
  store,
  onOpenMap,
  onOpenSettings
}) {
  const t = useT();
  const state = useKumi(store);
  if (state.available !== true) return null;
  const words = chipWords(state.status, state.focus?.transport?.tempo, t);
  const steps = nextSteps(state, t);
  const connected = Boolean(state.status?.connected);
  return /* @__PURE__ */ jsxs10(DropdownMenu, { children: [
    /* @__PURE__ */ jsx10(DropdownMenuTrigger, { asChild: true, children: /* @__PURE__ */ jsxs10(
      "button",
      {
        "aria-label": t("chip.label", words.text),
        className: FOCUSABLE,
        title: "Kumi",
        type: "button",
        style: {
          display: "inline-flex",
          alignItems: "center",
          gap: 5,
          height: "100%",
          padding: "0 6px",
          border: 0,
          background: "transparent",
          color: "var(--ui-text-tertiary)",
          font: "inherit",
          fontSize: "0.6875rem",
          fontVariantNumeric: "tabular-nums",
          cursor: "pointer",
          whiteSpace: "nowrap"
        },
        children: [
          /* @__PURE__ */ jsx10(StatusDot, { tone: words.tone }),
          words.text
        ]
      }
    ) }),
    /* @__PURE__ */ jsxs10(DropdownMenuContent, { align: "end", side: "top", children: [
      /* @__PURE__ */ jsx10(DropdownMenuItem, { onSelect: onOpenMap, children: t("chip.openMap") }),
      steps.map((step2) => /* @__PURE__ */ jsx10(DropdownMenuItem, { onSelect: () => void ask(ctx, step2.request, { send: true, session: connected ? ownerSession(state) : null }), children: step2.label }, step2.request)),
      /* @__PURE__ */ jsx10(DropdownMenuSeparator, {}),
      /* @__PURE__ */ jsx10(DropdownMenuItem, { onSelect: onOpenSettings, children: t("chip.settings") })
    ] })
  ] });
}

// src/desktop/events.ts
import { host as host3 } from "@hermes/plugin-sdk";
function profileKey(value) {
  return value.trim() || "default";
}
function forThisProfile(payload) {
  const own = host3.state.profile?.get();
  return typeof payload.profile === "string" && typeof own === "string" && profileKey(payload.profile) === profileKey(own);
}
function forAnotherProfile(payload) {
  const own = host3.state.profile?.get();
  return typeof payload.profile === "string" && typeof own === "string" && profileKey(payload.profile) !== profileKey(own);
}
function forChatInFront(payload) {
  if (!forThisProfile(payload) || typeof payload.session_id !== "string" || !payload.session_id) return false;
  return host3.state.focusedStoredSessionId?.get() === payload.session_id;
}
function callId(value) {
  return typeof value === "string" && value.length > 0 && value.length <= 200 ? value : null;
}
var Calls = class {
  constructor(most = 200) {
    this.most = most;
  }
  most;
  seen = [];
  has(id) {
    return this.seen.includes(id);
  }
  add(id) {
    if (this.has(id)) return;
    this.seen.push(id);
    if (this.seen.length > this.most) this.seen.shift();
  }
  clear() {
    this.seen = [];
  }
};
function payloadOf(event) {
  return event.payload && typeof event.payload === "object" ? event.payload : {};
}

// src/desktop/map.tsx
import { host as host4 } from "@hermes/plugin-sdk";
import { useEffect as useEffect9, useRef as useRef5, useState as useState8 } from "react";

// src/shared/MakePanel.tsx
import { useEffect as useEffect5, useLayoutEffect, useMemo, useRef as useRef2, useState as useState4 } from "react";
import { Fragment as Fragment4, jsx as jsx11, jsxs as jsxs11 } from "react/jsx-runtime";
var GLYPHS = {
  blend: /* @__PURE__ */ jsxs11(Fragment4, { children: [
    /* @__PURE__ */ jsx11("circle", { cx: "11", cy: "10", r: "6.5", fill: "currentColor", fillOpacity: "0.35" }),
    /* @__PURE__ */ jsx11("circle", { cx: "17", cy: "10", r: "6.5", fill: "currentColor", fillOpacity: "0.35" })
  ] }),
  attack: /* @__PURE__ */ jsx11("path", { d: "M2 17 L7 17 L8.5 2 L10 12 Q14 16 26 17", fill: "none", stroke: "currentColor", strokeWidth: "1.6", strokeLinejoin: "round" }),
  resonate: /* @__PURE__ */ jsxs11(Fragment4, { children: [
    /* @__PURE__ */ jsx11("circle", { cx: "9", cy: "10", r: "2", fill: "currentColor" }),
    /* @__PURE__ */ jsx11("path", { d: "M13 5 Q16 10 13 15 M17 3 Q21.5 10 17 17 M21 1.5 Q27 10 21 18.5", fill: "none", stroke: "currentColor", strokeWidth: "1.4", strokeLinecap: "round" })
  ] }),
  modulate: /* @__PURE__ */ jsx11("path", { d: "M1 10 Q3 2 5 10 T9 10 Q11 5 13 10 T17 10 Q19 8 21 10 T25 10 Q26 9.5 27 10", fill: "none", stroke: "currentColor", strokeWidth: "1.6", strokeLinecap: "round" }),
  texture: /* @__PURE__ */ jsx11(Fragment4, { children: [
    [3, 6],
    [8, 13],
    [12, 4],
    [16, 11],
    [20, 6],
    [24, 14],
    [6, 17],
    [18, 17],
    [25, 3]
  ].map(([x, y], index) => /* @__PURE__ */ jsx11("circle", { cx: x, cy: y, fill: "currentColor", fillOpacity: 0.45 + index % 3 * 0.2, r: 1.4 + index % 2 * 0.6 }, index)) }),
  loop: /* @__PURE__ */ jsx11("path", { d: "M8 6 H19 A4 4 0 0 1 19 14 H9 M11 11 L8 14 L11 17 M8 6 A4 4 0 0 0 6.5 13", fill: "none", stroke: "currentColor", strokeWidth: "1.6", strokeLinecap: "round", strokeLinejoin: "round" }),
  smear: /* @__PURE__ */ jsx11("path", { d: "M1 10 Q3 3 5 10 Q7 15 10 10 Q14 6 18 10 Q22 12.5 27 10", fill: "none", stroke: "currentColor", strokeWidth: "1.6", strokeLinecap: "round" }),
  stack: /* @__PURE__ */ jsxs11(Fragment4, { children: [
    /* @__PURE__ */ jsx11("rect", { fill: "currentColor", fillOpacity: "0.85", height: "3", rx: "1.5", width: "20", x: "4", y: "4" }),
    /* @__PURE__ */ jsx11("rect", { fill: "currentColor", fillOpacity: "0.6", height: "3", rx: "1.5", width: "20", x: "4", y: "9" }),
    /* @__PURE__ */ jsx11("rect", { fill: "currentColor", fillOpacity: "0.35", height: "3", rx: "1.5", width: "20", x: "4", y: "14" })
  ] })
};
function Glyph({ card, size = 20 }) {
  return /* @__PURE__ */ jsx11("svg", { "aria-hidden": true, height: size, style: { flex: "none" }, viewBox: "0 0 28 20", width: size * 28 / 20, children: GLYPHS[card] });
}
var OP_GLYPH = { cross: "blend", fuse: "smear", stack: "stack" };
var OPS = ["cross", "fuse", "stack"];
var SOURCE_CHIPS = 48;
var ATTACK_CHOICES = 64;
function Flying({ flight, onDone }) {
  const ghost = useRef2(null);
  const done = useRef2(onDone);
  done.current = onDone;
  const [origin, setOrigin] = useState4(null);
  useLayoutEffect(() => {
    const box = ghost.current?.getBoundingClientRect();
    setOrigin(box ? { left: box.left, top: box.top } : null);
  }, []);
  useEffect5(() => {
    const element = ghost.current;
    if (!element || !origin) return;
    const { from, to } = flight;
    const dx = to.left + Math.min(64, to.width / 2) - (from.left + from.width / 2);
    const dy = to.bottom - Math.min(22, to.height / 2) - (from.top + from.height / 2);
    const animation = element.animate(
      [
        { transform: "translate(0, 0) scale(1)", opacity: 1 },
        { transform: `translate(${dx * 0.45}px, ${dy * 0.45 - 36}px) scale(0.5)`, opacity: 0.95, offset: 0.5 },
        { transform: `translate(${dx}px, ${dy}px) scale(0.12)`, opacity: 0.3 }
      ],
      { duration: 640, easing: "cubic-bezier(.35,.6,.4,1)", fill: "forwards" }
    );
    animation.onfinish = () => done.current();
    return () => animation.cancel();
  }, [origin, flight]);
  return /* @__PURE__ */ jsx11(
    "div",
    {
      "aria-hidden": true,
      ref: ghost,
      style: {
        position: "fixed",
        left: origin ? flight.from.left - origin.left : 0,
        top: origin ? flight.from.top - origin.top : 0,
        width: flight.from.width,
        height: flight.from.height,
        borderRadius: 8,
        pointerEvents: "none",
        zIndex: 20,
        visibility: origin ? "visible" : "hidden",
        background: flight.image ? `center / 100% 100% no-repeat url(${flight.image})` : T.accent,
        boxShadow: `0 10px 28px -10px ${T.accent}`
      }
    }
  );
}
function Wave2({ peaks, playing, duration }) {
  const canvas = useRef2(null);
  const box = useRef2(null);
  const [width, setWidth] = useState4(0);
  const [drawn, setDrawn] = useState4(false);
  const [startedAt, setStartedAt] = useState4(null);
  const [now, setNow] = useState4(0);
  useEffect5(() => {
    const element = box.current;
    if (!element) return;
    const observer = new ResizeObserver((entries) => setWidth(Math.floor(entries[0]?.contentRect.width ?? 0)));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  useEffect5(() => {
    const frame2 = requestAnimationFrame(() => setDrawn(true));
    return () => cancelAnimationFrame(frame2);
  }, []);
  useEffect5(() => setStartedAt(playing ? performance.now() : null), [playing]);
  useEffect5(() => {
    if (startedAt === null) return;
    const frame2 = requestAnimationFrame(setNow);
    return () => cancelAnimationFrame(frame2);
  }, [startedAt, now]);
  useEffect5(() => {
    const element = canvas.current;
    const context = element?.getContext("2d");
    if (!element || !context || width === 0) return;
    const ratio = window.devicePixelRatio || 1;
    const height = 56;
    element.width = width * ratio;
    element.height = height * ratio;
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, width, height);
    const accent = getComputedStyle(element).color || "#4a90e2";
    const gradient = context.createLinearGradient(0, 0, width, 0);
    gradient.addColorStop(0, accent);
    gradient.addColorStop(1, `color-mix(in srgb, ${accent} 55%, transparent)`);
    context.fillStyle = gradient;
    const bars = peaks.length || 1;
    const step2 = width / bars;
    for (let bar = 0; bar < peaks.length; bar++) {
      const tall = Math.max(1, peaks[bar] * (height - 4));
      context.fillRect(bar * step2, (height - tall) / 2, Math.max(1, step2 - 0.4), tall);
    }
  }, [peaks, width]);
  const progress = startedAt !== null && duration > 0 ? Math.min(1, (now - startedAt) / 1e3 / duration) : null;
  return /* @__PURE__ */ jsxs11("div", { ref: box, style: { position: "relative", height: 56, color: T.accent }, children: [
    /* @__PURE__ */ jsx11(
      "canvas",
      {
        "aria-hidden": true,
        ref: canvas,
        style: { width: "100%", height: 56, display: "block", clipPath: drawn ? "inset(0 0 0 0)" : "inset(0 100% 0 0)", transition: "clip-path 520ms cubic-bezier(.2,.8,.2,1)" }
      }
    ),
    progress !== null && progress > 0 && progress < 1 && /* @__PURE__ */ jsx11("span", { "aria-hidden": true, style: { position: "absolute", top: 0, bottom: 0, left: `${progress * 100}%`, width: 2, borderRadius: 1, background: T.text, opacity: 0.8 } })
  ] });
}
function OptionSlider({ name, label, value, onChange }) {
  const t = useT();
  const range2 = RANGES[name];
  const shown = value ?? (range2.min + range2.max) / 2;
  return /* @__PURE__ */ jsxs11("label", { style: { display: "flex", flexDirection: "column", gap: 2, minWidth: 150, flex: "1 1 150px" }, children: [
    /* @__PURE__ */ jsxs11("span", { style: { ...S.row, gap: 6, fontSize: 11.5, color: T.secondary }, children: [
      /* @__PURE__ */ jsx11("span", { style: { flex: 1 }, children: value === void 0 ? t(`make.options.${name}`) : label(value) }),
      /* @__PURE__ */ jsx11(
        "button",
        {
          "aria-pressed": value === void 0,
          className: FOCUSABLE,
          onClick: (event) => {
            event.preventDefault();
            onChange(value === void 0 ? shown : void 0);
          },
          title: t("make.autoTitle"),
          type: "button",
          style: { font: "inherit", fontSize: 10.5, padding: "0 6px", borderRadius: 8, border: `1px solid ${value === void 0 ? T.accent : T.border}`, background: "transparent", color: value === void 0 ? T.accent : T.faint, cursor: "pointer" },
          children: t("make.auto")
        }
      )
    ] }),
    /* @__PURE__ */ jsx11(
      "input",
      {
        "aria-label": t(`make.options.${name}`),
        className: FOCUSABLE,
        max: range2.max,
        min: range2.min,
        onChange: (event) => onChange(Number(event.target.value)),
        step: range2.step,
        style: { accentColor: T.accent, opacity: value === void 0 ? 0.45 : 1 },
        type: "range",
        value: shown
      }
    )
  ] });
}
function MakePanel({ sources, colorOf: colorOf2, tempo, results, onMake, onClose, playing, onPlay, onStop, onKeep, kept, use, inline = false, dock }) {
  const t = useT();
  const [cardKey, setCardKey] = useState4("blend");
  const [lastMode, setLastMode] = useState4({ cross: "blend", fuse: "texture", stack: "stack" });
  const [options, setOptions] = useState4(() => typeof tempo === "number" && tempo >= 40 && tempo <= 240 ? { bpm: Math.round(tempo) } : {});
  const [busy, setBusy] = useState4(null);
  const [error, setError] = useState4(null);
  const requests = useRef2(/* @__PURE__ */ new Map());
  const [shown, setShown] = useState4({});
  const [fresh, setFresh] = useState4(() => /* @__PURE__ */ new Set());
  const [arrived, setArrived] = useState4(null);
  const cards = useRef2(/* @__PURE__ */ new Map());
  useEffect5(() => {
    if (!arrived) return;
    cards.current.get(arrived)?.scrollIntoView({ block: "nearest", behavior: window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth" });
    setArrived(null);
  }, [arrived, results]);
  const [flights, setFlights] = useState4([]);
  const flightKey = useRef2(0);
  const fly = (id) => {
    const canvas = cards.current.get(id)?.querySelector("canvas");
    const to = dock?.current?.getBoundingClientRect();
    if (!canvas || !to || to.width === 0 || window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const key = ++flightKey.current;
    setFlights((current) => [...current, { key, from: canvas.getBoundingClientRect(), to, image: canvas.toDataURL() }]);
  };
  const [open, setOpen] = useState4(false);
  useEffect5(() => {
    const frame2 = requestAnimationFrame(() => setOpen(true));
    return () => cancelAnimationFrame(frame2);
  }, []);
  const card = CARDS.find((entry) => entry.key === cardKey) ?? CARDS[0];
  const note = cardNote(sources.length);
  const refused = note !== null;
  const takes = useMemo(() => {
    const byKey = /* @__PURE__ */ new Map();
    for (const result of [...results ?? []].reverse()) {
      const key = takeKey(result);
      const take = byKey.get(key);
      if (take) take.results.push(result);
      else byKey.set(key, { key, results: [result], request: requests.current.get(key) ?? requestOf(result) });
    }
    return [...byKey.values()].reverse();
  }, [results]);
  const make = async (request, label) => {
    setBusy(label);
    setError(null);
    const answer = await onMake(request);
    setBusy(null);
    if ("result" in answer) {
      const key = takeKey(answer.result);
      requests.current.set(key, request);
      setShown((current) => ({ ...current, [key]: Number.MAX_SAFE_INTEGER }));
      setFresh((current) => /* @__PURE__ */ new Set([...current, answer.result.artifact_id]));
      setArrived(answer.result.artifact_id);
      onPlay(answer.result.artifact_id);
      return;
    }
    setError(answer.status === 404 ? t("make.missing") : [400, 503, 504].includes(answer.status) && answer.error ? answer.error : t("make.failed", answer.error || String(answer.status)));
  };
  const optionRow = (entry) => entry.options.map((name) => {
    if (name === "attack_from") {
      return /* @__PURE__ */ jsxs11("label", { style: { display: "flex", flexDirection: "column", gap: 2, minWidth: 150, flex: "1 1 150px", fontSize: 11.5, color: T.secondary }, children: [
        t("make.options.attack_from"),
        /* @__PURE__ */ jsxs11(
          "select",
          {
            className: FOCUSABLE,
            onChange: (event) => setOptions((current) => ({ ...current, attack_from: event.target.value === "" ? void 0 : Number(event.target.value) })),
            style: { font: "inherit", fontSize: 12, padding: "1px 4px", borderRadius: 3, border: `1px solid ${T.border}`, background: "transparent", color: T.text },
            value: options.attack_from ?? "",
            children: [
              /* @__PURE__ */ jsx11("option", { value: "", children: t("make.auto") }),
              sources.slice(0, ATTACK_CHOICES).map((source, index) => /* @__PURE__ */ jsx11("option", { value: index, children: clean(source.name ?? source.sound_id ?? source.artifact_id ?? "", 40) }, `${index}`))
            ]
          }
        )
      ] }, name);
    }
    if (name === "split") return null;
    const labels = {
      seconds: (value) => t("make.optionValues.seconds", seconds(value)),
      bpm: (value) => t("make.optionValues.bpm", num(value, 0)),
      bars: (value) => t("make.optionValues.bars", value),
      depth: (value) => t("make.optionValues.depth", Math.round(value * 100)),
      stretch: (value) => t("make.optionValues.stretch", value)
    };
    return /* @__PURE__ */ jsx11(OptionSlider, { label: labels[name], name, onChange: (next) => setOptions((current) => ({ ...current, [name]: next })), value: options[name] }, name);
  });
  const tint = `color-mix(in srgb, ${T.accent} 16%, transparent)`;
  const chooser = /* @__PURE__ */ jsxs11("div", { style: { display: "flex", flexDirection: "column", gap: 7 }, children: [
    /* @__PURE__ */ jsx11("div", { "aria-label": t("make.way"), role: "radiogroup", style: { display: "grid", gridTemplateColumns: "repeat(3, 1fr)", border: `1px solid ${T.border}`, borderRadius: 8, overflow: "hidden" }, children: OPS.map((op, index) => {
      const chosen = card.op === op;
      return /* @__PURE__ */ jsxs11(
        "button",
        {
          "aria-checked": chosen,
          className: FOCUSABLE,
          disabled: refused,
          onClick: () => setCardKey(lastMode[op]),
          role: "radio",
          title: t(`make.opHints.${op}`),
          type: "button",
          style: {
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            gap: 6,
            minWidth: 0,
            font: "inherit",
            fontSize: 12.5,
            fontWeight: chosen ? 650 : 500,
            padding: "6px 4px",
            border: "none",
            borderLeft: index > 0 ? `1px solid ${T.border}` : "none",
            background: chosen ? tint : "transparent",
            color: refused ? T.faint : chosen ? T.accent : T.secondary,
            cursor: refused ? "default" : "pointer",
            transition: "background-color 140ms, color 140ms"
          },
          children: [
            /* @__PURE__ */ jsx11(Glyph, { card: OP_GLYPH[op], size: 14 }),
            /* @__PURE__ */ jsx11("span", { style: S.ellipsis, children: t(`make.ops.${op}`) })
          ]
        },
        op
      );
    }) }),
    card.op !== "stack" && /* @__PURE__ */ jsx11("div", { "aria-label": t(`make.ops.${card.op}`), role: "radiogroup", style: { ...S.row, flexWrap: "wrap", gap: 6 }, children: CARDS.filter((entry) => entry.op === card.op).map((entry) => {
      const chosen = entry.key === card.key;
      return /* @__PURE__ */ jsxs11(
        "button",
        {
          "aria-checked": chosen,
          className: FOCUSABLE,
          disabled: refused,
          onClick: () => {
            setCardKey(entry.key);
            setLastMode((current) => ({ ...current, [entry.op]: entry.key }));
          },
          role: "radio",
          title: t(`make.cards.${entry.key}.hint`),
          type: "button",
          style: {
            ...S.row,
            gap: 5,
            font: "inherit",
            fontSize: 12,
            fontWeight: chosen ? 600 : 500,
            padding: "3px 10px 3px 7px",
            borderRadius: 14,
            border: `1px solid ${chosen ? T.accent : T.border}`,
            background: chosen ? tint : "transparent",
            boxShadow: chosen ? `0 4px 14px -8px ${T.accent}` : "none",
            color: refused ? T.faint : chosen ? T.accent : T.secondary,
            cursor: refused ? "default" : "pointer",
            transition: "border-color 140ms, background-color 140ms, color 140ms, box-shadow 200ms"
          },
          children: [
            /* @__PURE__ */ jsx11(Glyph, { card: entry.key, size: 13 }),
            t(`make.cards.${entry.key}.title`)
          ]
        },
        entry.key
      );
    }) }),
    /* @__PURE__ */ jsx11("span", { style: { ...S.muted, fontSize: 11.5 }, children: t(`make.cards.${card.key}.hint`) })
  ] });
  return /* @__PURE__ */ jsxs11(
    "section",
    {
      "aria-label": t("make.title"),
      style: {
        ...inline ? {} : { position: "absolute", left: 8, right: 8, bottom: 8, maxHeight: "82%" },
        display: "flex",
        flexDirection: "column",
        borderRadius: 12,
        border: `1px solid ${T.border}`,
        background: T.raised,
        boxShadow: "0 18px 48px -18px rgba(0,0,0,.55)",
        color: T.text,
        transform: open ? "none" : "translateY(24px)",
        opacity: open ? 1 : 0,
        transition: "transform 260ms cubic-bezier(.2,.8,.2,1), opacity 200ms",
        zIndex: inline ? void 0 : 2
      },
      children: [
        /* @__PURE__ */ jsxs11("div", { style: { ...S.row, gap: 8, padding: "10px 12px 6px", borderBottom: `1px solid ${T.border}` }, children: [
          /* @__PURE__ */ jsx11("span", { style: { fontSize: 13.5, fontWeight: 650 }, children: t("make.title") }),
          /* @__PURE__ */ jsx11("span", { style: S.faint, children: t("make.from", sources.length) }),
          /* @__PURE__ */ jsx11("span", { style: { flex: 1 } }),
          /* @__PURE__ */ jsx11(TextButton, { label: t("make.close"), onClick: onClose, title: t("make.close"), children: "×" })
        ] }),
        /* @__PURE__ */ jsxs11("div", { style: { overflowY: "auto", padding: "8px 12px 12px", display: "flex", flexDirection: "column", gap: 10 }, children: [
          /* @__PURE__ */ jsxs11("div", { "aria-label": t("make.sources"), role: "list", style: { ...S.row, flex: "none", flexWrap: "nowrap", gap: 4, overflowX: "auto", paddingBottom: 2, scrollbarWidth: "thin" }, children: [
            sources.slice(0, SOURCE_CHIPS).map((source, index) => /* @__PURE__ */ jsxs11("span", { role: "listitem", style: { ...S.row, gap: 4, flex: "none", fontSize: 11, padding: "1px 7px 1px 5px", borderRadius: 10, border: `1px solid ${T.border}`, color: T.secondary, maxWidth: 160 }, children: [
              /* @__PURE__ */ jsx11("span", { style: { width: 7, height: 7, borderRadius: "50%", background: colorOf2(source), flex: "none" } }),
              /* @__PURE__ */ jsx11("span", { style: S.ellipsis, children: clean(source.name ?? source.sound_id ?? source.artifact_id ?? "", 40) })
            ] }, `${index}-${source.sound_id ?? source.artifact_id}`)),
            sources.length > SOURCE_CHIPS && /* @__PURE__ */ jsx11("span", { style: { ...S.faint, flex: "none" }, children: t("make.moreSources", sources.length - SOURCE_CHIPS) })
          ] }),
          chooser,
          card.options.length > 0 && /* @__PURE__ */ jsx11("div", { style: { ...S.row, flexWrap: "wrap", gap: 10, alignItems: "flex-end" }, children: optionRow(card) }),
          /* @__PURE__ */ jsxs11("div", { style: { ...S.row, gap: 8, flexWrap: "wrap" }, children: [
            /* @__PURE__ */ jsx11(
              "button",
              {
                className: FOCUSABLE,
                disabled: refused || busy !== null,
                onClick: () => void make(requestFor(card, sources, options), "make"),
                title: t("make.makeTitle", t(`make.cards.${card.key}.title`)),
                type: "button",
                style: {
                  font: "inherit",
                  fontSize: 13,
                  fontWeight: 650,
                  padding: "6px 16px",
                  borderRadius: 8,
                  border: `1px solid ${T.accent}`,
                  // The accent tinted, with the accent's own text: neither theme names a colour that reads on a solid accent.
                  background: refused ? "transparent" : `color-mix(in srgb, ${T.accent} 24%, transparent)`,
                  color: refused ? T.faint : T.accent,
                  cursor: refused || busy ? "default" : "pointer",
                  opacity: busy === "make" ? 0.75 : 1
                },
                children: busy === "make" ? t("make.making") : `${t("make.make")} · ${t(`make.cards.${card.key}.title`)}`
              }
            ),
            busy === "make" && /* @__PURE__ */ jsx11("span", { "aria-hidden": true, className: "kumi-spin", style: { width: 14, height: 14, borderRadius: "50%", border: `2px solid ${T.accent}`, borderRightColor: "transparent" } }),
            note && /* @__PURE__ */ jsx11("span", { style: S.faint, children: t(note.key, note.least) }),
            /* @__PURE__ */ jsx11("span", { "aria-live": "polite", style: error ? { ...S.faint, color: T.warn, flex: "1 1 14rem" } : S.hidden, children: error ?? "" })
          ] }),
          takes.length > 0 && /* @__PURE__ */ jsx11("div", { "aria-label": t("make.results"), role: "list", style: { display: "flex", flexDirection: "column", gap: 8 }, children: takes.map((take) => {
            const at = Math.min(take.results.length - 1, shown[take.key] ?? take.results.length - 1);
            const result = take.results[at];
            const isPlaying = playing === result.artifact_id;
            const inPalette = kept.has(result.artifact_id);
            return /* @__PURE__ */ jsxs11(
              "div",
              {
                className: fresh.has(result.artifact_id) ? "kumi-pop" : void 0,
                ref: (element) => {
                  if (element) cards.current.set(result.artifact_id, element);
                  else cards.current.delete(result.artifact_id);
                },
                role: "listitem",
                style: { display: "flex", flexDirection: "column", gap: 5, padding: "8px 10px", borderRadius: 10, border: `1px solid ${isPlaying ? T.accent : T.border}`, background: isPlaying ? `color-mix(in srgb, ${T.accent} 6%, transparent)` : "transparent", transition: "border-color 160ms, background-color 160ms" },
                children: [
                  /* @__PURE__ */ jsx11("span", { style: { ...S.text, fontSize: 12, lineHeight: 1.35, display: "-webkit-box", WebkitLineClamp: 2, WebkitBoxOrient: "vertical", overflow: "hidden" }, title: result.recipe, children: clean(result.recipe, 220) || resultName(result) }),
                  /* @__PURE__ */ jsxs11("div", { style: { ...S.row, gap: 8 }, children: [
                    /* @__PURE__ */ jsx11(
                      "button",
                      {
                        "aria-label": isPlaying ? t("make.stop") : t("make.play"),
                        className: FOCUSABLE,
                        onClick: () => isPlaying ? onStop() : onPlay(result.artifact_id),
                        type: "button",
                        style: {
                          flex: "none",
                          width: 30,
                          height: 30,
                          display: "flex",
                          alignItems: "center",
                          justifyContent: "center",
                          padding: 0,
                          borderRadius: "50%",
                          border: `1px solid ${T.accent}`,
                          background: isPlaying ? `color-mix(in srgb, ${T.accent} 30%, transparent)` : `color-mix(in srgb, ${T.accent} 12%, transparent)`,
                          color: T.accent,
                          fontSize: 11,
                          cursor: "pointer",
                          transition: "background-color 140ms"
                        },
                        children: isPlaying ? "■" : "▶"
                      }
                    ),
                    /* @__PURE__ */ jsx11("div", { "aria-hidden": true, onClick: () => isPlaying ? onStop() : onPlay(result.artifact_id), style: { flex: 1, minWidth: 0, cursor: "pointer" }, children: /* @__PURE__ */ jsx11(Wave2, { duration: result.duration, peaks: result.peaks, playing: isPlaying }) })
                  ] }),
                  /* @__PURE__ */ jsxs11("div", { style: { ...S.row, gap: 6, flexWrap: "wrap" }, children: [
                    /* @__PURE__ */ jsxs11("span", { style: S.faint, children: [
                      t(`make.ops.${result.op}`),
                      result.mode ? ` · ${t(`make.cards.${result.mode}.title`)}` : "",
                      " · ",
                      seconds(result.duration)
                    ] }),
                    /* @__PURE__ */ jsxs11("span", { style: { ...S.faint, fontSize: 10.5, fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace" }, title: t("make.seedTitle"), children: [
                      "#",
                      result.seed
                    ] }),
                    /* @__PURE__ */ jsx11("span", { style: { flex: 1 } }),
                    take.results.length > 1 && /* @__PURE__ */ jsxs11("span", { role: "group", "aria-label": t("make.variations"), style: { ...S.row, gap: 2 }, children: [
                      /* @__PURE__ */ jsx11(TextButton, { disabled: at === 0, label: t("make.previous"), onClick: () => setShown((current) => ({ ...current, [take.key]: at - 1 })), children: "‹" }),
                      /* @__PURE__ */ jsx11("span", { style: { ...S.faint, fontVariantNumeric: "tabular-nums" }, children: t("make.variation", at + 1, take.results.length) }),
                      /* @__PURE__ */ jsx11(TextButton, { disabled: at === take.results.length - 1, label: t("make.next"), onClick: () => setShown((current) => ({ ...current, [take.key]: at + 1 })), children: "›" })
                    ] })
                  ] }),
                  /* @__PURE__ */ jsxs11("div", { style: { ...S.row, gap: 6, flexWrap: "wrap" }, children: [
                    /* @__PURE__ */ jsx11(TextButton, { disabled: busy !== null, onClick: () => void make(reroll(take.request), take.key), title: t("make.rerollTitle"), children: busy === take.key ? t("make.rerolling") : /* @__PURE__ */ jsxs11(Fragment4, { children: [
                      /* @__PURE__ */ jsx11("span", { "aria-hidden": true, children: "↻ " }),
                      t("make.reroll")
                    ] }) }),
                    /* @__PURE__ */ jsxs11(
                      TextButton,
                      {
                        disabled: inPalette,
                        onClick: () => {
                          fly(result.artifact_id);
                          void onKeep(result);
                        },
                        pressed: inPalette,
                        title: t("make.keepTitle"),
                        children: [
                          /* @__PURE__ */ jsx11("span", { "aria-hidden": true, children: inPalette ? "✓ " : "+ " }),
                          inPalette ? t("make.kept") : t("make.keep")
                        ]
                      }
                    ),
                    use?.(result)
                  ] })
                ]
              },
              take.key
            );
          }) }),
          takes.length === 0 && results !== null && /* @__PURE__ */ jsx11("span", { style: S.faint, children: t("make.none") })
        ] }),
        flights.map((flight) => /* @__PURE__ */ jsx11(Flying, { flight, onDone: () => setFlights((current) => current.filter((other) => other.key !== flight.key)) }, flight.key))
      ]
    }
  );
}

// src/shared/MapCanvas.tsx
import { useEffect as useEffect6, useMemo as useMemo2, useRef as useRef3, useState as useState5 } from "react";

// src/shared/mapModel.ts
var GRID = 64;
var FALLBACK = ["#e85d4a", "#f5a623", "#50c8b4", "#7ed321", "#4a90e2", "#9b59b6", "#ec70c8", "#e5c440"];
function groupColor(color, key) {
  if (Array.isArray(color) && color.length === 3 && color.every((c) => Number.isInteger(c) && c >= 0 && c <= 255)) {
    return `rgb(${color[0]}, ${color[1]}, ${color[2]})`;
  }
  if (typeof color === "string" && /^#[0-9a-f]{6}$/i.test(color)) return color;
  return FALLBACK[Math.abs(key) % FALLBACK.length];
}
var cellOf = (x, y) => Math.min(GRID - 1, Math.floor(y * GRID)) * GRID + Math.min(GRID - 1, Math.floor(x * GRID));
function buildModel(map) {
  const points = [];
  const cells = /* @__PURE__ */ new Map();
  for (const raw of Array.isArray(map?.points) ? map.points : []) {
    if (!Array.isArray(raw) || raw.length < 4) continue;
    const [id, x, y, group] = raw;
    if (typeof id !== "string" || !Number.isFinite(x) || !Number.isFinite(y)) continue;
    const point = { id, x: Math.min(1, Math.max(0, x)), y: Math.min(1, Math.max(0, y)), group: Number.isInteger(group) ? group : -1 };
    const cell = cellOf(point.x, point.y);
    const list = cells.get(cell);
    if (list) list.push(points.length);
    else cells.set(cell, [points.length]);
    points.push(point);
  }
  const groups = Object.entries(map?.groups ?? {}).map(([key, group]) => ({
    key: Number(key),
    name: clean(group?.name ?? `group ${key}`, 32),
    color: groupColor(group?.color, Number(key)),
    count: typeof group?.count === "number" ? group.count : points.filter((point) => point.group === Number(key)).length
  })).filter((group) => Number.isInteger(group.key)).sort((a, b) => a.key - b.key);
  return { revision: map?.revision ?? null, points, groups, sounds: map?.sounds ?? {}, cells };
}
var TILE_ROOM = 1.12;
function tileLayout(model) {
  const points = model.points;
  const count2 = points.length;
  const COARSE = 40;
  const covered = new Uint8Array(COARSE * COARSE);
  for (const point of points) covered[Math.min(COARSE - 1, Math.floor(point.y * COARSE)) * COARSE + Math.min(COARSE - 1, Math.floor(point.x * COARSE))] = 1;
  const area = Math.max(0.05, covered.reduce((sum, cell) => sum + cell, 0) / (COARSE * COARSE));
  const side = Math.max(2, Math.ceil(Math.sqrt(Math.max(1, count2) * TILE_ROOM / area)));
  const span2 = side + 1;
  const right = new Int32Array(side * span2);
  const left = new Int32Array(side * span2);
  for (let r = 0; r < side; r++) {
    for (let c = 0; c < span2; c++) {
      right[r * span2 + c] = c;
      left[r * span2 + c] = c;
    }
  }
  const find = (links, base, at) => {
    let x = at;
    while (links[base + x] !== x) {
      links[base + x] = links[base + links[base + x]];
      x = links[base + x];
    }
    return x;
  };
  const col = new Int32Array(count2);
  const row2 = new Int32Array(count2);
  const cells = new Int32Array(side * side).fill(-1);
  let cx = 0;
  let cy = 0;
  for (const point of points) {
    cx += point.x / Math.max(1, count2);
    cy += point.y / Math.max(1, count2);
  }
  const order = points.map((_, index) => index).sort((a, b) => (points[a].x - cx) ** 2 + (points[a].y - cy) ** 2 - ((points[b].x - cx) ** 2 + (points[b].y - cy) ** 2) || a - b);
  for (const index of order) {
    const tx = points[index].x * side - 0.5;
    const ty = points[index].y * side - 0.5;
    const r0 = Math.min(side - 1, Math.max(0, Math.round(ty)));
    const c0 = Math.min(side - 1, Math.max(0, Math.round(tx)));
    let best = Infinity;
    let bestRow = -1;
    let bestCol = -1;
    const consider = (r, c) => {
      const distance = (c - tx) ** 2 + (r - ty) ** 2;
      if (distance < best) {
        best = distance;
        bestRow = r;
        bestCol = c;
      }
    };
    for (let dy = 0; dy < side; dy++) {
      const near = Math.max(0, dy - 0.5);
      if (near * near >= best) break;
      for (const r of dy === 0 ? [r0] : [r0 - dy, r0 + dy]) {
        if (r < 0 || r >= side) continue;
        const base = r * span2;
        const after = find(right, base, c0);
        if (after < side) consider(r, after);
        const before = find(left, base, c0 + 1) - 1;
        if (before >= 0) consider(r, before);
      }
    }
    col[index] = bestCol;
    row2[index] = bestRow;
    cells[bestRow * side + bestCol] = index;
    right[bestRow * span2 + bestCol] = bestCol + 1;
    left[bestRow * span2 + bestCol + 1] = bestCol;
  }
  return { side, col, row: row2, cells };
}
function tileModel(model, layout) {
  const cells = /* @__PURE__ */ new Map();
  const points = model.points.map((point, index) => {
    const tiled = { ...point, x: (layout.col[index] + 0.5) / layout.side, y: (layout.row[index] + 0.5) / layout.side };
    const cell = cellOf(tiled.x, tiled.y);
    const list = cells.get(cell);
    if (list) list.push(index);
    else cells.set(cell, [index]);
    return tiled;
  });
  return { ...model, points, cells };
}
function fitView(width, height, margin = 16) {
  const size = Math.max(1, Math.min(width, height) - 2 * margin);
  return { scale: 1, dx: (width - size) / 2, dy: (height - size) / 2 };
}
function toScreen(view, size, x, y) {
  return [x * size * view.scale + view.dx, y * size * view.scale + view.dy];
}
function zoomAt(view, factor, px, py) {
  const scale = Math.min(40, Math.max(1, view.scale * factor));
  const applied = scale / view.scale;
  return { scale, dx: px - (px - view.dx) * applied, dy: py - (py - view.dy) * applied };
}
var WHEEL_GESTURE_MS = 300;
var MAC_WHEEL_STEP = 4.000244140625;
function wheelPans(event, memory, now) {
  const going = memory.pans !== null && now - memory.at <= WHEEL_GESTURE_MS;
  memory.at = now;
  if (going) return memory.pans;
  const vertical = Math.abs(event.deltaY) >= Math.abs(event.deltaX);
  const delta = vertical ? event.deltaY : event.deltaX;
  const legacy = vertical ? event.wheelDeltaY : event.wheelDeltaX;
  if (event.deltaMode !== 0 || delta !== 0 && delta % MAC_WHEEL_STEP === 0) memory.pans = false;
  else if (typeof legacy === "number" && legacy !== 0) memory.pans = legacy % 120 !== 0;
  else memory.pans = true;
  return memory.pans;
}
function pick(model, view, size, px, py, radius = 8, visible) {
  const unit = size * view.scale;
  if (unit <= 0) return -1;
  const mx = (px - view.dx) / unit;
  const my = (py - view.dy) / unit;
  const reach = radius / unit;
  const from = { x: Math.max(0, Math.floor((mx - reach) * GRID)), y: Math.max(0, Math.floor((my - reach) * GRID)) };
  const to = { x: Math.min(GRID - 1, Math.floor((mx + reach) * GRID)), y: Math.min(GRID - 1, Math.floor((my + reach) * GRID)) };
  let best = -1;
  let bestDistance = reach * reach;
  for (let cy = from.y; cy <= to.y; cy++) {
    for (let cx = from.x; cx <= to.x; cx++) {
      for (const index of model.cells.get(cy * GRID + cx) ?? []) {
        if (visible && !visible(index)) continue;
        const point = model.points[index];
        const distance = (point.x - mx) ** 2 + (point.y - my) ** 2;
        if (distance <= bestDistance) {
          bestDistance = distance;
          best = index;
        }
      }
    }
  }
  return best;
}
function mapKey(event) {
  if (event.altKey || event.ctrlKey || event.metaKey) return null;
  const direction = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] }[event.key];
  if (direction) {
    if (!event.shiftKey) return { move: direction };
    return direction[0] ? { order: direction[0] } : { zoom: direction[1] < 0 ? 1.5 : 1 / 1.5 };
  }
  return event.key === "Home" && !event.shiftKey ? { fit: true } : null;
}
function moveWith(model, view, size, from, key, middle, visible) {
  return "move" in key ? step(model, view, size, from, key.move, middle, visible) : stepOrder(model, from, key.order, visible);
}
function stepOrder(model, from, delta, visible) {
  const count2 = model.points.length;
  let index = from ?? (delta > 0 ? -1 : count2);
  for (let tried = 0; tried < count2; tried++) {
    index = ((index + delta) % count2 + count2) % count2;
    if (index !== from && (!visible || visible(index))) return index;
  }
  return -1;
}
function step(model, view, size, from, direction, middle, visible) {
  const origin = from === void 0 ? middle : toScreen(view, size, model.points[from].x, model.points[from].y);
  const best = [-1, -1];
  const bestScore = [Infinity, Infinity];
  model.points.forEach((point, index) => {
    if (index === from || visible && !visible(index)) return;
    const [x, y] = toScreen(view, size, point.x, point.y);
    const ox = x - origin[0];
    const oy = y - origin[1];
    let score;
    let side = 0;
    if (from === void 0) {
      score = ox * ox + oy * oy;
    } else {
      const along = ox * direction[0] + oy * direction[1];
      if (along <= 0.5) return;
      const across = Math.abs(ox * direction[1] - oy * direction[0]);
      score = along + 2 * across;
      side = across <= along ? 0 : 1;
    }
    if (score < bestScore[side]) {
      best[side] = index;
      bestScore[side] = score;
    }
  });
  return best[0] >= 0 ? best[0] : best[1];
}
function keepInView(view, px, py, width, height, margin = 24) {
  const shiftX = px < margin ? margin - px : px > width - margin ? width - margin - px : 0;
  const shiftY = py < margin ? margin - py : py > height - margin ? height - margin - py : 0;
  return shiftX || shiftY ? { ...view, dx: view.dx + shiftX, dy: view.dy + shiftY } : view;
}
function mapLabel(name) {
  return clean(name, 28);
}
var haystacks = /* @__PURE__ */ new WeakMap();
function haystackOf(model) {
  let text = haystacks.get(model);
  if (!text) {
    text = model.points.map((point) => {
      const sound = model.sounds[point.id];
      return [sound?.name, sound?.class, sound?.kind, sound?.folder].filter((value) => typeof value === "string").join(" ").toLowerCase();
    });
    haystacks.set(model, text);
  }
  return text;
}
function indexedValues(sound, t = english) {
  if (!sound) return "";
  const parts = [];
  if (typeof sound.bpm === "number" && Number.isFinite(sound.bpm) && sound.bpm > 0) parts.push(`${num(sound.bpm, 1)} BPM`);
  const key = clean(sound.key, 24);
  if (key) parts.push(key);
  const note = clean(sound.note, 8);
  if (note) parts.push(t("map.note", note));
  return parts.join(" · ");
}
function foundLine(match, near, t = english) {
  if (!match) return "";
  const closeness = near && typeof match.closeness === "number" ? [t("map.closeness", String(match.closeness))] : [];
  return [...closeness, ...match.why].join(" · ");
}
function searchSounds(model, query) {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return null;
  const text = haystackOf(model);
  const found = /* @__PURE__ */ new Set();
  model.points.forEach((point, index) => {
    if (words.every((word) => text[index].includes(word))) found.add(point.id);
  });
  return found;
}
function frame(model, ids, width, height, margin = 48) {
  const chosen = model.points.filter((point) => ids.has(point.id));
  if (!chosen.length) return null;
  const xs = chosen.map((point) => point.x);
  const ys = chosen.map((point) => point.y);
  const [x0, x1, y0, y1] = [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)];
  const size = Math.max(1, Math.min(width, height) - 32);
  const span2 = Math.max(x1 - x0, y1 - y0, 0.04);
  const scale = Math.min(40, Math.max(1, (Math.min(width, height) - 2 * margin) / (span2 * size)));
  const cx = (x0 + x1) / 2;
  const cy = (y0 + y1) / 2;
  return { scale, dx: width / 2 - cx * size * scale, dy: height / 2 - cy * size * scale };
}
function pointsInBox(model, view, size, box, visible) {
  const unit = size * view.scale;
  if (unit <= 0) return [];
  const [ax, ay, bx, by] = box;
  const x0 = (Math.min(ax, bx) - view.dx) / unit;
  const x1 = (Math.max(ax, bx) - view.dx) / unit;
  const y0 = (Math.min(ay, by) - view.dy) / unit;
  const y1 = (Math.max(ay, by) - view.dy) / unit;
  if (x1 < 0 || y1 < 0 || x0 > 1 || y0 > 1) return [];
  const cell = (value) => Math.min(GRID - 1, Math.max(0, Math.floor(value * GRID)));
  const out = [];
  for (let cy = cell(y0); cy <= cell(y1); cy++) {
    for (let cx = cell(x0); cx <= cell(x1); cx++) {
      for (const index of model.cells.get(cy * GRID + cx) ?? []) {
        const point = model.points[index];
        if (point.x >= x0 && point.x <= x1 && point.y >= y0 && point.y <= y1 && (!visible || visible(index))) out.push(index);
      }
    }
  }
  return out.sort((a, b) => a - b);
}

// src/shared/MapCanvas.tsx
import { jsx as jsx12, jsxs as jsxs12 } from "react/jsx-runtime";
var MATCHES_LISTED = 6;
var FADE_MS = 450;
var INTRO_MS = 900;
var MORPH_MS = 760;
var FLY_MS = 620;
var FLY_STAGGER = 14;
var FLY_MOST = 48;
var RIPPLE_MS = 1100;
var STAGGER = 0.55;
var clamp01 = (value) => Math.min(1, Math.max(0, value));
var easeInOut = (t) => t < 0.5 ? 4 * t * t * t : 1 - (-2 * t + 2) ** 3 / 2;
var easeOutBack = (t) => 1 + 2.2 * (t - 1) ** 3 + 1.2 * (t - 1) ** 2;
function morphAt(morph, now) {
  return morph.from + (morph.to - morph.from) * easeInOut(clamp01((now - morph.at) / MORPH_MS));
}
function resolve(canvas) {
  const style = getComputedStyle(canvas);
  const read = (name, fallback) => style.getPropertyValue(name).trim() || fallback;
  return {
    text: read(CANVAS.text, "#999"),
    faint: read(CANVAS.faint, "#666"),
    border: read(CANVAS.border, "#444"),
    accent: read(CANVAS.accent, "#4a90e2"),
    strong: read(CANVAS.strong, "#ddd")
  };
}
var ROUNDS = 8;
var maps = 0;
function MapCanvas({ map, found, playing, selected, onPick, onSelect, actions, empty, leaving, hiddenActions, marked, picked, onPicked, onHover, tools, pickedActions, tiles = false, onTiles, flying, wheel = "auto" }) {
  const t = useT();
  const model = useMemo2(() => buildModel(map), [map]);
  const canvasRef = useRef3(null);
  const boxRef = useRef3(null);
  const [size, setSize] = useState5({ width: 0, height: 0 });
  const [view, setView] = useState5(null);
  const [query, setQuery] = useState5("");
  const [searchFor, setSearchFor] = useState5("");
  const [hidden, setHidden] = useState5(() => /* @__PURE__ */ new Set());
  const [hover, setHover] = useState5(null);
  const drag = useRef3(null);
  const [box, setBox] = useState5(null);
  const [handTool, setHandTool] = useState5(false);
  const [grabbing, setGrabbing] = useState5(false);
  const over = useRef3(false);
  const [spaceHeld, setSpaceHeld] = useState5(false);
  const space = useRef3(false);
  useEffect6(() => {
    const hold = (held) => {
      if (space.current === held) return;
      space.current = held;
      setSpaceHeld(held);
    };
    const down = (event) => {
      if (event.code !== "Space" || event.defaultPrevented || !over.current) return;
      const target = event.target;
      if (target?.isContentEditable || target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) return;
      event.preventDefault();
      hold(true);
    };
    const up = (event) => {
      if (event.code === "Space") hold(false);
    };
    const blur = () => hold(false);
    window.addEventListener("keydown", down, true);
    window.addEventListener("keyup", up, true);
    window.addEventListener("blur", blur);
    return () => {
      window.removeEventListener("keydown", down, true);
      window.removeEventListener("keyup", up, true);
      window.removeEventListener("blur", blur);
    };
  }, []);
  const rested = useRef3(null);
  const rest = (id) => {
    if (rested.current === id) return;
    rested.current = id;
    onHover?.(id);
  };
  const viewRef = useRef3(null);
  viewRef.current = view;
  const nextView = useRef3(null);
  const frameRef = useRef3(0);
  const queueView = (next) => {
    nextView.current = next;
    if (frameRef.current) return;
    frameRef.current = requestAnimationFrame(() => {
      frameRef.current = 0;
      const latest = nextView.current;
      nextView.current = null;
      if (latest) setView(latest);
    });
  };
  useEffect6(() => () => cancelAnimationFrame(frameRef.current), []);
  const backing = useRef3("");
  const [said, setSaid] = useState5("");
  const [hintId] = useState5(() => `kumi-map-keys-${maps += 1}`);
  const mapSize = Math.max(1, Math.min(size.width, size.height) - 32);
  const hasPoints = model.points.length > 0;
  useEffect6(() => {
    if (query === searchFor) return;
    const timer = setTimeout(() => setSearchFor(query), query ? 150 : 0);
    return () => clearTimeout(timer);
  }, [query, searchFor]);
  const searched = useMemo2(() => searchSounds(model, searchFor), [model, searchFor]);
  const indexOf = useMemo2(() => new Map(model.points.map((point, index) => [point.id, index])), [model]);
  const foundIds = useMemo2(
    () => found && (found.revision === null || found.revision === model.revision) ? new Set(found.ids) : null,
    [found, model.revision]
  );
  const mask = useMemo2(() => {
    const out = new Uint8Array(model.points.length);
    model.points.forEach((point, index) => {
      out[index] = leaving?.has(point.id) ? 2 : hidden.size > 0 && hidden.has(point.group) || searched !== null && !searched.has(point.id) ? 0 : 1;
    });
    return out;
  }, [model, hidden, searched, leaving]);
  const visible = useMemo2(() => (index) => mask[index] === 1, [mask]);
  const shown = useMemo2(() => mask.reduce((count2, keep) => count2 + (keep === 1 ? 1 : 0), 0), [mask]);
  const leavingCount = useMemo2(() => mask.reduce((count2, keep) => count2 + (keep === 2 ? 1 : 0), 0), [mask]);
  const still = useMemo2(() => typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches, []);
  const leftAt = useRef3(/* @__PURE__ */ new Map());
  const introAt = useRef3(-Infinity);
  const until = useRef3(0);
  const [clock2, setClock] = useState5(0);
  const animate = (ms2) => {
    until.current = Math.max(until.current, performance.now() + ms2);
    setClock(performance.now());
  };
  useEffect6(() => {
    const now = performance.now();
    for (const id of leaving ?? []) if (!leftAt.current.has(id)) leftAt.current.set(id, now);
    for (const id of [...leftAt.current.keys()]) if (!leaving?.has(id)) leftAt.current.delete(id);
    if (leaving?.size) animate(FADE_MS);
  }, [leaving]);
  useEffect6(() => {
    if (still || !model.revision) return;
    introAt.current = performance.now();
    animate(INTRO_MS);
  }, [model.revision]);
  const [morph, setMorph] = useState5(() => ({ from: tiles ? 1 : 0, to: tiles ? 1 : 0, at: -Infinity }));
  useEffect6(() => {
    const now = performance.now();
    setMorph((current) => current.to === (tiles ? 1 : 0) ? current : { from: still ? tiles ? 1 : 0 : morphAt(current, now), to: tiles ? 1 : 0, at: now });
    if (!still) animate(MORPH_MS);
  }, [tiles]);
  useEffect6(() => {
    if (flying && !still) animate(FLY_MS + FLY_STAGGER * FLY_MOST);
  }, [flying]);
  const playingHere = playing !== null && indexOf.has(playing) && !still;
  useEffect6(() => {
    if (performance.now() >= until.current && !playingHere) return;
    const frame2 = requestAnimationFrame(setClock);
    return () => cancelAnimationFrame(frame2);
  }, [clock2, playingHere]);
  const layout = useMemo2(() => tiles || morph.from > 0 ? tileLayout(model) : null, [model, tiles, morph.from > 0]);
  const tiled = useMemo2(() => layout ? tileModel(model, layout) : null, [model, layout]);
  const active = tiles && tiled ? tiled : model;
  const spread = useMemo2(() => {
    const points = model.points;
    const out = new Float32Array(points.length);
    if (points.length === 0) return out;
    const cx = points.reduce((sum, point) => sum + point.x, 0) / points.length;
    const cy = points.reduce((sum, point) => sum + point.y, 0) / points.length;
    let far = 0;
    points.forEach((point, index) => {
      out[index] = Math.hypot(point.x - cx, point.y - cy);
      far = Math.max(far, out[index]);
    });
    if (far > 0) for (let index = 0; index < out.length; index++) out[index] /= far;
    return out;
  }, [model]);
  const matches = useMemo2(() => {
    const ids = searched ?? foundIds;
    return ids ? [...ids].filter((id) => indexOf.has(id) && visible(indexOf.get(id))) : [];
  }, [searched, foundIds, indexOf, visible]);
  const pickedIds = useMemo2(() => picked && picked.size > 0 ? model.points.filter((point) => picked.has(point.id)).map((point) => point.id) : [], [model, picked]);
  const nearName = useMemo2(() => {
    const seeds = !searched && foundIds ? (found?.near ?? []).filter((id) => model.sounds[id]) : [];
    if (seeds.length === 0) return "";
    const first = clean(model.sounds[seeds[0]].name ?? "", 60);
    return seeds.length === 1 ? t("map.near", first) : t("map.nearMany", first, seeds.length - 1);
  }, [searched, foundIds, found, model, t]);
  const [painted, setPainted] = useState5("");
  useEffect6(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const timer = setInterval(() => {
      const now = Object.values(resolve(canvas)).join();
      setPainted((current) => current === now ? current : now);
    }, 1e3);
    return () => clearInterval(timer);
  }, [hasPoints]);
  useEffect6(() => {
    const box2 = boxRef.current;
    if (!box2) return;
    const observer = new ResizeObserver((entries) => {
      const rect = entries[0]?.contentRect;
      if (rect) setSize({ width: Math.floor(rect.width), height: Math.floor(rect.height) });
    });
    observer.observe(box2);
    return () => observer.disconnect();
  }, [hasPoints]);
  useEffect6(() => {
    if (size.width > 0 && size.height > 0) setView((current) => current ?? fitView(size.width, size.height));
  }, [size.width, size.height]);
  useEffect6(() => setView(size.width > 0 ? fitView(size.width, size.height) : null), [model.revision]);
  useEffect6(() => {
    if (!foundIds || size.width === 0) return;
    const framed = frame(model, foundIds, size.width, size.height);
    if (framed) setView(framed);
  }, [foundIds, model, size.width, size.height]);
  const wheelMode = useRef3(wheel);
  wheelMode.current = wheel;
  useEffect6(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const memory = { pans: null, at: 0 };
    const onWheel = (event) => {
      event.preventDefault();
      const base = nextView.current ?? viewRef.current;
      if (!base) return;
      const legacy = event;
      const pans = !event.ctrlKey && (wheelMode.current === "auto" ? wheelPans(legacy, memory, performance.now()) : wheelMode.current === "pan");
      const unit = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? 400 : 1;
      if (pans) {
        queueView({ ...base, dx: base.dx - event.deltaX * unit, dy: base.dy - event.deltaY * unit });
        return;
      }
      const rect = canvas.getBoundingClientRect();
      const factor = Math.exp(-event.deltaY * unit * (event.ctrlKey ? 0.01 : 25e-4));
      queueView(zoomAt(base, factor, event.clientX - rect.left, event.clientY - rect.top));
    };
    canvas.addEventListener("wheel", onWheel, { passive: false });
    return () => canvas.removeEventListener("wheel", onWheel);
  }, [hasPoints]);
  const hoverAt = hover ? hover.index : null;
  useEffect6(() => {
    const canvas = canvasRef.current;
    if (!canvas || !view || size.width === 0) return;
    const ratio = window.devicePixelRatio || 1;
    const wanted = `${Math.round(size.width * ratio)}x${Math.round(size.height * ratio)}`;
    if (backing.current !== wanted) {
      backing.current = wanted;
      canvas.width = Math.round(size.width * ratio);
      canvas.height = Math.round(size.height * ratio);
    }
    const context = canvas.getContext("2d");
    if (!context) return;
    const colors = resolve(canvas);
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, size.width, size.height);
    const now = performance.now();
    const points = model.points;
    const count2 = points.length;
    const base = count2 <= 40 ? 5 : count2 <= 300 ? 3.2 : 2.2;
    const radius = Math.max(1.5, Math.min(base + 3, base + Math.log2(view.scale) * 0.8));
    const groupColor2 = new Map(model.groups.map((group) => [group.key, group.color]));
    const unit = mapSize * view.scale;
    const [left, top, right, bottom] = [-8, -8, size.width + 8, size.height + 8];
    const morphed = still ? morph.to : morphAt(morph, now);
    const tilePoints = tiled && morphed > 0 ? tiled.points : null;
    const tileSide = layout ? unit / layout.side * 0.86 : 0;
    const blendOf = (index) => tilePoints ? clamp01(morphed * (1 + STAGGER) - spread[index] * STAGGER) : 0;
    const placeOf = (index) => {
      const point = points[index];
      const blend = blendOf(index);
      const x = tilePoints ? point.x + (tilePoints[index].x - point.x) * blend : point.x;
      const y = tilePoints ? point.y + (tilePoints[index].y - point.y) * blend : point.y;
      return [x * unit + view.dx, y * unit + view.dy, blend];
    };
    const introFor = (index) => {
      const since = now - introAt.current - spread[index] * (INTRO_MS * 0.45);
      return since >= INTRO_MS * 0.55 ? 1 : since <= 0 ? 0 : easeOutBack(since / (INTRO_MS * 0.55));
    };
    const shape = (path, x, y, blend, grow = 1) => {
      if (blend <= 0) {
        const r = radius * grow;
        if (r <= 0.05) return;
        path.moveTo(x + r, y);
        path.arc(x, y, r, 0, Math.PI * 2);
        return;
      }
      const side = (2 * radius + (tileSide - 2 * radius) * blend) * grow;
      if (side <= 0.1) return;
      const corner = side / 2 - (side / 2 - side * 0.18) * blend;
      if (typeof path.roundRect === "function") path.roundRect(x - side / 2, y - side / 2, side, side, corner);
      else path.rect(x - side / 2, y - side / 2, side, side);
    };
    const reach = (blend) => radius + (Math.max(radius, tileSide * 0.62) - radius) * blend;
    const labelled = [];
    let onScreen = 0;
    for (const pass of [0, 1]) {
      const rounds = Array.from({ length: ROUNDS }, () => /* @__PURE__ */ new Map());
      for (let index = 0; index < count2; index++) {
        if (mask[index] !== pass) continue;
        const [x, y, blend] = placeOf(index);
        if (x < left || y < top || x > right || y > bottom) continue;
        const color = groupColor2.get(points[index].group) ?? colors.faint;
        const paths = rounds[index % ROUNDS];
        let path = paths.get(color);
        if (!path) {
          path = new Path2D();
          paths.set(color, path);
        }
        shape(path, x, y, blend, introFor(index));
        if (pass === 1 && ++onScreen <= 41) labelled.push([x, y, model.sounds[points[index].id]?.name ?? ""]);
      }
      context.globalAlpha = pass ? 0.92 : 0.16;
      for (const paths of rounds) {
        for (const [color, path] of paths) {
          context.fillStyle = color;
          context.fill(path);
        }
      }
    }
    for (const [id, at2] of leftAt.current) {
      const progress = (now - at2) / FADE_MS;
      const index = indexOf.get(id);
      if (progress >= 1 || index === void 0) continue;
      const eased = 1 - (1 - progress) ** 2;
      const [x, y, blend] = placeOf(index);
      const path = new Path2D();
      shape(path, x, y, blend, 1 - eased + 0.02);
      context.globalAlpha = 0.92 * (1 - eased);
      context.fillStyle = groupColor2.get(points[index].group) ?? colors.faint;
      context.fill(path);
    }
    if (onScreen <= 40) {
      context.globalAlpha = 0.85;
      context.fillStyle = colors.text;
      context.font = `11px ${getComputedStyle(canvas).fontFamily || "sans-serif"}`;
      context.textBaseline = "middle";
      const placed = [];
      const gap = reach(morphed) + 4;
      for (const [x, y, name] of labelled) {
        const text = mapLabel(name);
        if (!text) continue;
        const box2 = [x + gap, y - 7, context.measureText(text).width, 14];
        if (placed.some(([left2, top2, width, height]) => box2[0] < left2 + width && left2 < box2[0] + box2[2] && box2[1] < top2 + height && top2 < box2[1] + box2[3])) continue;
        placed.push(box2);
        context.fillText(text, box2[0], y);
      }
    }
    context.globalAlpha = 1;
    const rings = (ids, extra) => {
      if (!ids || ids.size === 0) return null;
      const path = new Path2D();
      for (const id of ids) {
        const index = indexOf.get(id);
        if (index === void 0 || mask[index] === 2) continue;
        const [x, y, blend] = placeOf(index);
        if (x < left || y < top || x > right || y > bottom) continue;
        const r = reach(blend) + extra;
        path.moveTo(x + r, y);
        path.arc(x, y, r, 0, Math.PI * 2);
      }
      return path;
    };
    const inPalette = rings(marked, 2.2);
    if (inPalette) {
      context.globalAlpha = 0.8;
      context.strokeStyle = colors.accent;
      context.lineWidth = 1;
      context.stroke(inPalette);
      context.globalAlpha = 1;
    }
    const inPicks = rings(picked, 1.4);
    if (inPicks) {
      context.strokeStyle = colors.strong;
      context.lineWidth = 1.6;
      context.stroke(inPicks);
    }
    const ring = (id, color, extra, width) => {
      const index = indexOf.get(id);
      if (index === void 0) return;
      const [x, y, blend] = placeOf(index);
      context.strokeStyle = color;
      context.lineWidth = width;
      context.beginPath();
      context.arc(x, y, reach(blend) + extra, 0, Math.PI * 2);
      context.stroke();
    };
    foundIds?.forEach((id) => ring(id, colors.accent, 3, 1.5));
    const hovered2 = hoverAt;
    if (hovered2 !== null && hovered2 < count2 && mask[hovered2] === 1) {
      const [x, y, blend] = placeOf(hovered2);
      const color = groupColor2.get(points[hovered2].group) ?? colors.faint;
      const path = new Path2D();
      shape(path, x, y, blend, 1.45);
      context.save();
      context.shadowColor = color;
      context.shadowBlur = 14;
      context.fillStyle = color;
      context.fill(path);
      context.restore();
      const ring2 = new Path2D();
      shape(ring2, x, y, blend, 2.3);
      context.strokeStyle = colors.strong;
      context.lineWidth = 1.25;
      context.stroke(ring2);
    }
    if (playing) {
      ring(playing, colors.accent, 6, 2);
      const index = indexOf.get(playing);
      if (index !== void 0 && !still) {
        const [x, y, blend] = placeOf(index);
        for (const offset of [0, 0.5]) {
          const phase = ((now / RIPPLE_MS + offset) % 1 + 1) % 1;
          context.globalAlpha = 0.65 * (1 - phase) ** 1.5;
          context.strokeStyle = groupColor2.get(points[index].group) ?? colors.accent;
          context.lineWidth = 2 - phase;
          context.beginPath();
          context.arc(x, y, reach(blend) + 4 + phase * 26, 0, Math.PI * 2);
          context.stroke();
        }
        context.globalAlpha = 1;
      }
    }
    if (selected) ring(selected, colors.strong, 4, 2);
    if (flying && !still) {
      const target = [size.width / 2, size.height + 24];
      flying.ids.slice(0, FLY_MOST).forEach((id, order) => {
        const index = indexOf.get(id);
        if (index === void 0) return;
        const progress = clamp01((now - flying.at - order * FLY_STAGGER) / FLY_MS);
        if (progress <= 0 || progress >= 1) return;
        const eased = progress * progress * (3 - 2 * progress);
        const [x0, y0] = placeOf(index);
        const x = x0 + (target[0] - x0) * eased;
        const y = y0 + (target[1] - y0) * eased - Math.sin(progress * Math.PI) * 28;
        context.globalAlpha = 0.95 - 0.5 * progress;
        context.fillStyle = groupColor2.get(points[index].group) ?? colors.accent;
        context.beginPath();
        context.arc(x, y, Math.max(1.4, (radius + 2.5) * (1 - 0.55 * progress)), 0, Math.PI * 2);
        context.fill();
        context.strokeStyle = colors.strong;
        context.lineWidth = 0.9;
        context.stroke();
      });
      context.globalAlpha = 1;
    }
  }, [model, indexOf, view, size, mask, foundIds, selected, playing, mapSize, painted, clock2, marked, picked, tiled, layout, morph, spread, flying, still, hoverAt]);
  const at = (event) => {
    const rect = canvasRef.current?.getBoundingClientRect();
    return rect ? [event.clientX - rect.left, event.clientY - rect.top] : [0, 0];
  };
  const reachPx = (base) => tiles && layout && view ? Math.max(base, mapSize * view.scale / layout.side * 0.62) : base;
  const press = (event) => {
    if (!view || event.button !== 0 && event.button !== 1) return;
    if (event.button === 1) event.preventDefault();
    event.currentTarget.setPointerCapture?.(event.pointerId);
    const [x, y] = at(event);
    const boxing = event.button === 0 && Boolean(onPicked) && !space.current && !handTool;
    drag.current = { x, y, moved: false, view, box: boxing, add: event.shiftKey || event.metaKey || event.ctrlKey, button: event.button };
    rest(null);
    setHover(null);
    if (boxing) setBox([x, y, x, y]);
    else setGrabbing(true);
  };
  const release = (event) => {
    const held = drag.current;
    drag.current = null;
    setBox(null);
    setGrabbing(false);
    if (!held || !view) return;
    const [x, y] = at(event);
    if (held.box && held.moved) {
      const inside = pointsInBox(active, view, mapSize, [held.x, held.y, x, y], visible).map((index2) => model.points[index2].id);
      onPicked?.(inside, held.add ? "add" : "replace");
      return;
    }
    if (held.moved || held.button !== 0) return;
    const index = pick(active, view, mapSize, x, y, reachPx(10), visible);
    const adding = event.shiftKey || event.metaKey || event.ctrlKey;
    if (index < 0) {
      if (adding) return;
      if (pickedIds.length > 0) onPicked?.([], "replace");
      if (selected !== null) onSelect(null);
      return;
    }
    const id = model.points[index].id;
    if (onPicked && (event.metaKey || event.ctrlKey)) onPicked([id], "toggle");
    else if (onPicked && event.shiftKey) onPicked([id], "add");
    else onPick(id);
  };
  const onKey = (event) => {
    if (event.key === "Escape" && !event.shiftKey && !event.altKey && !event.ctrlKey && !event.metaKey && pickedIds.length > 0 && onPicked) {
      event.preventDefault();
      onPicked([], "replace");
      setSaid(t("dock.cleared"));
      return;
    }
    const key = mapKey(event);
    if (!view || !key) return;
    event.preventDefault();
    const from = selected === null ? void 0 : indexOf.get(selected);
    const picked2 = from !== void 0 && visible(from) ? from : void 0;
    const middle = [size.width / 2, size.height / 2];
    if ("fit" in key) {
      setView(fitView(size.width, size.height));
    } else if ("zoom" in key) {
      const [x, y] = picked2 === void 0 ? middle : toScreen(view, mapSize, active.points[picked2].x, active.points[picked2].y);
      setView(zoomAt(view, key.zoom, x, y));
    } else {
      const next = moveWith(active, view, mapSize, picked2, key, middle, visible);
      if (next < 0) return;
      const point = active.points[next];
      const [x, y] = toScreen(view, mapSize, point.x, point.y);
      setView(keepInView(view, x, y, size.width, size.height));
      onSelect(point.id);
      const sound = model.sounds[point.id];
      setSaid([clean(sound?.name ?? point.id, 80), clean(sound?.class, 16), clean(sound?.folder, 60)].filter(Boolean).join(", "));
    }
  };
  if (!model.points.length) {
    return /* @__PURE__ */ jsxs12("div", { style: { ...S.muted, padding: 16 }, children: [
      empty,
      (map?.hidden ?? 0) > 0 && /* @__PURE__ */ jsxs12("div", { role: "status", style: { ...S.row, flexWrap: "wrap", gap: 6, marginTop: 8 }, children: [
        /* @__PURE__ */ jsx12("span", { style: { ...S.faint, flex: "1 1 14rem" }, children: t("map.hidden", map?.hidden ?? 0) }),
        hiddenActions
      ] })
    ] });
  }
  const chosen = selected ? model.sounds[selected] : void 0;
  const hovered = hover ? model.points[hover.index] : void 0;
  const hoveredSound = hovered ? model.sounds[hovered.id] : void 0;
  const countWords = shown === model.points.length - leavingCount ? t("map.count", num(shown, 0)) : t("map.countOf", num(shown, 0), num(model.points.length - leavingCount, 0));
  return /* @__PURE__ */ jsxs12("div", { style: { display: "flex", flexDirection: "column", height: "100%", minHeight: 240, color: T.text }, children: [
    /* @__PURE__ */ jsxs12("div", { style: { ...S.row, flexWrap: "wrap", padding: "8px 10px", gap: 6, borderBottom: `1px solid ${T.border}` }, children: [
      /* @__PURE__ */ jsxs12("div", { style: { position: "relative", display: "flex", flex: "1 1 160px", minWidth: 130 }, children: [
        /* @__PURE__ */ jsx12(
          "input",
          {
            "aria-label": t("map.search"),
            className: FOCUSABLE,
            onChange: (event) => setQuery(event.target.value),
            placeholder: t("map.searchHint"),
            style: { flex: 1, minWidth: 0, font: "inherit", fontSize: 12, padding: "3px 8px", paddingRight: `${countWords.length * 0.55 + 1.2}em`, borderRadius: 3, border: `1px solid ${T.border}`, background: "transparent", color: T.text },
            value: query
          }
        ),
        /* @__PURE__ */ jsx12(
          "span",
          {
            style: { ...S.faint, fontSize: 10.5, lineHeight: "16px", padding: "0 6px", borderRadius: 8, background: T.chip, position: "absolute", right: 4, top: "50%", transform: "translateY(-50%)", whiteSpace: "nowrap", cursor: "default" },
            title: model.revision ? t("map.revision", model.revision) : void 0,
            children: countWords
          }
        )
      ] }),
      onPicked && matches.length > 0 && // Native's `a`: every sound the search keeps, or every sound Kumi found (a "More like these" among them).
      /* @__PURE__ */ jsx12(TextButton, { onClick: () => onPicked(matches, "replace"), title: searched !== null ? t("dock.pickAllTitle") : t("dock.pickFoundTitle"), children: t("dock.pickAll", matches.length) }),
      onPicked && /* @__PURE__ */ jsx12(TextButton, { onClick: () => setHandTool(!handTool), pressed: handTool, title: t("map.handTitle"), children: t("map.hand") }),
      onTiles && /* @__PURE__ */ jsx12(TextButton, { onClick: () => onTiles(!tiles), pressed: tiles, title: t("map.tilesTitle"), children: t("map.tiles") }),
      tools
    ] }),
    /* @__PURE__ */ jsx12("div", { style: { ...S.row, flexWrap: "wrap", gap: 4, padding: "6px 10px" }, children: model.groups.map((group) => {
      const off = hidden.has(group.key);
      return /* @__PURE__ */ jsxs12(
        "button",
        {
          "aria-pressed": !off,
          className: FOCUSABLE,
          onClick: () => setHidden((current) => {
            const next = new Set(current);
            if (next.has(group.key)) next.delete(group.key);
            else next.add(group.key);
            return next;
          }),
          title: off ? t("map.show", group.name) : t("map.hide", group.name),
          type: "button",
          style: { ...S.row, gap: 4, font: "inherit", fontSize: 11, padding: "1px 6px", borderRadius: 10, border: `1px solid ${T.border}`, background: "transparent", color: off ? T.faint : T.secondary, cursor: "pointer", opacity: off ? 0.6 : 1 },
          children: [
            /* @__PURE__ */ jsx12("span", { style: { width: 8, height: 8, borderRadius: "50%", background: group.color, opacity: off ? 0.3 : 1 } }),
            group.name,
            " ",
            /* @__PURE__ */ jsx12("span", { style: { color: T.faint }, children: group.count })
          ]
        },
        group.key
      );
    }) }),
    matches.length > 0 && /* @__PURE__ */ jsxs12("div", { "aria-label": searched ? t("map.matches") : nearName || t("map.found"), role: "group", style: { display: "flex", flexDirection: "column", gap: 1, padding: "0 10px 6px" }, children: [
      nearName && /* @__PURE__ */ jsx12("span", { style: { ...S.muted, padding: "0 6px" }, children: nearName }),
      matches.slice(0, MATCHES_LISTED).map((id) => {
        const sound = model.sounds[id];
        const indexed = indexedValues(sound, t);
        const said2 = searched ? "" : foundLine(found?.matches?.[id], nearName !== "", t);
        return /* @__PURE__ */ jsxs12(
          "button",
          {
            "aria-pressed": selected === id,
            className: FOCUSABLE,
            onClick: () => onPick(id),
            type: "button",
            style: { display: "flex", flexDirection: "column", alignItems: "stretch", gap: 0, font: "inherit", fontSize: 11.5, padding: "1px 6px", borderRadius: 3, border: "none", textAlign: "left", cursor: "pointer", color: T.text, background: selected === id ? T.card : "transparent" },
            children: [
              /* @__PURE__ */ jsxs12("span", { style: { ...S.row, gap: 6 }, children: [
                /* @__PURE__ */ jsx12("span", { style: { ...S.ellipsis, flex: 1 }, children: clean(sound?.name ?? id, 80) }),
                indexed && /* @__PURE__ */ jsx12("span", { style: { color: T.faint, whiteSpace: "nowrap" }, title: t("map.indexedTitle"), children: t("map.indexed", indexed) }),
                sound?.class && /* @__PURE__ */ jsx12("span", { style: { color: T.faint }, children: clean(sound.class, 16) })
              ] }),
              said2 && /* @__PURE__ */ jsx12("span", { style: { ...S.ellipsis, color: T.faint, fontSize: 10.5 }, title: nearName ? t("map.closenessTitle") : void 0, children: said2 })
            ]
          },
          id
        );
      }),
      matches.length > MATCHES_LISTED && /* @__PURE__ */ jsx12("span", { style: { ...S.faint, padding: "0 6px" }, children: t("map.more", matches.length - MATCHES_LISTED) })
    ] }),
    /* @__PURE__ */ jsxs12("div", { ref: boxRef, style: { position: "relative", flex: 1, minHeight: 160, overflow: "hidden" }, children: [
      /* @__PURE__ */ jsx12(
        "canvas",
        {
          ref: canvasRef,
          "aria-describedby": hintId,
          "aria-label": t("map.canvas"),
          "aria-roledescription": t("map.roledescription"),
          className: FOCUSABLE,
          onKeyDown: onKey,
          onPointerDown: press,
          onPointerMove: (event) => {
            const [x, y] = at(event);
            const held = drag.current;
            if (held) {
              if (Math.abs(x - held.x) + Math.abs(y - held.y) > 3) held.moved = true;
              if (held.box) setBox([held.x, held.y, x, y]);
              else if (held.moved) queueView({ ...held.view, dx: held.view.dx + x - held.x, dy: held.view.dy + y - held.y });
              return;
            }
            if (!view) return;
            const index = pick(active, view, mapSize, x, y, reachPx(8), visible);
            setHover(index >= 0 ? { index, x, y } : null);
            rest(index >= 0 ? model.points[index].id : null);
          },
          onPointerUp: release,
          onPointerCancel: () => {
            drag.current = null;
            setBox(null);
            setGrabbing(false);
          },
          onPointerEnter: () => {
            over.current = true;
          },
          onPointerLeave: () => {
            over.current = false;
            setHover(null);
            rest(null);
          },
          onMouseDown: (event) => {
            if (event.button === 1) event.preventDefault();
          },
          onDoubleClick: () => setView(fitView(size.width, size.height)),
          role: "application",
          tabIndex: 0,
          style: {
            position: "absolute",
            inset: 0,
            width: "100%",
            height: "100%",
            outlineOffset: -3,
            cursor: grabbing ? "grabbing" : spaceHeld || handTool || !onPicked ? "grab" : box ? "crosshair" : hover ? "pointer" : "default",
            touchAction: "none"
          }
        }
      ),
      box && Math.abs(box[2] - box[0]) + Math.abs(box[3] - box[1]) > 3 && // The box being drawn, its edge marching (`kumi-ants`, in FocusStyle).
      /* @__PURE__ */ jsx12(
        "svg",
        {
          "aria-hidden": true,
          height: Math.abs(box[3] - box[1]),
          style: { position: "absolute", left: Math.min(box[0], box[2]), top: Math.min(box[1], box[3]), overflow: "visible", pointerEvents: "none" },
          width: Math.abs(box[2] - box[0]),
          children: /* @__PURE__ */ jsx12(
            "rect",
            {
              className: "kumi-ants",
              height: Math.max(0, Math.abs(box[3] - box[1]) - 1),
              rx: 2,
              style: { fill: `color-mix(in srgb, ${T.accent} 12%, transparent)`, stroke: T.accent, strokeWidth: 1, strokeDasharray: "5 4" },
              width: Math.max(0, Math.abs(box[2] - box[0]) - 1),
              x: 0.5,
              y: 0.5
            }
          )
        }
      ),
      /* @__PURE__ */ jsx12("span", { id: hintId, style: S.hidden, children: t("map.keys") }),
      /* @__PURE__ */ jsx12("span", { "aria-live": "polite", style: S.hidden, children: said }),
      hovered && hoveredSound && hover && /* @__PURE__ */ jsxs12(
        "div",
        {
          style: { position: "absolute", left: Math.min(hover.x + 12, Math.max(0, size.width - 220)), top: Math.max(0, hover.y - 34), maxWidth: 220, pointerEvents: "none", padding: "3px 7px", borderRadius: 4, background: T.raised, border: `1px solid ${T.border}`, boxShadow: "0 6px 16px -8px rgba(0,0,0,.6)", fontSize: 11.5, color: T.text, ...S.ellipsis },
          children: [
            clean(hoveredSound.name, 48),
            " ",
            /* @__PURE__ */ jsxs12("span", { style: { color: T.muted }, children: [
              "· ",
              clean(hoveredSound.class ?? "", 16)
            ] })
          ]
        }
      )
    ] }),
    pickedIds.length > 0 && /* @__PURE__ */ jsxs12("div", { role: "group", "aria-label": t("dock.picked", pickedIds.length), style: { ...S.row, flexWrap: "wrap", gap: 6, padding: "6px 10px", borderTop: `1px solid ${T.border}` }, children: [
      /* @__PURE__ */ jsx12("span", { style: { ...S.text, fontWeight: 600, flex: "1 1 8rem" }, children: t("dock.picked", pickedIds.length) }),
      pickedActions?.(pickedIds)
    ] }),
    (map?.hidden ?? 0) + leavingCount > 0 && // One quiet line for every sound that couldn't play here, never a notice per sound.
    /* @__PURE__ */ jsxs12("div", { role: "status", style: { ...S.row, flexWrap: "wrap", gap: 6, padding: "4px 10px", borderTop: `1px solid ${T.border}` }, children: [
      /* @__PURE__ */ jsx12("span", { style: { ...S.faint, flex: "1 1 14rem" }, children: t("map.hidden", (map?.hidden ?? 0) + leavingCount) }),
      hiddenActions
    ] }),
    selected && chosen && // Beside the chat the map can be narrow: the name and the actions wrap rather than push actions out of view.
    /* @__PURE__ */ jsxs12("div", { style: { ...S.row, justifyContent: "space-between", flexWrap: "wrap", gap: 8, padding: "8px 10px", borderTop: `1px solid ${T.border}` }, children: [
      /* @__PURE__ */ jsxs12("div", { style: { display: "flex", flexDirection: "column", minWidth: 0 }, children: [
        /* @__PURE__ */ jsx12("span", { style: { ...S.text, ...S.ellipsis, fontWeight: 600 }, children: clean(chosen.name, 80) }),
        /* @__PURE__ */ jsxs12("span", { style: { ...S.row, gap: 4, marginTop: 2 }, children: [
          chosen.class && /* @__PURE__ */ jsx12(Chip, { children: clean(chosen.class, 16) }),
          chosen.kind && /* @__PURE__ */ jsx12(Chip, { children: clean(chosen.kind, 16) }),
          typeof chosen.duration === "number" && /* @__PURE__ */ jsx12(Chip, { children: seconds(chosen.duration) }),
          chosen.folder && /* @__PURE__ */ jsx12("span", { style: { ...S.faint, ...S.ellipsis }, children: clean(chosen.folder, 60) })
        ] })
      ] }),
      /* @__PURE__ */ jsx12("div", { style: { ...S.row, gap: 6, flexWrap: "wrap", justifyContent: "flex-end", marginLeft: "auto" }, children: actions?.(selected, chosen) })
    ] })
  ] });
}

// src/shared/palette.ts
var MOST_GATHERED = 1e4;
var LOOSE = 5;
var NAME_MOST = 80;
function marksOf(palette, revision) {
  const out = /* @__PURE__ */ new Set();
  if (!palette?.current || !revision || palette.map_revision !== revision) return out;
  for (const item of palette.current.items) if (item.sound_id) out.add(item.sound_id);
  return out;
}
function playableId(item) {
  return item.sound_id ?? item.artifact_id;
}
function moved(items, key, to) {
  const from = items.findIndex((item) => item.key === key);
  if (from < 0) return [...items];
  const out = items.filter((item) => item.key !== key);
  out.splice(Math.max(0, Math.min(out.length, to)), 0, items[from]);
  return out;
}
function paletteName(text) {
  const name = text.replace(/[\p{Cc}\p{Cf}\u2028\u2029]+/gu, " ").replace(/\s+/g, " ").trim().slice(0, NAME_MOST).trim();
  return name || null;
}
function kitRequest(name) {
  const said = quotable(name);
  return said ? `Load the sounds in my palette “${said}” as a kit` : "Load the sounds in my palette as a kit";
}
function gatherIds(ids) {
  const seen = /* @__PURE__ */ new Set();
  for (const id of ids) if (plainId(id)) seen.add(id);
  const all = [...seen];
  return { ids: all.slice(0, MOST_GATHERED), dropped: Math.max(0, all.length - MOST_GATHERED) };
}
function peaksOf2(samples, bins) {
  const out = new Array(bins).fill(0);
  if (samples.length === 0 || bins <= 0) return out;
  let loudest = 0;
  for (let bin = 0; bin < bins; bin++) {
    const start = Math.floor(bin * samples.length / bins);
    const end = Math.max(start + 1, Math.floor((bin + 1) * samples.length / bins));
    let peak = 0;
    for (let at = start; at < end && at < samples.length; at++) {
      const value = Math.abs(samples[at]);
      if (value > peak) peak = value;
    }
    out[bin] = peak;
    if (peak > loudest) loudest = peak;
  }
  return loudest > 0 ? out.map((peak) => peak / loudest) : out;
}

// src/shared/PaletteDock.tsx
import { useEffect as useEffect7, useMemo as useMemo3, useRef as useRef4, useState as useState6 } from "react";
import { Fragment as Fragment5, jsx as jsx13, jsxs as jsxs13 } from "react/jsx-runtime";
var CHIPS_SHOWN = 240;
var BARS = 28;
var THUMB_MOST = 12 * 1024 * 1024;
var peaksKept = /* @__PURE__ */ new Map();
var peaksLoading = /* @__PURE__ */ new Map();
var decodeQueue = Promise.resolve();
function decodePeaks(id, audio) {
  const kept = peaksKept.get(id);
  if (kept !== void 0) return Promise.resolve(kept);
  const loading = peaksLoading.get(id);
  if (loading) return loading;
  const run = async () => {
    const url = await audio(id);
    const comma = url.indexOf(",");
    if (comma < 0 || url.length > THUMB_MOST || typeof OfflineAudioContext !== "function") return null;
    const binary = atob(url.slice(comma + 1));
    const bytes = new Uint8Array(binary.length);
    for (let at = 0; at < binary.length; at++) bytes[at] = binary.charCodeAt(at);
    const buffer = await new OfflineAudioContext(1, 1, 44100).decodeAudioData(bytes.buffer);
    const mixed = new Float32Array(buffer.length);
    for (let channel = 0; channel < buffer.numberOfChannels; channel++) {
      const data = buffer.getChannelData(channel);
      for (let at = 0; at < data.length; at++) mixed[at] += data[at];
    }
    return peaksOf2(mixed, BARS);
  };
  const promise = decodeQueue.then(run, run).catch(() => null).then((peaks) => {
    peaksKept.set(id, peaks);
    peaksLoading.delete(id);
    return peaks;
  });
  decodeQueue = promise;
  peaksLoading.set(id, promise);
  return promise;
}
function Waveform({ id, color, audio }) {
  const canvas = useRef4(null);
  const [peaks, setPeaks] = useState6(() => id ? peaksKept.get(id) ?? null : null);
  useEffect7(() => {
    const element = canvas.current;
    if (!id || !element || peaks) return;
    let live = true;
    const observer = new IntersectionObserver((entries) => {
      if (!entries.some((entry) => entry.isIntersecting)) return;
      observer.disconnect();
      void decodePeaks(id, audio).then((found) => live && setPeaks(found));
    });
    observer.observe(element);
    return () => {
      live = false;
      observer.disconnect();
    };
  }, [id, audio, peaks]);
  useEffect7(() => {
    const element = canvas.current;
    const context = element?.getContext("2d");
    if (!element || !context) return;
    const ratio = window.devicePixelRatio || 1;
    const [width, height] = [44, 16];
    element.width = width * ratio;
    element.height = height * ratio;
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, width, height);
    context.fillStyle = getComputedStyle(element).color;
    const bars = peaks ?? new Array(BARS).fill(0.08);
    const step2 = width / bars.length;
    bars.forEach((peak, bar) => {
      const tall = Math.max(1, peak * (height - 2));
      context.globalAlpha = peaks ? 0.95 : 0.35;
      context.fillRect(bar * step2, (height - tall) / 2, Math.max(1, step2 - 0.6), tall);
    });
  }, [peaks, color]);
  return /* @__PURE__ */ jsx13("canvas", { "aria-hidden": true, ref: canvas, style: { width: 44, height: 16, flex: "none", color } });
}
function Chip2({
  item,
  index,
  color,
  playing,
  audio,
  onPlay,
  onStop,
  onRemove,
  onMove,
  onDropHere,
  dragging,
  onDrag,
  arriving
}) {
  const t = useT();
  const id = playableId(item);
  const name = clean(item.name, 60) || item.key;
  const [over, setOver] = useState6(false);
  const keys = (event) => {
    if (event.altKey && (event.key === "ArrowLeft" || event.key === "ArrowRight")) {
      event.preventDefault();
      onMove(index + (event.key === "ArrowLeft" ? -1 : 1));
    } else if (event.key === "Delete") {
      event.preventDefault();
      onRemove();
    }
  };
  return /* @__PURE__ */ jsxs13(
    "div",
    {
      draggable: true,
      onDragEnd: () => onDrag(null),
      onDragLeave: () => setOver(false),
      onDragOver: (event) => {
        if (!dragging || dragging === item.key) return;
        event.preventDefault();
        setOver(true);
      },
      onDragStart: (event) => {
        event.dataTransfer.effectAllowed = "move";
        event.dataTransfer.setData("text/plain", name);
        onDrag(item.key);
      },
      onDrop: (event) => {
        event.preventDefault();
        setOver(false);
        if (dragging && dragging !== item.key) onDropHere();
      },
      className: arriving === null ? void 0 : "kumi-pop",
      role: "listitem",
      style: {
        display: "flex",
        alignItems: "center",
        gap: 2,
        maxWidth: 220,
        padding: "2px 2px 2px 4px",
        borderRadius: 6,
        border: `1px solid ${playing ? color : over ? T.accent : T.border}`,
        boxShadow: playing ? `0 0 0 1px ${color}, 0 0 10px -2px ${color}` : "none",
        background: playing ? T.chip : "transparent",
        opacity: dragging === item.key ? 0.4 : id ? 1 : 0.55,
        transition: "border-color 120ms, box-shadow 160ms, opacity 120ms",
        cursor: "grab",
        animationDelay: arriving === null ? void 0 : `${380 + Math.min(arriving, 24) * 22}ms`
      },
      children: [
        /* @__PURE__ */ jsxs13(
          "button",
          {
            "aria-label": playing ? t("dock.stop", name) : t("dock.play", name),
            "aria-pressed": playing,
            className: FOCUSABLE,
            disabled: !id,
            onClick: () => playing ? onStop() : onPlay(),
            onKeyDown: keys,
            title: id ? `${name}${item.class ? ` · ${clean(item.class, 16)}` : ""}
${t("dock.reorderHint")}` : t("dock.offMap"),
            type: "button",
            style: { ...S.row, gap: 5, minWidth: 0, font: "inherit", fontSize: 11.5, padding: "1px 3px", border: "none", background: "transparent", color: T.text, cursor: id ? "pointer" : "default" },
            children: [
              /* @__PURE__ */ jsx13(Waveform, { audio, color, id }),
              /* @__PURE__ */ jsx13("span", { style: S.ellipsis, children: name })
            ]
          }
        ),
        /* @__PURE__ */ jsx13(
          "button",
          {
            "aria-label": t("dock.remove", name),
            className: FOCUSABLE,
            onClick: onRemove,
            title: t("dock.remove", name),
            type: "button",
            style: { font: "inherit", fontSize: 12, lineHeight: "14px", width: 16, height: 16, padding: 0, borderRadius: 3, border: "none", background: "transparent", color: T.faint, cursor: "pointer", flex: "none" },
            children: "×"
          }
        )
      ]
    }
  );
}
function NameField({ initial, label, onDone }) {
  const [text, setText] = useState6(initial);
  const field = useRef4(null);
  const done = useRef4(false);
  useEffect7(() => {
    field.current?.focus();
    field.current?.select();
  }, []);
  const finish = (name) => {
    if (done.current) return;
    done.current = true;
    onDone(name);
  };
  return /* @__PURE__ */ jsx13(
    "input",
    {
      "aria-label": label,
      className: FOCUSABLE,
      maxLength: 120,
      onBlur: () => finish(paletteName(text)),
      onChange: (event) => setText(event.target.value),
      onKeyDown: (event) => {
        if (event.key === "Enter") {
          event.preventDefault();
          finish(paletteName(text));
        } else if (event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          finish(null);
        }
      },
      placeholder: label,
      ref: field,
      style: { font: "inherit", fontSize: 12, fontWeight: 600, width: "12rem", padding: "1px 6px", borderRadius: 3, border: `1px solid ${T.accent}`, background: "transparent", color: T.text },
      value: text
    }
  );
}
function PaletteDock({ palette, groups, playing, onPlay, onStop, onDo, said, audio, kit, make, landing }) {
  const t = useT();
  const current = palette?.current ?? null;
  const [editing, setEditing] = useState6(null);
  const [forgetting, setForgetting] = useState6(false);
  const [open, setOpen] = useState6(true);
  const [dragging, setDragging] = useState6(null);
  const [order, setOrder] = useState6(null);
  useEffect7(() => setOrder(null), [palette]);
  const known = useRef4(null);
  const arrived = useMemo3(() => {
    const keys = new Set((current?.items ?? []).map((item) => item.key));
    const before = known.current;
    const fresh = before && before.id === current?.id ? (current?.items ?? []).filter((item) => !before.keys.has(item.key)).map((item) => item.key) : [];
    known.current = { id: current?.id ?? null, keys };
    return new Map(fresh.map((key, order2) => [key, order2]));
  }, [current]);
  const strip = useRef4(null);
  useEffect7(() => {
    const element = strip.current;
    if (!element || arrived.size === 0) return;
    element.scrollTo({ top: element.scrollHeight, behavior: window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth" });
  }, [arrived]);
  const [now, setNow] = useState6(() => Date.now());
  const shownFor = said?.failed ? 8e3 : 4e3;
  useEffect7(() => {
    if (!said) return;
    setNow(Date.now());
    const timer = setTimeout(() => setNow(Date.now()), Math.max(0, said.at + shownFor - Date.now()));
    return () => clearTimeout(timer);
  }, [said, shownFor]);
  const saying = said && now - said.at < shownFor ? said : null;
  const run = (action) => onDo(action).then((done) => {
    if (!done) setOrder(null);
  });
  const items = order ?? current?.items ?? [];
  const colorOf2 = (group) => group === null ? T.faint : groupColor(groups?.[String(group)]?.color, group);
  const move = (key, to) => {
    if (to < 0 || to >= items.length) return;
    setOrder(moved(items, key, to));
    void run({ action: "move", key, to });
  };
  const header = /* @__PURE__ */ jsxs13("div", { style: { ...S.row, flexWrap: "wrap", gap: 6 }, children: [
    /* @__PURE__ */ jsxs13(
      "button",
      {
        "aria-expanded": open,
        className: FOCUSABLE,
        onClick: () => setOpen(!open),
        title: open ? t("dock.collapse") : t("dock.expand"),
        type: "button",
        style: { ...S.heading, font: "inherit", fontSize: 11, padding: 0, border: "none", background: "transparent", cursor: "pointer" },
        children: [
          /* @__PURE__ */ jsx13("span", { "aria-hidden": true, style: { display: "inline-block", transform: open ? "rotate(90deg)" : "none", transition: "transform 120ms" }, children: "▸" }),
          t("dock.title")
        ]
      }
    ),
    editing ? /* @__PURE__ */ jsx13(
      NameField,
      {
        initial: editing === "rename" ? current?.name ?? "" : "",
        label: editing === "rename" ? t("dock.renameTitle") : t("dock.newName"),
        onDone: (name) => {
          const action = editing;
          setEditing(null);
          if (name && action === "rename" && name !== current?.name) void run({ action: "rename", name });
          if (name && action === "new") void run({ action: "new", name });
        }
      }
    ) : /* @__PURE__ */ jsxs13(Fragment5, { children: [
      palette && palette.palettes.length > 0 && /* @__PURE__ */ jsxs13(
        "select",
        {
          "aria-label": t("dock.switch"),
          className: FOCUSABLE,
          onChange: (event) => {
            const value = event.target.value;
            if (value === "\0new") setEditing("new");
            else if (value !== current?.id) void run({ action: "use", id: value });
          },
          style: { font: "inherit", fontSize: 12, fontWeight: 600, maxWidth: "14rem", padding: "0 4px", borderRadius: 3, border: `1px solid ${T.border}`, background: "transparent", color: T.text },
          value: current?.id ?? "",
          children: [
            !current && /* @__PURE__ */ jsx13("option", { value: "", children: t("dock.choose") }),
            palette.palettes.map((entry) => /* @__PURE__ */ jsx13("option", { value: entry.id, children: `${clean(entry.name, 40)} (${num(entry.items, 0)})` }, entry.id)),
            /* @__PURE__ */ jsx13("option", { value: "\0new", children: t("dock.newPalette") })
          ]
        }
      ),
      (!palette || palette.palettes.length === 0) && /* @__PURE__ */ jsx13("span", { style: S.faint, children: t("dock.none") }),
      current && /* @__PURE__ */ jsx13(TextButton, { onClick: () => setEditing("rename"), title: t("dock.renameTitle"), children: t("dock.rename") }),
      palette && palette.palettes.length === 0 && /* @__PURE__ */ jsx13(TextButton, { onClick: () => setEditing("new"), title: t("dock.newName"), children: t("dock.newPalette") })
    ] }),
    current && /* @__PURE__ */ jsxs13("span", { role: "group", "aria-label": t("dock.mode"), style: { ...S.row, gap: 0 }, children: [
      /* @__PURE__ */ jsx13(TextButton, { onClick: () => current.reach !== 0 && void run({ action: "reach", reach: 0 }), pressed: current.reach === 0, title: t("dock.strictTitle"), children: t("dock.strict") }),
      /* @__PURE__ */ jsx13(TextButton, { onClick: () => current.reach === 0 && void run({ action: "reach", reach: LOOSE }), pressed: current.reach > 0, title: t("dock.looseTitle", current.reach || LOOSE), children: current.reach > 0 ? t("dock.looseN", current.reach) : t("dock.loose") })
    ] }),
    current && /* @__PURE__ */ jsxs13("span", { style: S.faint, children: [
      t("dock.count", current.items.length),
      current.excluded > 0 ? ` · ${t("dock.excluded", current.excluded)}` : ""
    ] }),
    /* @__PURE__ */ jsx13("span", { style: { flex: 1 } }),
    current && current.items.length > 1 && make && /* @__PURE__ */ jsx13(TextButton, { onClick: () => make(current), title: t("make.openTitle"), children: t("make.open") }),
    current && current.items.length > 0 && kit?.(current),
    current && !forgetting && /* @__PURE__ */ jsx13(TextButton, { onClick: () => setForgetting(true), title: t("dock.forgetTitle", clean(current.name, 40)), children: t("dock.forget") })
  ] });
  return /* @__PURE__ */ jsxs13("section", { "aria-label": t("dock.title"), ref: landing, style: { display: "flex", flexDirection: "column", gap: 6, padding: "8px 10px", borderTop: `1px solid ${T.border}`, color: T.text }, children: [
    header,
    current && forgetting && /* @__PURE__ */ jsxs13("div", { role: "alert", style: { ...S.row, flexWrap: "wrap", gap: 6 }, children: [
      /* @__PURE__ */ jsx13("span", { style: S.muted, children: t("dock.forgetAsk", clean(current.name, 40), current.items.length) }),
      /* @__PURE__ */ jsx13(
        TextButton,
        {
          onClick: () => {
            setForgetting(false);
            void run({ action: "forget", id: current.id });
          },
          strong: true,
          children: t("dock.forgetYes")
        }
      ),
      /* @__PURE__ */ jsx13(TextButton, { onClick: () => setForgetting(false), children: t("dock.keep") })
    ] }),
    open && current && items.length > 0 && /* @__PURE__ */ jsxs13("div", { "aria-label": t("dock.sounds"), ref: strip, role: "list", style: { display: "flex", flexWrap: "wrap", gap: 5, maxHeight: 116, overflowY: "auto", paddingBottom: 1 }, children: [
      items.slice(0, CHIPS_SHOWN).map((item, index) => /* @__PURE__ */ jsx13(
        Chip2,
        {
          audio,
          color: item.source === "artifact" ? T.accent : colorOf2(item.group),
          dragging,
          index,
          item,
          arriving: arrived.get(item.key) ?? null,
          onDrag: setDragging,
          onDropHere: () => dragging && move(dragging, index),
          onMove: (to) => move(item.key, to),
          onPlay: () => {
            const id = playableId(item);
            if (id) onPlay(id);
          },
          onRemove: () => {
            setOrder(items.filter((other) => other.key !== item.key));
            void run({ action: "remove", keys: [item.key] });
          },
          onStop,
          playing: playing !== null && playing === playableId(item)
        },
        item.key
      )),
      items.length > CHIPS_SHOWN && /* @__PURE__ */ jsx13("span", { style: { ...S.faint, alignSelf: "center" }, children: t("dock.more", items.length - CHIPS_SHOWN) })
    ] }),
    open && (!current || items.length === 0) && /* @__PURE__ */ jsx13("span", { style: S.faint, children: t("dock.empty") }),
    /* @__PURE__ */ jsx13("span", { "aria-live": "polite", style: saying ? { ...S.faint, color: saying.failed ? T.warn : T.muted } : S.hidden, children: saying ? saying.failed ? t("dock.failed", saying.text) : saying.text : "" })
  ] });
}

// src/shared/paletteMap.ts
import { useEffect as useEffect8, useMemo as useMemo4, useState as useState7 } from "react";
function usePaletteMap(store, state, audition, t, { hover, other, left }) {
  const revision = state.map?.revision ?? null;
  const [picks, setPicks] = useState7({ revision, ids: /* @__PURE__ */ new Set() });
  const picked = picks.revision === revision ? picks.ids : /* @__PURE__ */ new Set();
  useEffect8(() => setPicks((current) => current.revision === revision ? current : { revision, ids: /* @__PURE__ */ new Set() }), [revision]);
  const marked = useMemo4(() => marksOf(state.palette, revision), [state.palette, revision]);
  const [flying, setFlying] = useState7(null);
  const [making, setMaking] = useState7(null);
  const kept = useMemo4(() => new Set((state.palette?.current?.items ?? []).flatMap((item) => item.artifact_id ? [item.artifact_id] : [])), [state.palette]);
  const groupOf = useMemo4(() => new Map((state.map?.points ?? []).map(([id, , , group]) => [id, group])), [state.map]);
  useEffect8(() => setMaking((current) => current?.some((source) => source.sound_id && source.map_revision !== revision) ? null : current), [revision]);
  const open = (sources) => {
    setMaking(sources);
    if (state.designs === null) void store.readDesigns();
  };
  const play = (id, preview = false) => {
    audition.play(id, preview ? { preview: PREVIEW_SECONDS } : {}).catch((error) => {
      const why = playFailure(error);
      if (why === "gone" || why === "unplayable") {
        store.hide(id, why);
        left?.(id);
      } else if (why === "other") {
        other?.(error);
      }
    });
  };
  const onHover = useHoverPlay(audition, hover, (id) => play(id, true));
  return {
    picked,
    marked,
    play,
    onHover,
    flying,
    making,
    kept,
    makeFromPicks: (ids) => open(sourcesOf(ids.map((id) => ({ sound_id: id, name: state.map?.sounds[id]?.name ?? null })), revision)),
    makeFromPalette: (view) => open(sourcesOf(view.items, state.palette?.map_revision ?? null)),
    closeMake: () => setMaking(null),
    keep: (result) => store.paletteDo({ action: "gather", artifact_ids: [result.artifact_id] }),
    colorOf: (source) => {
      const group = source.sound_id ? groupOf.get(source.sound_id) : void 0;
      return group === void 0 ? T.accent : groupColor(state.map?.groups[String(group)]?.color, group);
    },
    onPicked: (ids, how) => setPicks((current) => {
      const base = current.revision === revision ? current.ids : /* @__PURE__ */ new Set();
      const next = how === "replace" ? /* @__PURE__ */ new Set() : new Set(base);
      for (const id of ids) {
        if (how === "toggle" && next.has(id)) next.delete(id);
        else next.add(id);
      }
      return { revision, ids: next };
    }),
    clearPicks: () => setPicks({ revision, ids: /* @__PURE__ */ new Set() }),
    gather: async (ids) => {
      const { ids: sent, dropped } = gatherIds(ids);
      if (sent.length === 0) return;
      setFlying({ ids: sent.filter((id) => !marked.has(id)), at: performance.now() });
      const done = await store.paletteDo({ action: "gather", sound_ids: sent }, dropped > 0 ? t("dock.firstOnly", sent.length + dropped) : void 0);
      if (done) setPicks({ revision, ids: /* @__PURE__ */ new Set() });
    }
  };
}

// src/desktop/map.tsx
import { Fragment as Fragment6, jsx as jsx14, jsxs as jsxs14 } from "react/jsx-runtime";
var HOVER_PLAY = "mapHover";
var TILES = "mapTiles";
var PAD_ROWS = [48, 44, 40, 36].map((start) => [start, start + 1, start + 2, start + 3]);
function PadPicker({ daw, onPick, onClose }) {
  const t = useT();
  const first = useRef5(null);
  useEffect9(() => first.current?.focus(), []);
  return /* @__PURE__ */ jsx14(
    "div",
    {
      "aria-label": t("map.pads"),
      onKeyDown: (event) => {
        if (event.key !== "Escape") return;
        event.stopPropagation();
        onClose();
      },
      role: "group",
      style: { flexBasis: "100%", display: "grid", gridTemplateColumns: "repeat(4, 2.8rem)", justifyContent: "end", gap: 4, paddingTop: 6 },
      children: PAD_ROWS.flat().map((note) => {
        const name = daw === "ableton" ? liveNote(note) : String(note);
        return /* @__PURE__ */ jsx14(
          "button",
          {
            "aria-label": t("map.pad", name, note),
            className: FOCUSABLE,
            onClick: () => onPick(note),
            ref: note === 36 ? first : void 0,
            style: { font: "inherit", fontSize: 11, height: 28, borderRadius: 4, border: `1px solid ${T.border}`, background: "transparent", color: T.text, cursor: "pointer" },
            title: t("map.pad", name, note),
            type: "button",
            children: name
          },
          note
        );
      })
    }
  );
}
function MapWorkspace({ ctx, store, audition }) {
  const t = useT();
  useWant(store, "map");
  useWant(store, "palette");
  const state = useKumi(store);
  const playing = usePlaying(audition);
  const [selected, setSelected] = useState8(null);
  const [padsFor, setPadsFor] = useState8(null);
  const [hover, setHover] = useState8(() => ctx.storage.get(HOVER_PLAY, true) !== false);
  const [tiles, setTiles] = useState8(() => ctx.storage.get(TILES, false) === true);
  const palette = usePaletteMap(store, state, audition, t, {
    hover,
    other: (error) => host4.notify({ kind: "warning", message: t("map.playFailed", errorText(error)) }),
    left: (id) => setSelected((current) => current === id ? null : current)
  });
  const dock = useRef5(null);
  const session = ownerSession(state);
  const map = state.map;
  const empty = state.available === false ? t("map.agentOff") : map === null ? t("map.reading") : t("map.empty");
  return /* @__PURE__ */ jsxs14("div", { style: { display: "flex", flexDirection: "column", height: "100%", minHeight: 0 }, children: [
    /* @__PURE__ */ jsxs14("div", { style: { flex: 1, minHeight: 0, position: "relative" }, children: [
      /* @__PURE__ */ jsx14(
        MapCanvas,
        {
          actions: (id, sound) => {
            const request = soundRequest(id, sound.name, map?.revision);
            const near = nearRequest([{ id, name: sound.name }], map?.revision);
            return /* @__PURE__ */ jsxs14(Fragment6, { children: [
              /* @__PURE__ */ jsx14(TextButton, { onClick: () => playing === id ? audition.stop() : palette.play(id), title: t("map.playTitle"), children: playing === id ? t("map.stop") : t("map.play") }),
              /* @__PURE__ */ jsx14(TextButton, { disabled: palette.marked.has(id), onClick: () => void palette.gather([id]), title: t("dock.addTitle"), children: palette.marked.has(id) ? t("dock.inPalette") : t("dock.add") }),
              near && /* @__PURE__ */ jsx14(TextButton, { onClick: () => void ask(ctx, near, { send: false, session }), title: t("map.moreLikeTitle"), children: t("map.moreLike") }),
              request && /* @__PURE__ */ jsx14(TextButton, { onClick: () => setPadsFor(padsFor === id ? null : id), pressed: padsFor === id, title: t("map.useOnPadTitle"), children: t("map.useOnPad") }),
              request && /* @__PURE__ */ jsx14(TextButton, { onClick: () => void ask(ctx, request, { send: false, session }), strong: true, title: t("map.useTitle"), children: t("map.use") }),
              request && padsFor === id && /* @__PURE__ */ jsx14(
                PadPicker,
                {
                  daw: state.status?.daw,
                  onClose: () => setPadsFor(null),
                  onPick: (note) => {
                    setPadsFor(null);
                    const onPad = padRequest(id, note, state.status?.daw, sound.name, map?.revision);
                    if (onPad) void ask(ctx, onPad, { send: false, session });
                  }
                }
              )
            ] });
          },
          empty,
          found: state.found,
          hiddenActions: /* @__PURE__ */ jsxs14(Fragment6, { children: [
            /* @__PURE__ */ jsx14(TextButton, { onClick: () => void store.showHidden(), title: t("map.showHiddenTitle"), children: t("map.showHidden") }),
            /* @__PURE__ */ jsx14(TextButton, { onClick: () => void ask(ctx, RESCAN_REQUEST, { send: false, session }), title: t("map.rescanTitle"), children: t("map.rescan") })
          ] }),
          leaving: state.leaving,
          map,
          marked: palette.marked,
          onHover: palette.onHover,
          onPick: (id) => {
            setSelected(id);
            palette.play(id);
          },
          onPicked: palette.onPicked,
          onSelect: (id) => {
            setSelected(id);
            if (id !== padsFor) setPadsFor(null);
          },
          picked: palette.picked,
          pickedActions: (ids) => {
            const near = ids.length > 1 ? nearRequest(ids.map((id) => ({ id, name: map?.sounds[id]?.name })), map?.revision) : null;
            return /* @__PURE__ */ jsxs14(Fragment6, { children: [
              /* @__PURE__ */ jsx14(TextButton, { onClick: () => void palette.gather(ids), strong: true, title: t("dock.addTitle"), children: t("dock.addPicked", ids.length) }),
              ids.length > 1 && /* @__PURE__ */ jsx14(TextButton, { onClick: () => palette.makeFromPicks(ids), title: t("make.openTitle"), children: t("make.open") }),
              near && /* @__PURE__ */ jsx14(
                TextButton,
                {
                  onClick: () => void ask(ctx, near, { send: false, session }),
                  title: ids.length > MOST_NEAR ? t("map.moreLikeTheseFirst", MOST_NEAR, ids.length) : t("map.moreLikeTheseTitle"),
                  children: t("map.moreLikeThese", Math.min(ids.length, MOST_NEAR))
                }
              ),
              /* @__PURE__ */ jsx14(TextButton, { onClick: palette.clearPicks, title: t("dock.clearPicksTitle"), children: t("dock.clearPicks", ids.length) })
            ] });
          },
          playing,
          selected,
          flying: palette.flying,
          onTiles: (on) => {
            ctx.storage.set(TILES, on);
            setTiles(on);
          },
          tiles,
          tools: /* @__PURE__ */ jsx14(
            TextButton,
            {
              onClick: () => {
                ctx.storage.set(HOVER_PLAY, !hover);
                setHover(!hover);
              },
              pressed: hover,
              title: t("dock.hoverTitle"),
              children: t("dock.hover")
            }
          )
        }
      ),
      palette.making && /* @__PURE__ */ jsx14(
        MakePanel,
        {
          colorOf: palette.colorOf,
          dock,
          kept: palette.kept,
          onClose: palette.closeMake,
          onKeep: palette.keep,
          onMake: (request) => store.design(request),
          onPlay: (id) => void audition.play(id).catch((error) => host4.notify({ kind: "warning", message: t("map.playFailed", errorText(error)) })),
          onStop: () => audition.stop(),
          playing,
          results: state.designs,
          sources: palette.making,
          tempo: state.focus?.transport?.tempo,
          use: (result) => {
            const request = useRequest(result);
            return request ? /* @__PURE__ */ jsx14(TextButton, { onClick: () => void ask(ctx, request, { send: false, session }), strong: true, title: t("make.useTitle"), children: t("make.use") }) : null;
          }
        }
      )
    ] }),
    /* @__PURE__ */ jsx14(
      PaletteDock,
      {
        audio: (id) => audition.url(id),
        groups: map?.groups,
        kit: (current) => /* @__PURE__ */ jsx14(TextButton, { onClick: () => void ask(ctx, kitRequest(current.name), { send: false, session }), title: t("dock.kitTitle"), children: t("dock.kit") }),
        landing: dock,
        make: palette.makeFromPalette,
        onDo: (action) => store.paletteDo(action),
        onPlay: (id) => palette.play(id),
        onStop: () => audition.stop(),
        palette: state.palette,
        playing,
        said: state.paletteSaid
      }
    )
  ] });
}

// src/desktop/measurements.tsx
import { Fragment as Fragment7, jsx as jsx15, jsxs as jsxs15 } from "react/jsx-runtime";
function MeasurementsWorkspace({ store, audition }) {
  const t = useT();
  const { measured } = useKumi(store);
  const note = (text) => /* @__PURE__ */ jsx15("span", { style: S.muted, children: text });
  return /* @__PURE__ */ jsx15("div", { style: { display: "flex", flexDirection: "column", gap: 10, padding: 16, color: T.text, overflowY: "auto", height: "100%" }, children: measured === null ? note(t("measurements.nothing")) : measured.result === null ? note(t("measurements.tooMuch")) : /* @__PURE__ */ jsxs15(Fragment7, { children: [
    /* @__PURE__ */ jsx15("span", { style: S.faint, children: t("measurements.measured", ago(measured.at, Date.now(), t)) }),
    /* @__PURE__ */ jsx15(ListenResultCard, { audition, fallback: note(t("measurements.inReply")), result: measured.result, store })
  ] }) });
}

// src/desktop/pet.ts
var PET_LINES = "petLines";
function petLine(before, after, t) {
  const was = before.history?.items;
  const items = after.history?.items ?? [];
  if (was && items.length) {
    const title = (item) => clean(item.title ?? item.label ?? item.id, 110);
    const top = items[0];
    if (!was.some((item) => item.id === top.id) && (top.state === "applied" || top.state === void 0)) return { id: "change", text: title(top), tone: "info" };
    const undone = items.find((item) => item.state === "undone" && was.some((old) => old.id === item.id && old.state !== "undone"));
    if (undone) return { id: "change", text: t("pet.undone", title(undone)), tone: "info" };
  }
  for (const [id, job] of Object.entries(after.jobs)) {
    if (job.ended_at === null || before.jobs[id]?.ended_at !== null) continue;
    const words = job.kind === "generate" ? generationJobWords(job, t) : jobWords(job, t);
    return { id: "job", text: [words.title, words.detail].filter(Boolean).join(" · "), tone: words.tone === "bad" ? "error" : "info" };
  }
  return null;
}

// src/desktop/settings.tsx
import { host as host5 } from "@hermes/plugin-sdk";
import { useState as useState9 } from "react";
import { Fragment as Fragment8, jsx as jsx16, jsxs as jsxs16 } from "react/jsx-runtime";
var OPEN_MAP_ON_FIND = "openMapOnFind";
var STATES2 = /* @__PURE__ */ new Set(["ready", "needs_runtime", "outdated_runtime", "damaged_runtime", "needs_daw_component", "no_pin", "unsupported_platform", "unknown"]);
var COMPONENT_STATES = /* @__PURE__ */ new Set(["not_installed", "installed", "outdated", "unknown"]);
var HEADING = { ...S.heading, display: "block", margin: "0 0 4px", fontWeight: "inherit" };
function Line({ label, children }) {
  return /* @__PURE__ */ jsxs16("div", { style: { display: "grid", gridTemplateColumns: "9rem 1fr", gap: 8, alignItems: "baseline", padding: "5px 0", borderTop: `1px solid ${T.border}` }, children: [
    /* @__PURE__ */ jsx16("span", { style: S.muted, children: label }),
    /* @__PURE__ */ jsx16("span", { style: { ...S.text, minWidth: 0 }, children })
  ] });
}
function megabytes(bytes) {
  return typeof bytes === "number" && bytes > 0 ? `${(bytes / 1e6).toFixed(1)} MB` : null;
}
var PLATFORMS = /* @__PURE__ */ new Set([
  "aarch64-apple-darwin",
  "x86_64-apple-darwin",
  "aarch64-pc-windows-msvc",
  "x86_64-pc-windows-msvc",
  "aarch64-unknown-linux-gnu",
  "x86_64-unknown-linux-gnu"
]);
function SettingsPage({ ctx, store, onOpenMap }) {
  const t = useT();
  useWant(store, "setup");
  const state = useKumi(store);
  const [openOnFind, setOpenOnFind] = useState9(() => ctx.storage.get(OPEN_MAP_ON_FIND, true));
  const [petLines, setPetLines] = useState9(() => ctx.storage.get(PET_LINES, false));
  const setup = state.setup;
  const steps = nextSteps(state, t);
  const runtime = setup?.runtime;
  const component = setup?.daw_component;
  const partWorks = dawPartWorks(state);
  const setupState = partWorks ? "ready" : setup?.state ?? "unknown";
  const componentState = component?.state ?? "unknown";
  const named = dawChosen(state) || partWorks || componentState === "installed" || Boolean(state.status?.connected);
  const daw = named ? state.status?.daw ?? setup?.daw : null;
  const next = setup?.message && !partWorks && (named || setupState !== "needs_daw_component") ? setup.message : null;
  return /* @__PURE__ */ jsxs16("div", { style: { display: "flex", flexDirection: "column", gap: 18, maxWidth: 640, color: T.text }, children: [
    /* @__PURE__ */ jsxs16("div", { style: { display: "flex", flexDirection: "column", gap: 4 }, children: [
      /* @__PURE__ */ jsx16("span", { style: { fontSize: 15, fontWeight: 600 }, children: "Kumi" }),
      /* @__PURE__ */ jsx16("span", { style: S.muted, children: t("settings.intro", knownDaw(daw) ?? "") })
    ] }),
    state.available === false ? /* @__PURE__ */ jsx16("span", { style: S.text, children: t("settings.agentOff") }) : /* @__PURE__ */ jsxs16(Fragment8, { children: [
      /* @__PURE__ */ jsxs16("div", { children: [
        /* @__PURE__ */ jsx16("h3", { style: HEADING, children: t("settings.setup") }),
        /* @__PURE__ */ jsx16(Line, { label: t("settings.state"), children: STATES2.has(setupState) ? t(`settings.states.${setupState}`) : clean(setupState, 40) }),
        next && /* @__PURE__ */ jsx16(Line, { label: t("settings.next"), children: clean(next, 300) }),
        runtime && /* @__PURE__ */ jsxs16(Line, { label: t("settings.runtime"), children: [
          runtime.installed ? t("settings.installed", clean(runtime.version ?? "", 20)) : t("settings.notInstalled"),
          setup?.pinned_version && setup.pinned_version !== runtime.version && /* @__PURE__ */ jsxs16("span", { style: S.faint, children: [
            " · ",
            t("settings.needs", clean(setup.pinned_version, 20))
          ] }),
          setup?.target && /* @__PURE__ */ jsxs16("span", { style: S.faint, children: [
            " · ",
            PLATFORMS.has(setup.target) ? t(`settings.platforms.${setup.target}`) : clean(setup.target, 40)
          ] })
        ] }),
        runtime && !runtime.installed && megabytes(setup?.download_bytes) && /* @__PURE__ */ jsxs16(Line, { label: t("settings.download"), children: [
          megabytes(setup?.download_bytes),
          megabytes(setup?.unpacked_bytes) && /* @__PURE__ */ jsxs16("span", { style: S.faint, children: [
            " ",
            t("settings.unpacked", megabytes(setup?.unpacked_bytes))
          ] })
        ] }),
        component && /* @__PURE__ */ jsxs16(Line, { label: t("settings.part", dawName(daw, t)), children: [
          (() => {
            const word = partWorks ? t("settings.partInUse") : COMPONENT_STATES.has(componentState) ? t(`settings.componentStates.${componentState}`) : clean(componentState, 20).replace("_", " ");
            return component.name ? t("settings.componentLine", clean(component.name, 40), word) : word;
          })(),
          component.path && /* @__PURE__ */ jsx16("div", { style: { ...S.faint, marginTop: 2 }, children: clean(component.path, 200) })
        ] }),
        setup?.existing_kumi?.path && /* @__PURE__ */ jsxs16(Line, { label: t("settings.standalone"), children: [
          clean(setup.existing_kumi.path, 120),
          /* @__PURE__ */ jsxs16("span", { style: S.faint, children: [
            " · ",
            setup.existing_kumi.adopted ? t("settings.adopted") : t("settings.leftAsIs")
          ] })
        ] }),
        setup?.compressed_audio?.message && /* @__PURE__ */ jsx16(Line, { label: t("settings.audioFormats"), children: /* @__PURE__ */ jsx16("span", { style: setup.compressed_audio.state === "available" ? void 0 : S.muted, children: clean(setup.compressed_audio.message, 400) }) })
      ] }),
      /* @__PURE__ */ jsxs16("div", { children: [
        /* @__PURE__ */ jsx16("h3", { style: HEADING, children: t("settings.connection") }),
        /* @__PURE__ */ jsx16(Line, { label: t("settings.now"), children: (() => {
          const words = chipWords(state.status, state.focus?.transport?.tempo, t);
          return /* @__PURE__ */ jsx16(Chip, { tone: words.tone === "muted" ? void 0 : words.tone, children: words.text });
        })() }),
        state.status?.owner?.session_id && /* @__PURE__ */ jsx16(Line, { label: t("settings.chat"), children: host5.state.focusedStoredSessionId?.get() === state.status.owner.session_id ? t("settings.chatHere") : t("settings.chatElsewhere") })
      ] }),
      /* @__PURE__ */ jsxs16("div", { style: { ...S.row, gap: 8, flexWrap: "wrap" }, children: [
        steps.map((step2) => /* @__PURE__ */ jsx16(
          TextButton,
          {
            onClick: () => void ask(ctx, step2.request, { send: true, session: state.status?.connected ? ownerSession(state) : null }),
            strong: steps.length === 1,
            children: step2.label
          },
          step2.request
        )),
        /* @__PURE__ */ jsx16(TextButton, { onClick: onOpenMap, children: t("settings.openMap") }),
        /* @__PURE__ */ jsx16(TextButton, { onClick: () => host5.navigate("/settings?tab=plugins&agent=kumi"), children: t("settings.agentSettings") })
      ] })
    ] }),
    /* @__PURE__ */ jsxs16("div", { children: [
      /* @__PURE__ */ jsx16("h3", { style: HEADING, children: t("settings.thisWindow") }),
      /* @__PURE__ */ jsxs16("label", { style: { ...S.row, gap: 8, padding: "5px 0", borderTop: `1px solid ${T.border}`, cursor: "pointer" }, children: [
        /* @__PURE__ */ jsx16(
          "input",
          {
            checked: openOnFind,
            className: FOCUSABLE,
            onChange: (event) => {
              setOpenOnFind(event.target.checked);
              ctx.storage.set(OPEN_MAP_ON_FIND, event.target.checked);
            },
            type: "checkbox"
          }
        ),
        /* @__PURE__ */ jsx16("span", { style: S.text, children: t("settings.openOnFind") })
      ] }),
      ctx.pet && /* @__PURE__ */ jsxs16("label", { style: { ...S.row, gap: 8, padding: "5px 0", borderTop: `1px solid ${T.border}`, cursor: "pointer" }, children: [
        /* @__PURE__ */ jsx16(
          "input",
          {
            checked: petLines,
            className: FOCUSABLE,
            onChange: (event) => {
              setPetLines(event.target.checked);
              ctx.storage.set(PET_LINES, event.target.checked);
            },
            type: "checkbox"
          }
        ),
        /* @__PURE__ */ jsx16("span", { style: S.text, children: t("settings.petLines") })
      ] })
    ] })
  ] });
}

// src/desktop/plugin.tsx
import { jsx as jsx17, jsxs as jsxs17 } from "react/jsx-runtime";
var PLUGIN_ID = "kumi";
var MAP_WORKSPACE = "kumi-sound-map";
var MEASUREMENTS_WORKSPACE = "kumi-measurements";
var LISTEN_FALLBACK_MS = 2e3;
var POLL_MS = 5e3;
var JOB_POLL_MS = 2e3;
var usePluginWords = typeof sdk.usePluginI18n === "function" ? sdk.usePluginI18n : null;
function WithT({ children }) {
  const t = usePluginWords ? usePluginWords(PLUGIN_ID) : english;
  return /* @__PURE__ */ jsxs17(I18n.Provider, { value: t, children: [
    /* @__PURE__ */ jsx17(FocusStyle, {}),
    children
  ] });
}
var besideTheChat = (open) => open ? { pane: `plugin-workspace:${open}`, pos: "center" } : { pane: "workspace", pos: "right" };
var plugin = {
  id: PLUGIN_ID,
  name: "Kumi",
  description: "Your DAW beside the chat: FOCUS, HISTORY with undo, a sound map of your sample folders, and Kumi's audio measurements.",
  register(ctx) {
    ctx.i18n?.register(MESSAGES);
    const store = new KumiStore((path, init) => ctx.rest(path, { timeoutMs: 15e3, ...init }));
    const audition = new Audition((id) => ctx.rest(`/audio?id=${encodeURIComponent(id)}`, { timeoutMs: 6e4 }));
    let closeMap = null;
    let mapPane = null;
    let closeMeasurements = null;
    let measurementsPane = null;
    const mapView = () => /* @__PURE__ */ jsx17(WithT, { children: /* @__PURE__ */ jsx17(MapWorkspace, { audition, ctx, store }) });
    const openMap = () => {
      if (typeof host6.openWorkspace === "function") {
        closeMap = host6.openWorkspace(MAP_WORKSPACE, {
          title: wordsOf(ctx)("map.tab"),
          minWidth: "26rem",
          dock: besideTheChat(closeMeasurements ? MEASUREMENTS_WORKSPACE : null),
          render: mapView,
          onClose: () => {
            closeMap = null;
          }
        });
        return;
      }
      mapPane ??= ctx.register({
        id: "map-pane",
        area: PANES_AREA,
        title: wordsOf(ctx)("map.tab"),
        data: { placement: "main" },
        render: mapView
      });
    };
    const openMeasurements = () => {
      const render = () => /* @__PURE__ */ jsx17(WithT, { children: /* @__PURE__ */ jsx17(MeasurementsWorkspace, { audition, store }) });
      const title = wordsOf(ctx)("measurements.tab");
      if (typeof host6.openWorkspace === "function") {
        closeMeasurements = host6.openWorkspace(MEASUREMENTS_WORKSPACE, {
          title,
          minWidth: "24rem",
          dock: besideTheChat(closeMap ? MAP_WORKSPACE : null),
          render,
          onClose: () => {
            closeMeasurements = null;
          }
        });
        return;
      }
      measurementsPane ??= ctx.register({ id: "measurements-pane", area: PANES_AREA, title, data: { placement: "main" }, render });
    };
    const openSettings = () => host6.navigate(typeof sdk.pluginSettingsHref === "function" ? sdk.pluginSettingsHref(PLUGIN_ID) : "/settings?tab=plugins");
    const refresh = () => void store.refreshStatus();
    refresh();
    ctx.setInterval(() => {
      if (document.visibilityState !== "hidden") refresh();
    }, POLL_MS);
    const ours = (act) => (event) => {
      const payload = payloadOf(event);
      if (!forAnotherProfile(payload)) act(payload);
    };
    ctx.onEvent(`plugin.${PLUGIN_ID}.status.changed`, ours(refresh));
    ctx.onEvent(`plugin.${PLUGIN_ID}.focus.changed`, ours(() => void store.refresh("focus")));
    ctx.onEvent(`plugin.${PLUGIN_ID}.history.changed`, ours(() => void store.refresh("history")));
    ctx.onEvent(`plugin.${PLUGIN_ID}.map.changed`, ours(() => void store.refresh("map")));
    ctx.onEvent(`plugin.${PLUGIN_ID}.palette.changed`, ours(() => void store.refresh("palette")));
    ctx.setInterval(() => {
      if (document.visibilityState !== "hidden") void store.refreshJobs();
    }, JOB_POLL_MS);
    ctx.onEvent(`plugin.${PLUGIN_ID}.job.progress`, ours((payload) => store.trackJob(payload.job_id, payload.state === "running")));
    ctx.onEvent(`plugin.${PLUGIN_ID}.generate.updated`, (event) => {
      const payload = payloadOf(event);
      if (forThisProfile(payload)) store.noteGeneration(payload.card);
    });
    ctx.onEvent(
      `plugin.${PLUGIN_ID}.settings.changed`,
      ours(() => {
        refresh();
        void store.refresh("setup");
      })
    );
    const calls = new Calls();
    const waiting = /* @__PURE__ */ new Map();
    const showFound = (id, revision, ids, said = {}) => {
      if (id) {
        if (calls.has(id)) return;
        calls.add(id);
      }
      if (!ids.length) return;
      store.found(revision, ids, said);
      if (ctx.storage.get(OPEN_MAP_ON_FIND, true)) openMap();
    };
    ctx.onEvent(`plugin.${PLUGIN_ID}.sounds.found`, (event) => {
      const payload = payloadOf(event);
      const id = callId(payload.tool_call_id);
      if (!id || !forChatInFront(payload)) return;
      const ids = Array.isArray(payload.sound_ids) ? payload.sound_ids.filter((value) => typeof value === "string") : [];
      showFound(id, typeof payload.map_revision === "string" ? payload.map_revision : null, ids.slice(0, 200), { near: payload.near, matches: payload.matches });
    });
    ctx.onEvent(`plugin.${PLUGIN_ID}.listen.done`, (event) => {
      const payload = payloadOf(event);
      const id = callId(payload.tool_call_id);
      if (!id || !forChatInFront(payload)) return;
      store.noteListen(payload.result);
      if (calls.has(id) || waiting.has(id)) return;
      const result = payload.result ?? null;
      if (result !== null && !parseListen(result)) return;
      waiting.set(
        id,
        ctx.setTimeout(() => {
          waiting.delete(id);
          if (calls.has(id)) return;
          calls.add(id);
          store.measure(id, result);
          openMeasurements();
        }, LISTEN_FALLBACK_MS)
      );
    });
    ctx.onEvent(`plugin.${PLUGIN_ID}.script.pending`, (event) => {
      const payload = payloadOf(event);
      if (forChatInFront(payload)) store.scriptPending(payload);
    });
    ctx.onEvent(`plugin.${PLUGIN_ID}.script.settled`, (event) => {
      const payload = payloadOf(event);
      if (forChatInFront(payload)) store.scriptSettled(payload.tool_call_id);
    });
    ctx.onEvent("tool.complete", (event) => {
      const payload = payloadOf(event);
      const name = typeof payload.name === "string" ? payload.name : "";
      if (!name.startsWith("kumi_")) return;
      refresh();
      store.trackJob(jobOf(payload.result), name !== "kumi_job");
      const id = callId(payload.tool_id);
      if (name === "kumi_script") store.scriptSettled(id);
      if (name === "kumi_listen") store.noteListen(payload.result);
      if (name === "kumi_listen" && id) {
        calls.add(id);
        waiting.get(id)?.();
        waiting.delete(id);
        return;
      }
      if (name !== "kumi_sounds") return;
      const found = foundSounds(payload.result);
      if (found) showFound(id, found.revision, found.ids, found);
    });
    for (const atom of [host6.state.profile, host6.state.connectionId]) {
      const stop = atom?.listen?.(() => {
        for (const cancel of waiting.values()) cancel();
        waiting.clear();
        calls.clear();
        store.reset();
        audition.reset();
        refresh();
      });
      if (stop) ctx.onDispose(stop);
    }
    ctx.onDispose(() => {
      for (const cancel of waiting.values()) cancel();
      closeMap?.();
      closeMeasurements?.();
      audition.dispose();
    });
    const pet = ctx.pet;
    if (pet && typeof pet.say === "function") {
      let last = store.get();
      ctx.onDispose(
        store.subscribe(() => {
          const now = store.get();
          const line = ctx.storage.get(PET_LINES, false) && pet.visible.get() ? petLine(last, now, wordsOf(ctx)) : null;
          last = now;
          if (line) pet.say(line.text, { id: line.id, tone: line.tone, ttlMs: 8e3 });
        })
      );
    }
    const contributions = [
      {
        id: "pane",
        area: PANES_AREA,
        title: "Kumi",
        data: { placement: "right", width: "320px" },
        render: () => /* @__PURE__ */ jsx17(WithT, { children: /* @__PURE__ */ jsx17(KumiPane, { ctx, onOpenMap: openMap, store }) })
      },
      {
        id: "status",
        area: STATUSBAR_AREAS.right,
        order: 60,
        render: () => /* @__PURE__ */ jsx17(WithT, { children: /* @__PURE__ */ jsx17(StatusChip, { ctx, onOpenMap: openMap, onOpenSettings: openSettings, store }) })
      }
    ];
    if (typeof sdk.TOOL_MEDIA_RENDERER_AREA === "string") {
      contributions.push({
        id: "listen-card",
        area: sdk.TOOL_MEDIA_RENDERER_AREA,
        data: {
          tools: ["kumi_listen"],
          render: (props) => /* @__PURE__ */ jsx17(WithT, { children: /* @__PURE__ */ jsx17(ListenCard, { audition, media: props, store }) })
        }
      });
      contributions.push({
        id: "generate-card",
        area: sdk.TOOL_MEDIA_RENDERER_AREA,
        data: {
          tools: ["kumi_job"],
          render: (props) => /* @__PURE__ */ jsx17(WithT, { children: /* @__PURE__ */ jsx17(GenerateCard, { audition, ctx, media: props, store }) })
        }
      });
    }
    if (typeof sdk.COMPOSER_AREAS?.underside === "string") {
      contributions.push({
        id: "focus-hint",
        area: sdk.COMPOSER_AREAS.underside,
        render: () => /* @__PURE__ */ jsx17(WithT, { children: /* @__PURE__ */ jsx17(FocusHint, { store }) })
      });
    }
    ctx.registerMany(contributions);
    const palette = (t) => [
      {
        id: "open-map",
        area: PALETTE_AREA,
        data: { id: "kumi.map", label: t("palette.map"), keywords: ["kumi", "sound", "samples", "map", "library"], run: openMap }
      },
      {
        id: "undo-last",
        area: PALETTE_AREA,
        data: {
          id: "kumi.undo-last",
          label: t("palette.undo"),
          keywords: ["kumi", "undo", "history", "revert"],
          run: () => {
            const state = store.get();
            const last = changesOf(state.history?.items ?? []).find((rows) => undoRowOf(rows) !== void 0);
            const next = last && undoRowOf(last);
            if (!last || !next || !canUndoChange(state.status, last)) {
              const unsure = Boolean(state.status?.unsure) || next?.state === "unsure" || next?.state === "partial";
              host6.notify({ kind: "info", message: wordsOf(ctx)(unsure ? "notify.undoUnsure" : "notify.nothingToUndo") });
              return;
            }
            void ask(ctx, undoRequest(next), { send: true, session: ownerSession(state) });
          }
        }
      },
      {
        id: "settings",
        area: PALETTE_AREA,
        data: { id: "kumi.settings", label: t("palette.settings"), keywords: ["kumi", "setup", "settings", "daw"], run: openSettings }
      }
    ];
    let unregisterPalette = ctx.registerMany(palette(wordsOf(ctx)));
    ctx.i18n?.onLocaleChange(() => {
      unregisterPalette();
      unregisterPalette = ctx.registerMany(palette(wordsOf(ctx)));
    });
    ctx.registerSettingsPage?.({
      id: "kumi",
      title: "Kumi",
      icon: "pulse",
      render: () => /* @__PURE__ */ jsx17(WithT, { children: /* @__PURE__ */ jsx17(SettingsPage, { ctx, onOpenMap: openMap, store }) })
    });
  }
};
var plugin_default = plugin;
export {
  JOB_POLL_MS,
  LISTEN_FALLBACK_MS,
  MAP_WORKSPACE,
  MEASUREMENTS_WORKSPACE,
  PLUGIN_ID,
  POLL_MS,
  plugin_default as default
};
