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
    agentOff: "Kumi's agent half is off in this profile.",
    agentOffHow: "Turn on the kumi plugin's Agent switch in Capabilities → Plugins, then restart the gateway. This pane shows your DAW's FOCUS and Kumi's HISTORY once it's on.",
    reading: "Reading Kumi's state…",
    backendError: (error) => `Kumi's backend didn't answer: ${error}`,
    unsure: (op, reason) => `Kumi isn't sure your project is as it was after ${op}${reason ? `: ${reason}` : ""}.`,
    unsureOp: "its last action",
    needsLook: "It needs a fresh look before another change.",
    lookAgain: "Ask Kumi to look again",
    focus: "Focus",
    jobs: "Jobs",
    history: "History",
    undoable: (n) => `· ${n} can be undone`,
    undo: "Undo",
    undoTitle: "Ask Kumi to undo this change",
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
    kept: "kept",
    partial: "partly applied",
    unsure: "unsure",
    refused: "refused",
    failed: "failed",
    recorded: "recorded",
    unconfirmed: "Kumi couldn’t confirm what happened.",
    lookFirst: "Ask Kumi to look again before another change.",
    restored: "Kumi put back what it changed meanwhile.",
    restoreUnsure: "Kumi isn’t sure it put everything back."
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
    agentOff: "Kumi's agent half is off in this profile, so there's no map to show.",
    reading: "Reading the sound map…",
    empty: "No sound map yet. Add a sample folder in Kumi's settings and ask Kumi to scan it; the map is made from what it finds there.",
    search: "Search sounds",
    searchHint: "Search the map: name, class or folder",
    count: (n) => `${n} sounds`,
    countOf: (shown, total) => `${shown} of ${total} sounds`,
    revision: (revision) => `map ${revision}`,
    show: (group) => `Show ${group}`,
    hide: (group) => `Hide ${group}`,
    matches: "Sounds that match",
    found: "Sounds Kumi found",
    more: (n) => `and ${n} more on the map`,
    play: "▶ Play",
    stop: "■ Stop",
    playTitle: "Play it here (nothing changes in your DAW)",
    use: "Use…",
    useTitle: "Put a request to use this sound in the chat, to finish and send",
    playFailed: (error) => `Kumi couldn't play that sound: ${error}`
  },
  measurements: {
    tab: "Measurements",
    nothing: "Nothing measured yet. When Kumi measures audio in your chat, its numbers show here as well.",
    tooMuch: "Kumi measured it, but there's too much to show here: the numbers are in its reply in the chat.",
    inReply: "Kumi measured it: the numbers are in its reply in the chat.",
    measured: (ago2) => `Measured ${ago2}`
  },
  listen: {
    measured: "measured",
    measuredTitle: "Measured on this computer by Kumi, from the audio file itself",
    captureTitle: "Kumi played this window and measured it; it isn't a change, so there's nothing to undo",
    waveform: (label) => `Waveform of ${label}`,
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
    agentOff: "Kumi's agent half is off in this profile. Turn on the kumi plugin's Agent switch in Capabilities → Plugins and restart the gateway; its settings form appears under Agent settings.",
    setup: "Setup",
    state: "State",
    next: "Next",
    runtime: "Kumi runtime",
    installed: (version) => `Kumi ${version}`,
    notInstalled: "Not installed",
    pinned: (version) => `pinned ${version}`,
    download: "Download",
    unpacked: (size) => `(${size} unpacked)`,
    part: (daw) => `${daw} part`,
    dawComponent: "DAW component",
    componentLine: (name, state) => `${name}: ${state}`,
    standalone: "Standalone Kumi",
    adopted: "adopted",
    leftAsIs: "left as it is",
    audioFormats: "Audio formats",
    connection: "Connection",
    now: "Now",
    chat: "Kumi's chat",
    openMap: "Open the sound map",
    agentSettings: "Agent settings",
    thisWindow: "This window",
    openOnFind: "Open the sound map when Kumi finds sounds",
    states: {
      ready: "Ready",
      needs_runtime: "Not set up yet",
      outdated_runtime: "Needs the pinned version",
      damaged_runtime: "Needs reinstalling",
      needs_daw_component: "Needs its DAW part",
      no_pin: "No pinned release in this plugin",
      unsupported_platform: "Not available on this system",
      unknown: "Unknown"
    },
    componentStates: {
      not_installed: "not installed",
      installed: "installed",
      outdated: "outdated",
      unknown: "unknown"
    }
  },
  generate: {
    generated: "generated",
    generatedTitle: "Made by Mirelo from your words. It isn't in your project until you place it.",
    soundTitle: "Generated sound",
    notesTitle: "Notes from audio",
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
  palette: {
    map: "Kumi: open the sound map",
    undo: "Kumi: undo the last change",
    settings: "Kumi: settings"
  },
  notify: {
    nothingToUndo: "Kumi has no change it can undo right now.",
    undoUnsure: "Kumi isn't sure of its last change: ask it to look at your project again first.",
    copied: "Copied the request: paste it into the chat to send it.",
    askInChat: (text) => `Ask in the chat: ${text}`,
    openChat: "Open a chat first, then try again.",
    notOpenHere: "Kumi's chat isn't open here, so the request is in your input, unsent. Send it from the chat Kumi is connected to."
  },
  dashboard: {
    agentOff: "Kumi's agent plugin is off in this profile.",
    agentOffHow: "Turn it on (Hermes's plugins settings, or `hermes plugins enable kumi`) and restart the dashboard to see Kumi here.",
    copyTitle: (text) => `Copy “${text}” to paste into a chat`,
    copied: "Copied",
    copyRequest: "Copy request",
    copyUndo: "Copy undo",
    mapEmpty: "No sound map yet: add a sample folder in Kumi's settings and ask Kumi to scan it.",
    askLook: "Ask Kumi in your chat to look again before another change."
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
    agentOff: "このプロファイルでは Kumi のエージェント側がオフです。",
    agentOffHow: "「スキルとツール → プラグイン」で kumi プラグインの Agent スイッチをオンにし、ゲートウェイを再起動してください。オンになると、このペインに DAW の FOCUS と Kumi の HISTORY が表示されます。",
    reading: "Kumi の状態を読み込み中…",
    backendError: (error) => `Kumi のバックエンドが応答しませんでした: ${error}`,
    unsure: (op, reason) => `${op}のあと、プロジェクトが元のままか Kumi は確信できません${reason ? `（${reason}）` : ""}。`,
    unsureOp: "直前の操作",
    needsLook: "次の変更の前に、もう一度確認が必要です。",
    lookAgain: "Kumi にもう一度確認してもらう",
    focus: "Focus",
    jobs: "ジョブ",
    history: "History",
    undoable: (n) => `· ${n} 件取り消せます`,
    undo: "取り消す",
    undoTitle: "この変更の取り消しを Kumi に頼む",
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
    kept: "維持",
    partial: "一部のみ適用",
    unsure: "不確か",
    refused: "拒否",
    failed: "失敗",
    recorded: "記録",
    unconfirmed: "何が起きたか Kumi は確認できませんでした。",
    lookFirst: "次の変更の前に、Kumi にもう一度確認してもらってください。",
    restored: "その間に変えたものは Kumi が元に戻しました。",
    restoreUnsure: "すべて元に戻せたか Kumi は確信できません。"
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
    agentOff: "このプロファイルでは Kumi のエージェント側がオフのため、表示できるマップがありません。",
    reading: "サンプルマップを読み込み中…",
    empty: "まだサンプルマップがありません。Kumi の設定でサンプルフォルダを追加し、Kumi にスキャンを頼んでください。見つかったサウンドからマップが作られます。",
    search: "サウンドを検索",
    searchHint: "マップを検索: 名前・種類・フォルダ",
    count: (n) => `${n} 個のサウンド`,
    countOf: (shown, total) => `${total} 個中 ${shown} 個のサウンド`,
    revision: (revision) => `マップ ${revision}`,
    show: (group) => `${group} を表示`,
    hide: (group) => `${group} を隠す`,
    matches: "一致するサウンド",
    found: "Kumi が見つけたサウンド",
    more: (n) => `ほか ${n} 個はマップ上に`,
    play: "▶ 再生",
    stop: "■ 停止",
    playTitle: "ここで再生（DAW は何も変わりません）",
    use: "使う…",
    useTitle: "このサウンドを使う依頼をチャットに入れます。仕上げて送信してください",
    playFailed: (error) => `Kumi はそのサウンドを再生できませんでした: ${error}`
  },
  measurements: {
    tab: "測定結果",
    nothing: "まだ測定はありません。Kumi がチャットでオーディオを測定すると、その数値がここにも表示されます。",
    tooMuch: "Kumi は測定しましたが、ここに表示するには多すぎます。数値はチャットの返答にあります。",
    inReply: "Kumi は測定しました。数値はチャットの返答にあります。",
    measured: (ago2) => `測定: ${ago2}`
  },
  listen: {
    measured: "測定値",
    measuredTitle: "Kumi がこのコンピュータ上で、オーディオファイルそのものから測定しました",
    captureTitle: "Kumi はこの区間を再生して測定しました。変更ではないので、取り消すものはありません",
    waveform: (label) => `${label} の波形`,
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
    agentOff: "このプロファイルでは Kumi のエージェント側がオフです。「スキルとツール → プラグイン」で kumi プラグインの Agent スイッチをオンにし、ゲートウェイを再起動してください。設定フォームは「エージェント設定」に表示されます。",
    setup: "セットアップ",
    state: "状態",
    next: "次にすること",
    runtime: "Kumi ランタイム",
    installed: (version) => `Kumi ${version}`,
    notInstalled: "未インストール",
    pinned: (version) => `固定版 ${version}`,
    download: "ダウンロード",
    unpacked: (size) => `（展開後 ${size}）`,
    part: (daw) => `${daw} 側の部品`,
    dawComponent: "DAW の部品",
    componentLine: (name, state) => `${name}：${state}`,
    standalone: "単体の Kumi",
    adopted: "引き継ぎ済み",
    leftAsIs: "そのまま",
    audioFormats: "オーディオ形式",
    connection: "接続",
    now: "現在",
    chat: "Kumi のチャット",
    openMap: "サンプルマップを開く",
    agentSettings: "エージェント設定",
    thisWindow: "このウィンドウ",
    openOnFind: "Kumi がサウンドを見つけたらサンプルマップを開く",
    states: {
      ready: "準備完了",
      needs_runtime: "まだセットアップされていません",
      outdated_runtime: "固定版への更新が必要です",
      damaged_runtime: "再インストールが必要です",
      needs_daw_component: "DAW 側の部品が必要です",
      no_pin: "このプラグインには固定版がありません",
      unsupported_platform: "このシステムでは利用できません",
      unknown: "不明"
    },
    componentStates: {
      not_installed: "未インストール",
      installed: "インストール済み",
      outdated: "古い版",
      unknown: "不明"
    }
  },
  generate: {
    generated: "生成",
    generatedTitle: "Mirelo があなたの言葉から作りました。配置するまでプロジェクトには入りません。",
    soundTitle: "生成したサウンド",
    notesTitle: "オーディオから取り出したノート",
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
  palette: {
    map: "Kumi: サンプルマップを開く",
    undo: "Kumi: 最後の変更を取り消す",
    settings: "Kumi: 設定"
  },
  notify: {
    nothingToUndo: "今すぐ取り消せる変更はありません。",
    undoUnsure: "Kumi は直前の変更に確信がありません。まずプロジェクトをもう一度確認するよう頼んでください。",
    copied: "依頼をコピーしました。チャットに貼り付けて送信してください。",
    askInChat: (text) => `チャットで頼んでください: ${text}`,
    openChat: "先にチャットを開いてから、もう一度試してください。",
    notOpenHere: "Kumi のチャットがここで開いていないため、依頼は未送信のまま入力欄にあります。Kumi が接続しているチャットから送信してください。"
  },
  dashboard: {
    agentOff: "このプロファイルでは Kumi のエージェントプラグインがオフです。",
    agentOffHow: "オンにして（Hermes のプラグイン設定、または `hermes plugins enable kumi`）、ダッシュボードを再起動すると、ここに Kumi が表示されます。",
    copyTitle: (text) => `「${text}」をコピーしてチャットに貼り付け`,
    copied: "コピーしました",
    copyRequest: "依頼をコピー",
    copyUndo: "取り消しをコピー",
    mapEmpty: "まだサンプルマップがありません。Kumi の設定でサンプルフォルダを追加し、Kumi にスキャンを頼んでください。",
    askLook: "次の変更の前に、チャットで Kumi にもう一度確認するよう頼んでください。"
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
    agentOff: "此配置中 Kumi 的 Agent 部分已关闭。",
    agentOffHow: "请在“技能与工具 → 插件”中打开 kumi 插件的 Agent 开关，然后重启网关。打开后，此面板会显示 DAW 的 FOCUS 和 Kumi 的 HISTORY。",
    reading: "正在读取 Kumi 的状态…",
    backendError: (error) => `Kumi 的后端没有响应：${error}`,
    unsure: (op, reason) => `在${op}之后，Kumi 不确定你的项目是否保持原样${reason ? `：${reason}` : ""}。`,
    unsureOp: "上一个操作",
    needsLook: "下一次改动之前，需要重新查看一次。",
    lookAgain: "让 Kumi 重新查看",
    focus: "Focus",
    jobs: "任务",
    history: "History",
    undoable: (n) => `· ${n} 项可撤销`,
    undo: "撤销",
    undoTitle: "让 Kumi 撤销这次改动",
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
    kept: "已保留",
    partial: "部分应用",
    unsure: "不确定",
    refused: "已拒绝",
    failed: "失败",
    recorded: "已记录",
    unconfirmed: "Kumi 无法确认发生了什么。",
    lookFirst: "下一次改动之前，请让 Kumi 重新查看。",
    restored: "Kumi 已还原期间做出的改动。",
    restoreUnsure: "Kumi 不确定是否已全部还原。"
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
    agentOff: "此配置中 Kumi 的 Agent 部分已关闭，没有可显示的地图。",
    reading: "正在读取采样地图…",
    empty: "还没有采样地图。请在 Kumi 设置中添加采样文件夹，并让 Kumi 扫描；地图会根据找到的声音生成。",
    search: "搜索声音",
    searchHint: "搜索地图：名称、类别或文件夹",
    count: (n) => `${n} 个声音`,
    countOf: (shown, total) => `${total} 个声音中的 ${shown} 个`,
    revision: (revision) => `地图 ${revision}`,
    show: (group) => `显示 ${group}`,
    hide: (group) => `隐藏 ${group}`,
    matches: "匹配的声音",
    found: "Kumi 找到的声音",
    more: (n) => `地图上还有 ${n} 个`,
    play: "▶ 播放",
    stop: "■ 停止",
    playTitle: "在这里播放（DAW 不会有任何改动）",
    use: "使用…",
    useTitle: "把使用这个声音的请求放进对话，补充完整后发送",
    playFailed: (error) => `Kumi 无法播放这个声音：${error}`
  },
  measurements: {
    tab: "测量结果",
    nothing: "还没有测量。Kumi 在对话中测量音频时，数值也会显示在这里。",
    tooMuch: "Kumi 已完成测量，但内容太多无法在此显示：数值在它在对话中的回复里。",
    inReply: "Kumi 已完成测量：数值在它在对话中的回复里。",
    measured: (ago2) => `测量于 ${ago2}`
  },
  listen: {
    measured: "实测",
    measuredTitle: "由 Kumi 在这台电脑上根据音频文件本身测量",
    captureTitle: "Kumi 播放并测量了这个片段；这不是改动，所以没有需要撤销的内容",
    waveform: (label) => `${label} 的波形`,
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
    agentOff: "此配置中 Kumi 的 Agent 部分已关闭。请在“技能与工具 → 插件”中打开 kumi 插件的 Agent 开关并重启网关；它的设置表单会出现在“智能体设置”中。",
    setup: "设置",
    state: "状态",
    next: "下一步",
    runtime: "Kumi 运行时",
    installed: (version) => `Kumi ${version}`,
    notInstalled: "未安装",
    pinned: (version) => `固定版本 ${version}`,
    download: "下载",
    unpacked: (size) => `（解压后 ${size}）`,
    part: (daw) => `${daw} 组件`,
    dawComponent: "DAW 组件",
    componentLine: (name, state) => `${name}：${state}`,
    standalone: "独立的 Kumi",
    adopted: "已接管",
    leftAsIs: "保持不变",
    audioFormats: "音频格式",
    connection: "连接",
    now: "当前",
    chat: "Kumi 的对话",
    openMap: "打开采样地图",
    agentSettings: "智能体设置",
    thisWindow: "此窗口",
    openOnFind: "Kumi 找到声音时打开采样地图",
    states: {
      ready: "就绪",
      needs_runtime: "尚未设置",
      outdated_runtime: "需要固定版本",
      damaged_runtime: "需要重新安装",
      needs_daw_component: "需要 DAW 组件",
      no_pin: "此插件没有固定版本",
      unsupported_platform: "此系统上不可用",
      unknown: "未知"
    },
    componentStates: {
      not_installed: "未安装",
      installed: "已安装",
      outdated: "版本过旧",
      unknown: "未知"
    }
  },
  generate: {
    generated: "生成",
    generatedTitle: "由 Mirelo 根据你的描述生成。放入项目之前，它不会出现在你的项目中。",
    soundTitle: "生成的声音",
    notesTitle: "从音频提取的音符",
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
  palette: {
    map: "Kumi：打开采样地图",
    undo: "Kumi：撤销最近一次改动",
    settings: "Kumi：设置"
  },
  notify: {
    nothingToUndo: "Kumi 现在没有可以撤销的改动。",
    undoUnsure: "Kumi 不确定它的上一次改动：请先让它重新查看你的项目。",
    copied: "已复制请求：粘贴到对话中发送。",
    askInChat: (text) => `请在对话中提出：${text}`,
    openChat: "请先打开一个对话，然后重试。",
    notOpenHere: "Kumi 的对话没有在这里打开，所以请求还在你的输入框中，尚未发送。请在 Kumi 连接的对话中发送。"
  },
  dashboard: {
    agentOff: "此配置中 Kumi 的 Agent 插件已关闭。",
    agentOffHow: "打开它（Hermes 的插件设置，或 `hermes plugins enable kumi`）并重启仪表板，即可在这里看到 Kumi。",
    copyTitle: (text) => `复制“${text}”以粘贴到对话中`,
    copied: "已复制",
    copyRequest: "复制请求",
    copyUndo: "复制撤销",
    mapEmpty: "还没有采样地图：请在 Kumi 设置中添加采样文件夹，并让 Kumi 扫描。",
    askLook: "下一次改动之前，请在对话中让 Kumi 重新查看。"
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
function placeRequest(artifactId, name) {
  const id = plainId(artifactId);
  if (!id) return null;
  const said = clean(name ?? "", 80);
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
  const said = clean(name ?? "", 80);
  const which = `sound ${id}${from ? ` from map ${from}` : ""}`;
  return said ? `Use “${said}” (${which}) on the selected track` : `Use ${which} on the selected track`;
}
function setupRequest(daw) {
  return `Set up Kumi for ${dawName(daw)}`;
}
function connectRequest(daw) {
  return `Connect Kumi to ${dawName(daw)}`;
}
var LOOK_AGAIN_REQUEST = "Kumi, look at my project again before changing anything else";
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
import { useEffect, useSyncExternalStore } from "react";
function useKumi(store) {
  return useSyncExternalStore(store.subscribe, store.get, store.get);
}
function useWant(store, part) {
  useEffect(() => store.want(part), [store, part]);
}
var AUDITION_CACHE_CHARS = 48 * 1024 * 1024;
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
  /** Moves on with every play and stop: a play whose turn has passed never starts its sound. */
  turn = 0;
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
   *  there's never more than one sound playing, and never one that can't be stopped. */
  async play(id) {
    this.stop();
    const turn = this.turn;
    this.changed(id);
    try {
      const url = await this.url(id);
      if (turn !== this.turn) return;
      const audio = new Audio(url);
      this.audio = audio;
      audio.onended = () => {
        if (this.audio === audio) this.stop();
      };
      await audio.play();
    } catch (error) {
      if (turn === this.turn) this.stop();
      throw error;
    }
  }
  stop() {
    this.turn += 1;
    if (this.audio) {
      this.audio.pause();
      this.audio.onended = null;
      this.audio = null;
    }
    if (this.playing !== null) this.changed(null);
  }
  dispose() {
    this.reset();
    this.listeners.clear();
  }
};
function usePlaying(audition) {
  return useSyncExternalStore(audition.subscribe, () => audition.playing, () => audition.playing);
}

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
function listenView(result, t = english) {
  const mine = result.metrics ?? {};
  const compare = result.compare;
  const reference = compare?.metrics;
  const differences = compare?.differences;
  const theirs = (value) => compare ? value ?? null : void 0;
  const rows = [
    row(t("listen.loudness"), mine.loudness?.integratedLufs, theirs(reference?.loudness?.integratedLufs), (v) => `${num(v)} LUFS`, differences?.loudness?.differenceLu, (v) => `${signed(v)} LU`),
    row(t("listen.truePeak"), mine.loudness?.truePeakDbtp, theirs(reference?.loudness?.truePeakDbtp), (v) => `${num(v)} dBTP`, void 0, (v) => `${signed(v)} dB`),
    row(t("listen.brightness"), mine.balance?.centroidHz, theirs(reference?.balance?.centroidHz), hz, void 0, (v) => `${signed(v, 0)} Hz`),
    row(t("listen.range"), mine.loudness?.rangeLu, theirs(reference?.loudness?.rangeLu), (v) => `${num(v)} LU`, void 0, (v) => `${signed(v)} LU`),
    row(t("listen.crest"), mine.dynamics?.crestDb, theirs(reference?.dynamics?.crestDb), (v) => `${num(v)} dB`, void 0, (v) => `${signed(v)} dB`),
    row(t("listen.tempo"), mine.tempo?.bpm, theirs(reference?.tempo?.bpm), (v) => `${num(v)} BPM`, void 0, (v) => `${signed(v)} BPM`)
  ].filter((value) => value !== null);
  const key = mine.key?.name;
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
  const view = {
    title: fileName(mine, t("listen.measuredAudio")),
    mineId: typeof result.artifact_id === "string" ? result.artifact_id : void 0,
    rows,
    bands: differenceBands.length ? differenceBands : levelBands,
    bandsAre: differenceBands.length ? "difference" : "level",
    headlines: Array.isArray(differences?.headlines) ? differences.headlines.filter((line) => typeof line === "string").map((line) => clean(line, 140)) : [],
    verdict: typeof result.verdict_text === "string" && result.verdict_text ? clean(result.verdict_text, 400) : void 0,
    capture: captureWords(result.capture, t),
    // CC-2 A3: one line of at most 200 characters, shown as it is and never read for a decision.
    captureNote: typeof result.capture?.note === "string" && result.capture.note.trim() ? clean(result.capture.note, 200) : void 0
  };
  if (compare) {
    view.referenceTitle = fileName(reference, t("listen.reference"));
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

// src/shared/store.ts
var JOB_KEPT_MS = 6e4;
var MOST_JOBS = 20;
var EMPTY = { available: null, error: null, status: null, focus: null, history: null, map: null, setup: null, found: null, jobs: {}, measured: null };
function finished(job) {
  return typeof job.state === "string" && job.state !== "running";
}
function statusOf(error) {
  const text = error instanceof Error ? error.message : String(error ?? "");
  const match = /^(\d{3})\b/.exec(text.trim()) ?? /\b(\d{3})\b/.exec(text);
  return match ? Number(match[1]) : 0;
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
      const held = { focus: this.state.focus?.seq ?? null, history: this.state.history?.seq ?? null, map: this.state.map?.revision ?? null };
      const moved = (part) => this.state[part] === null || (after[part] ?? null) !== held[part];
      const reads = [];
      if (liveness || moved("focus")) reads.push(this.refresh("focus"));
      if (liveness || moved("history")) reads.push(this.refresh("history"));
      if ((this.wanted.get("map") ?? 0) > 0 && moved("map")) reads.push(this.refresh("map"));
      if (first || (this.wanted.get("setup") ?? 0) > 0) reads.push(this.refresh("setup"));
      await Promise.all(reads);
    });
  }
  refresh(part) {
    return this.once(part, async () => {
      const generation = this.generation;
      const path = part === "history" ? `/history?limit=${this.historyLimit}` : `/${part}`;
      try {
        const value = await this.fetch(path);
        if (generation === this.generation) this.set({ [part]: heard(value) });
      } catch (error) {
        if (generation !== this.generation) return;
        if (statusOf(error) === 404 && part !== "setup") this.set({ available: false });
      }
    });
  }
  /** The sounds a `kumi_sounds` call found, to light up on the map of the same revision. */
  found(revision, ids) {
    this.set({ found: { revision, ids: ids.slice(0, 200), at: Date.now() } });
  }
  /** A `kumi_listen` result to show in the Measurements tab (CC-9: the transcript showed none). */
  measure(toolCallId, result) {
    this.set({ measured: { tool_call_id: toolCallId, result: result ?? null, at: Date.now() } });
  }
  /** Follow job `id` (from a tool's result or a `job.progress` event): read it now, then while it runs. */
  trackJob(id) {
    const jobId = plainId(id);
    if (!jobId || this.state.jobs[jobId] || Object.keys(this.state.jobs).length >= MOST_JOBS) return;
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
        const waiting = this.cards.has(id);
        if (!before && finished(read) && !waiting) return;
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
  return { revision, ids };
}

// src/desktop/theme.ts
var T = {
  text: "var(--ui-text-primary)",
  secondary: "var(--ui-text-secondary)",
  muted: "var(--ui-text-tertiary)",
  faint: "var(--ui-text-quaternary)",
  border: "var(--ui-stroke-secondary)",
  card: "var(--ui-bg-card)",
  chip: "var(--ui-bg-tertiary)",
  accent: "var(--ui-accent)",
  good: "var(--ui-green)",
  warn: "var(--ui-yellow)",
  bad: "var(--ui-red)"
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
  ellipsis: { overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", minWidth: 0 }
};
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
  disabled,
  strong
}) {
  return /* @__PURE__ */ jsx(
    "button",
    {
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
        border: `1px solid ${strong ? T.accent : T.border}`,
        background: "transparent",
        color: disabled ? T.faint : strong ? T.accent : T.secondary,
        cursor: disabled ? "default" : "pointer",
        whiteSpace: "nowrap",
        flex: "none"
      },
      children
    }
  );
}
function Section({ title, right, children }) {
  return /* @__PURE__ */ jsxs("section", { style: S.section, children: [
    /* @__PURE__ */ jsxs("div", { style: { ...S.heading, justifyContent: "space-between" }, children: [
      /* @__PURE__ */ jsx("span", { style: { ...S.row, gap: 6 }, children: title }),
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
    if (await composer.insertText(null, text)) return { outcome: "drafted" };
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

// src/desktop/card.tsx
import { useEffect as useEffect2, useState } from "react";
import { jsx as jsx2, jsxs as jsxs2 } from "react/jsx-runtime";
async function peaksOf(dataUrl, buckets = 96) {
  const comma = dataUrl.indexOf(",");
  const binary = atob(dataUrl.slice(comma + 1));
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  const decoded = await new OfflineAudioContext(1, 1, 44100).decodeAudioData(bytes.buffer);
  const samples = decoded.getChannelData(0);
  const step = Math.max(1, Math.floor(samples.length / buckets));
  const peaks = [];
  for (let start = 0; start < samples.length && peaks.length < buckets; start += step) {
    let peak = 0;
    for (let i = start; i < Math.min(samples.length, start + step); i++) peak = Math.max(peak, Math.abs(samples[i]));
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
      /* @__PURE__ */ jsx2(TextButton, { onClick: () => playing ? audition.stop() : void audition.play(id).catch(() => setFailed(true)), children: playing ? "■" : "▶" })
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
  return /* @__PURE__ */ jsxs2("table", { style: { borderCollapse: "collapse", fontSize: 12, color: T.text, width: "100%" }, children: [
    compared && /* @__PURE__ */ jsx2("thead", { children: /* @__PURE__ */ jsxs2("tr", { style: { color: T.muted, fontSize: 11 }, children: [
      /* @__PURE__ */ jsx2("th", { style: { ...cell, textAlign: "left", fontWeight: 400 } }),
      /* @__PURE__ */ jsx2("th", { style: { ...cell, fontWeight: 400 }, children: t("listen.this") }),
      /* @__PURE__ */ jsx2("th", { style: { ...cell, fontWeight: 400 }, children: t("listen.reference") }),
      /* @__PURE__ */ jsx2("th", { style: { ...cell, fontWeight: 400 }, children: t("listen.difference") })
    ] }) }),
    /* @__PURE__ */ jsx2("tbody", { children: view.rows.map((row2) => /* @__PURE__ */ jsxs2("tr", { style: { borderTop: `1px solid ${T.border}` }, children: [
      /* @__PURE__ */ jsx2("td", { style: { ...cell, textAlign: "left", color: T.muted }, children: row2.label }),
      /* @__PURE__ */ jsx2("td", { style: cell, children: row2.mine }),
      compared && /* @__PURE__ */ jsx2("td", { style: { ...cell, color: T.muted }, children: row2.reference ?? "—" }),
      compared && /* @__PURE__ */ jsx2("td", { style: { ...cell, color: row2.direction === 0 ? T.muted : T.text, fontWeight: 600 }, children: row2.delta ?? "—" })
    ] }, row2.label)) })
  ] });
}
function Bands({ view }) {
  const t = useT();
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
function ListenCard({ media, audition }) {
  if (media.file?.kind !== "audio") return media.fallback;
  return /* @__PURE__ */ jsx2(ListenResultCard, { audition, fallback: media.fallback, result: media.result });
}
function ListenResultCard({ result, audition, fallback }) {
  const t = useT();
  const parsed = parseListen(result);
  if (!parsed) return fallback;
  const view = listenView(parsed, t);
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
        view.mineId ? /* @__PURE__ */ jsxs2("div", { style: { ...S.row, gap: 12, flexWrap: "wrap", alignItems: "flex-start" }, children: [
          /* @__PURE__ */ jsx2(Wave, { audition, id: view.mineId, label: view.title }),
          view.referenceId && /* @__PURE__ */ jsx2(Wave, { audition, id: view.referenceId, label: view.referenceTitle ?? t("listen.reference") })
        ] }) : fallback,
        /* @__PURE__ */ jsx2(Rows, { view }),
        /* @__PURE__ */ jsx2(Bands, { view }),
        view.headlines.length > 0 && /* @__PURE__ */ jsx2("ul", { style: { margin: 0, paddingLeft: 16, display: "flex", flexDirection: "column", gap: 1 }, children: view.headlines.map((line, index) => /* @__PURE__ */ jsx2("li", { style: S.muted, children: line }, index)) }),
        /* @__PURE__ */ jsx2(Interpretation, { view })
      ]
    }
  );
}

// src/shared/generate.ts
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
  return {
    state,
    kind: typeof result.kind === "string" ? clean(result.kind, 24) : "",
    prompt: typeof result.prompt === "string" ? clean(result.prompt, 200) : "",
    artifactId: plainId(result.artifact_id),
    file: typeof result.file === "string" ? clean(result.file.split(/[\\/]/).pop() ?? "", 80) : "",
    outputType: typeof result.output_type === "string" ? clean(result.output_type, 16) : null,
    seconds: positive(result.seconds),
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
  return made.state !== "running" && (made.submission === "not_submitted" || made.error?.code === "not_submitted");
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
  if (made.outputType === null) return made.kind === "sfx" ? "audio" : made.kind === "audio_to_midi" ? "notes" : null;
  if (made.outputType === "audio") return "audio";
  return NOTES.has(made.outputType) ? "notes" : null;
}
function generationTitle(made, t = english) {
  return made.prompt || (outputKind(made) === "notes" ? t("generate.notesTitle") : t("generate.soundTitle"));
}
function generationJobWords(job, t = english) {
  const words = jobWords(job, t);
  const made = parseGeneration({ ...job, kind: "generate" });
  if (!made) return words;
  const frozen = (made.ledger ?? parseGeneration(job.card)?.ledger)?.frozen === true;
  const over = made.actual !== null && made.reserved !== null && made.actual > made.reserved;
  const reason = made.state === "failed" || made.state === "interrupted" ? made.error?.reason ?? "" : "";
  const detail = [made.state === "running" ? made.message : made.file, reason, costWords(made, t), frozen ? t("generate.frozen") : ""];
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
import { useEffect as useEffect3, useState as useState2 } from "react";

// src/shared/HistoryView.tsx
import { jsx as jsx3, jsxs as jsxs3 } from "react/jsx-runtime";
var STATE = {
  applied: { mark: "✓", tone: "good", word: "history.applied" },
  undone: { mark: "↺", tone: "muted", word: "history.undone" },
  kept: { mark: "✓", tone: "good", word: "history.kept" },
  partial: { mark: "◐", tone: "warn", word: "history.partial" },
  unsure: { mark: "?", tone: "warn", word: "history.unsure" },
  refused: { mark: "✕", tone: "muted", word: "history.refused" },
  failed: { mark: "✕", tone: "bad", word: "history.failed" }
};
function stateOf(item, t = english) {
  const known = STATE[item.state ?? ""];
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
function HistoryView({ history, now, action }) {
  const t = useT();
  const items = history?.items ?? [];
  if (!items.length) {
    return /* @__PURE__ */ jsx3("span", { style: S.muted, children: t("history.empty") });
  }
  return /* @__PURE__ */ jsx3("ol", { style: { listStyle: "none", margin: 0, padding: 0, display: "flex", flexDirection: "column", gap: 2 }, children: items.map((item) => {
    const state = stateOf(item, t);
    const track = nameOf(item.track);
    return /* @__PURE__ */ jsxs3("li", { style: { ...S.row, gap: 7, padding: "4px 0", borderTop: `1px solid ${T.border}`, opacity: item.state === "undone" ? 0.7 : 1 }, children: [
      /* @__PURE__ */ jsx3("span", { "aria-label": state.word, title: state.word, style: { width: 14, textAlign: "center", color: TONE[state.tone], flex: "none" }, children: state.mark }),
      /* @__PURE__ */ jsxs3("span", { style: { display: "flex", flexDirection: "column", minWidth: 0, flex: 1 }, children: [
        /* @__PURE__ */ jsx3("span", { style: { ...S.text, ...S.ellipsis, textDecoration: item.state === "undone" ? "line-through" : void 0 }, title: clean(item.title ?? "", 300), children: clean(item.title ?? item.label ?? item.id, 120) }),
        /* @__PURE__ */ jsxs3("span", { style: { ...S.row, gap: 5 }, children: [
          track && /* @__PURE__ */ jsx3(Dot, { color: colorOf(item.track) ?? T.faint, size: 6 }),
          /* @__PURE__ */ jsx3("span", { style: { ...S.faint, ...S.ellipsis }, children: [track && clean(track, 24), item.family && clean(item.family, 16), item.label && clean(item.label, 12), ago(item.at, now, t)].filter(Boolean).join(" · ") })
        ] }),
        item.state === "unsure" || item.state === "partial" ? /* @__PURE__ */ jsx3("span", { style: { ...S.muted, color: T.warn, whiteSpace: "normal" }, children: [clean(item.reason ?? item.note ?? t("history.unconfirmed"), 200), cleanupWords(item.cleanup, t), t("history.lookFirst")].filter(Boolean).join(" ") }) : (
          // The runtime's own words for any other row: why an undo didn't happen, say.
          item.note && /* @__PURE__ */ jsx3("span", { style: { ...S.muted, whiteSpace: "normal" }, children: clean(item.note, 200) })
        )
      ] }),
      action?.(item)
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

// src/desktop/pane.tsx
import { Fragment, jsx as jsx5, jsxs as jsxs5 } from "react/jsx-runtime";
function useNow(every = 3e4) {
  const [now, setNow] = useState2(() => Date.now());
  useEffect3(() => {
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
function nextStep(state, t = english) {
  const setup = state.setup;
  const daw = state.status?.daw ?? setup?.daw;
  if (setup && (NEEDS_SETUP[setup.state ?? ""] || setup.next_step === "install_runtime" || setup.next_step === "install_daw_component")) {
    return { label: t("step.setup", dawName(daw, t)), request: setupRequest(daw) };
  }
  if (setup?.state === "ready" && !state.status?.connected) return { label: t("step.connect", dawName(daw, t)), request: connectRequest(daw) };
  return null;
}
var TONE3 = { good: T.good, warn: T.warn, bad: T.bad, muted: T.faint };
function KumiPane({ ctx, store, onOpenMap }) {
  const t = useT();
  const state = useKumi(store);
  const now = useNow();
  if (state.available === false) {
    return /* @__PURE__ */ jsxs5("div", { style: { padding: 14, display: "flex", flexDirection: "column", gap: 8 }, children: [
      /* @__PURE__ */ jsx5("span", { style: S.text, children: t("pane.agentOff") }),
      /* @__PURE__ */ jsx5("span", { style: S.muted, children: t("pane.agentOffHow") })
    ] });
  }
  if (state.available === null) {
    return /* @__PURE__ */ jsx5("div", { style: { ...S.muted, padding: 14 }, children: t("pane.reading") });
  }
  const words = chipWords(state.status, state.focus?.transport?.tempo, t);
  const step = nextStep(state, t);
  const session = ownerSession(state);
  const connected = Boolean(state.status?.connected);
  const setupState = state.setup?.state;
  const setupMessage = typeof state.setup?.message === "string" && setupState && setupState !== "ready" && setupState !== "unknown" ? state.setup.message : null;
  const undoable = (state.history?.items ?? []).filter((item) => canUndo(state.status, item)).length;
  return /* @__PURE__ */ jsxs5("div", { style: { height: "100%", overflowY: "auto", overflowX: "hidden", color: T.text }, children: [
    /* @__PURE__ */ jsxs5("div", { style: { ...S.section, gap: 8 }, children: [
      /* @__PURE__ */ jsxs5("div", { style: { ...S.row, gap: 8 }, children: [
        /* @__PURE__ */ jsx5(Dot, { color: TONE3[words.tone], size: 8 }),
        /* @__PURE__ */ jsx5("span", { style: { ...S.text, ...S.ellipsis, fontWeight: 600, flex: 1 }, children: words.text }),
        state.status?.runtime?.version && /* @__PURE__ */ jsxs5("span", { style: S.faint, children: [
          "Kumi ",
          state.status.runtime.version
        ] })
      ] }),
      setupMessage && /* @__PURE__ */ jsx5("span", { style: S.muted, children: setupMessage }),
      state.error && /* @__PURE__ */ jsx5("span", { style: { ...S.muted, color: T.warn }, children: t("pane.backendError", state.error) }),
      state.status?.unsure && /* @__PURE__ */ jsxs5("div", { style: { display: "flex", flexDirection: "column", gap: 4 }, children: [
        /* @__PURE__ */ jsx5("span", { style: { ...S.muted, color: T.warn }, children: [
          t("pane.unsure", clean(state.status.unsure.op ?? t("pane.unsureOp"), 40), state.status.unsure.reason ? clean(state.status.unsure.reason, 160) : ""),
          cleanupWords(state.status.unsure.cleanup, t),
          t("pane.needsLook")
        ].filter(Boolean).join(" ") }),
        /* @__PURE__ */ jsx5("div", { style: S.row, children: /* @__PURE__ */ jsx5(TextButton, { onClick: () => void ask(ctx, LOOK_AGAIN_REQUEST, { send: true, session }), children: t("pane.lookAgain") }) })
      ] }),
      step && /* @__PURE__ */ jsx5("div", { style: S.row, children: /* @__PURE__ */ jsx5(TextButton, { onClick: () => void ask(ctx, step.request, { send: true, session: connected ? session : null }), strong: true, children: step.label }) })
    ] }),
    /* @__PURE__ */ jsx5(Section, { title: t("pane.focus"), children: /* @__PURE__ */ jsx5(FocusView, { focus: state.focus, now }) }),
    Object.keys(state.jobs).length > 0 && /* @__PURE__ */ jsx5(Section, { title: t("pane.jobs"), children: /* @__PURE__ */ jsx5(JobsView, { jobs: state.jobs, onRequest: (request) => void ask(ctx, request, { send: false, session }) }) }),
    /* @__PURE__ */ jsx5(
      Section,
      {
        title: /* @__PURE__ */ jsxs5(Fragment, { children: [
          t("pane.history"),
          " ",
          /* @__PURE__ */ jsx5("span", { style: { textTransform: "none", letterSpacing: 0 }, children: undoable ? t("pane.undoable", undoable) : "" })
        ] }),
        children: /* @__PURE__ */ jsx5(
          HistoryView,
          {
            history: state.history,
            now,
            action: (item) => canUndo(state.status, item) ? /* @__PURE__ */ jsx5(TextButton, { onClick: () => void ask(ctx, undoRequest(item), { send: true, session }), title: t("pane.undoTitle"), children: t("pane.undo") }) : null
          }
        )
      }
    ),
    /* @__PURE__ */ jsxs5("div", { style: { ...S.row, padding: "10px 12px", gap: 8 }, children: [
      /* @__PURE__ */ jsx5(TextButton, { onClick: onOpenMap, children: t("pane.openMap") }),
      !connected && state.status?.owner && /* @__PURE__ */ jsx5("span", { style: S.faint, children: t("pane.undoNeedsConnection") })
    ] })
  ] });
}

// src/desktop/generate.tsx
import { jsx as jsx6, jsxs as jsxs6 } from "react/jsx-runtime";
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
  const offer = generationRequest(made, t);
  return /* @__PURE__ */ jsxs6(
    "div",
    {
      "data-kumi-card": "generate",
      style: { display: "flex", flexDirection: "column", gap: 8, maxWidth: 560, padding: "10px 12px", borderRadius: 6, border: `1px solid ${T.border}`, background: T.card, color: T.text },
      children: [
        /* @__PURE__ */ jsxs6("div", { style: { ...S.row, justifyContent: "space-between", alignItems: "flex-start", gap: 8 }, children: [
          /* @__PURE__ */ jsxs6("span", { style: { ...S.text, fontWeight: 600, minWidth: 0, whiteSpace: "normal" }, children: [
            /* @__PURE__ */ jsx6("span", { style: { color: T.accent }, children: kind === "notes" ? "♫" : "♪" }),
            " ",
            made.prompt ? `“${title}”` : title
          ] }),
          /* @__PURE__ */ jsxs6("span", { style: { ...S.row, gap: 4, flexShrink: 0 }, children: [
            made.outputType && OUTPUTS.has(made.outputType) && /* @__PURE__ */ jsx6(Chip, { children: t(`generate.outputs.${made.outputType}`) }),
            made.noteCount !== null && /* @__PURE__ */ jsx6(Chip, { children: t("generate.notes", made.noteCount, String(made.noteCount)) }),
            made.seconds !== null && /* @__PURE__ */ jsx6(Chip, { children: seconds(made.seconds) }),
            ENDED.has(made.state) ? /* @__PURE__ */ jsx6(Chip, { tone: "bad", children: t(`generate.states.${made.state}`) }) : /* @__PURE__ */ jsx6(Chip, { title: t("generate.generatedTitle"), tone: "accent", children: t("generate.generated") })
          ] })
        ] }),
        made.file && /* @__PURE__ */ jsx6("span", { style: { ...S.faint, ...S.ellipsis }, children: made.file }),
        kind === "audio" && made.artifactId && media.file?.kind === "audio" ? (
          // In a row, as in the listen card: the waveform's flex basis is a width there, not a height.
          /* @__PURE__ */ jsx6("div", { style: { display: "flex" }, children: /* @__PURE__ */ jsx6(Wave, { audition, caption: "", id: made.artifactId, label: title }) })
        ) : (
          // Notes, MIDI and MusicXML: Desktop's own file chip, to open or save it.
          media.fallback
        ),
        ENDED.has(made.state) && made.error?.reason && /* @__PURE__ */ jsx6("span", { style: { ...S.muted, whiteSpace: "normal" }, children: made.error.reason }),
        (cost || left) && /* @__PURE__ */ jsxs6("span", { style: { ...S.row, gap: 6, flexWrap: "wrap" }, children: [
          cost && /* @__PURE__ */ jsx6(Chip, { title: t("generate.creditsTitle"), children: cost }),
          left && /* @__PURE__ */ jsx6("span", { style: S.faint, title: t("generate.capTitle"), children: left })
        ] }),
        made.ledger?.frozen && /* @__PURE__ */ jsx6("span", { style: { ...S.text, color: T.warn, whiteSpace: "normal" }, children: t("generate.frozen") }),
        offer && /* @__PURE__ */ jsx6("span", { children: /* @__PURE__ */ jsx6(TextButton, { onClick: () => void ask(ctx, offer.request, { send: false, session: ownerSession(store.get()) }), strong: true, title: offer.title, children: offer.label }) })
      ]
    }
  );
}

// src/desktop/composer.tsx
import { host as host2 } from "@hermes/plugin-sdk";
import { useCallback, useSyncExternalStore as useSyncExternalStore2 } from "react";
import { jsx as jsx7, jsxs as jsxs7 } from "react/jsx-runtime";
var noChange = () => () => {
};
function useFocusedChat() {
  const atom = host2.state.focusedStoredSessionId;
  const subscribe = useCallback(
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
  return /* @__PURE__ */ jsxs7(
    "div",
    {
      "data-unsure": unsure || void 0,
      style: { ...S.row, gap: 6, padding: "2px 10px", maxWidth: "100%", opacity: unsure ? 0.5 : 1 },
      title: unsure ? t("hint.titleUnsure") : t("hint.title"),
      children: [
        /* @__PURE__ */ jsx7(Dot, { color: unsure ? T.faint : colorOf(state.focus?.focus?.track) ?? T.faint, size: 7 }),
        /* @__PURE__ */ jsx7("span", { style: { ...S.faint, ...S.ellipsis }, children: `${unsure ? t("hint.lastSaw") : t("hint.sees")} ${path.join(" › ")}${tempo ? ` · ${tempo}` : ""}` })
      ]
    }
  );
}

// src/desktop/chip.tsx
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger, StatusDot } from "@hermes/plugin-sdk";
import { jsx as jsx8, jsxs as jsxs8 } from "react/jsx-runtime";
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
  const step = nextStep(state, t);
  const connected = Boolean(state.status?.connected);
  return /* @__PURE__ */ jsxs8(DropdownMenu, { children: [
    /* @__PURE__ */ jsx8(DropdownMenuTrigger, { asChild: true, children: /* @__PURE__ */ jsxs8(
      "button",
      {
        "aria-label": t("chip.label", words.text),
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
          /* @__PURE__ */ jsx8(StatusDot, { tone: words.tone }),
          words.text
        ]
      }
    ) }),
    /* @__PURE__ */ jsxs8(DropdownMenuContent, { align: "end", side: "top", children: [
      /* @__PURE__ */ jsx8(DropdownMenuItem, { onSelect: onOpenMap, children: t("chip.openMap") }),
      step && /* @__PURE__ */ jsx8(DropdownMenuItem, { onSelect: () => void ask(ctx, step.request, { send: true, session: connected ? ownerSession(state) : null }), children: step.label }),
      /* @__PURE__ */ jsx8(DropdownMenuSeparator, {}),
      /* @__PURE__ */ jsx8(DropdownMenuItem, { onSelect: onOpenSettings, children: t("chip.settings") })
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
import { useState as useState4 } from "react";

// src/shared/MapCanvas.tsx
import { useEffect as useEffect4, useMemo, useRef, useState as useState3 } from "react";

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
function searchSounds(model, query) {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return null;
  const found = /* @__PURE__ */ new Set();
  for (const point of model.points) {
    const sound = model.sounds[point.id];
    const text = [sound?.name, sound?.class, sound?.kind, sound?.folder].filter(Boolean).join(" ").toLowerCase();
    if (words.every((word) => text.includes(word))) found.add(point.id);
  }
  return found;
}
function frame(model, ids, width, height, margin = 48) {
  const chosen = model.points.filter((point) => ids.has(point.id));
  if (!chosen.length) return null;
  const xs = chosen.map((point) => point.x);
  const ys = chosen.map((point) => point.y);
  const [x0, x1, y0, y1] = [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)];
  const size = Math.max(1, Math.min(width, height) - 32);
  const span = Math.max(x1 - x0, y1 - y0, 0.04);
  const scale = Math.min(40, Math.max(1, (Math.min(width, height) - 2 * margin) / (span * size)));
  const cx = (x0 + x1) / 2;
  const cy = (y0 + y1) / 2;
  return { scale, dx: width / 2 - cx * size * scale, dy: height / 2 - cy * size * scale };
}

// src/shared/MapCanvas.tsx
import { jsx as jsx9, jsxs as jsxs9 } from "react/jsx-runtime";
var MATCHES_LISTED = 6;
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
function MapCanvas({ map, found, playing, selected, onPick, actions, empty }) {
  const t = useT();
  const model = useMemo(() => buildModel(map), [map]);
  const canvasRef = useRef(null);
  const boxRef = useRef(null);
  const [size, setSize] = useState3({ width: 0, height: 0 });
  const [view, setView] = useState3(null);
  const [query, setQuery] = useState3("");
  const [hidden, setHidden] = useState3(() => /* @__PURE__ */ new Set());
  const [hover, setHover] = useState3(null);
  const drag = useRef(null);
  const mapSize = Math.max(1, Math.min(size.width, size.height) - 32);
  const hasPoints = model.points.length > 0;
  const searched = useMemo(() => searchSounds(model, query), [model, query]);
  const indexOf = useMemo(() => new Map(model.points.map((point, index) => [point.id, index])), [model]);
  const foundIds = useMemo(
    () => found && (found.revision === null || found.revision === model.revision) ? new Set(found.ids) : null,
    [found, model.revision]
  );
  const visible = useMemo(() => {
    const groupHidden = hidden.size > 0;
    return (index) => {
      const point = model.points[index];
      if (groupHidden && hidden.has(point.group)) return false;
      return !searched || searched.has(point.id);
    };
  }, [model, hidden, searched]);
  const matches = useMemo(() => {
    const ids = searched ?? foundIds;
    return ids ? [...ids].filter((id) => indexOf.has(id) && visible(indexOf.get(id))) : [];
  }, [searched, foundIds, indexOf, visible]);
  useEffect4(() => {
    const box = boxRef.current;
    if (!box) return;
    const observer = new ResizeObserver((entries) => {
      const rect = entries[0]?.contentRect;
      if (rect) setSize({ width: Math.floor(rect.width), height: Math.floor(rect.height) });
    });
    observer.observe(box);
    return () => observer.disconnect();
  }, [hasPoints]);
  useEffect4(() => {
    if (size.width > 0 && size.height > 0) setView((current) => current ?? fitView(size.width, size.height));
  }, [size.width, size.height]);
  useEffect4(() => setView(size.width > 0 ? fitView(size.width, size.height) : null), [model.revision]);
  useEffect4(() => {
    if (!foundIds || size.width === 0) return;
    const framed = frame(model, foundIds, size.width, size.height);
    if (framed) setView(framed);
  }, [foundIds, model, size.width, size.height]);
  useEffect4(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const wheel = (event) => {
      event.preventDefault();
      const rect = canvas.getBoundingClientRect();
      const factor = Math.exp(-event.deltaY * (event.ctrlKey ? 0.01 : 25e-4));
      setView((current) => current ? zoomAt(current, factor, event.clientX - rect.left, event.clientY - rect.top) : current);
    };
    canvas.addEventListener("wheel", wheel, { passive: false });
    return () => canvas.removeEventListener("wheel", wheel);
  }, [hasPoints]);
  useEffect4(() => {
    const canvas = canvasRef.current;
    if (!canvas || !view || size.width === 0) return;
    const ratio = window.devicePixelRatio || 1;
    canvas.width = Math.round(size.width * ratio);
    canvas.height = Math.round(size.height * ratio);
    const context = canvas.getContext("2d");
    if (!context) return;
    const colors = resolve(canvas);
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, size.width, size.height);
    const radius = Math.min(5, 2.2 + Math.log2(view.scale) * 0.8);
    const groupColor2 = new Map(model.groups.map((group) => [group.key, group.color]));
    for (const pass of [false, true]) {
      context.globalAlpha = pass ? 0.92 : 0.16;
      model.points.forEach((point, index) => {
        if (visible(index) !== pass) return;
        const [x, y] = toScreen(view, mapSize, point.x, point.y);
        if (x < -8 || y < -8 || x > size.width + 8 || y > size.height + 8) return;
        context.fillStyle = groupColor2.get(point.group) ?? colors.faint;
        context.beginPath();
        context.arc(x, y, radius, 0, Math.PI * 2);
        context.fill();
      });
    }
    context.globalAlpha = 1;
    const ring = (id, color, extra, width) => {
      const index = indexOf.get(id);
      if (index === void 0) return;
      const [x, y] = toScreen(view, mapSize, model.points[index].x, model.points[index].y);
      context.strokeStyle = color;
      context.lineWidth = width;
      context.beginPath();
      context.arc(x, y, radius + extra, 0, Math.PI * 2);
      context.stroke();
    };
    foundIds?.forEach((id) => ring(id, colors.accent, 3, 1.5));
    if (playing) ring(playing, colors.accent, 6, 2);
    if (selected) ring(selected, colors.strong, 4, 2);
  }, [model, indexOf, view, size, visible, foundIds, selected, playing, mapSize]);
  const at = (event) => {
    const rect = canvasRef.current?.getBoundingClientRect();
    return rect ? [event.clientX - rect.left, event.clientY - rect.top] : [0, 0];
  };
  if (!model.points.length) {
    return /* @__PURE__ */ jsx9("div", { style: { ...S.muted, padding: 16 }, children: empty });
  }
  const chosen = selected ? model.sounds[selected] : void 0;
  const hovered = hover ? model.points[hover.index] : void 0;
  const hoveredSound = hovered ? model.sounds[hovered.id] : void 0;
  const shown = model.points.reduce((count2, _, index) => count2 + (visible(index) ? 1 : 0), 0);
  return /* @__PURE__ */ jsxs9("div", { style: { display: "flex", flexDirection: "column", height: "100%", minHeight: 240, color: T.text }, children: [
    /* @__PURE__ */ jsxs9("div", { style: { ...S.row, flexWrap: "wrap", padding: "8px 10px", gap: 6, borderBottom: `1px solid ${T.border}` }, children: [
      /* @__PURE__ */ jsx9(
        "input",
        {
          "aria-label": t("map.search"),
          onChange: (event) => setQuery(event.target.value),
          placeholder: t("map.searchHint"),
          style: { flex: "1 1 180px", minWidth: 140, font: "inherit", fontSize: 12, padding: "3px 8px", borderRadius: 3, border: `1px solid ${T.border}`, background: "transparent", color: T.text },
          value: query
        }
      ),
      /* @__PURE__ */ jsxs9("span", { style: S.faint, children: [
        shown === model.points.length ? t("map.count", num(model.points.length, 0)) : t("map.countOf", num(shown, 0), num(model.points.length, 0)),
        model.revision ? ` · ${t("map.revision", model.revision)}` : ""
      ] })
    ] }),
    /* @__PURE__ */ jsx9("div", { style: { ...S.row, flexWrap: "wrap", gap: 4, padding: "6px 10px" }, children: model.groups.map((group) => {
      const off = hidden.has(group.key);
      return /* @__PURE__ */ jsxs9(
        "button",
        {
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
            /* @__PURE__ */ jsx9("span", { style: { width: 8, height: 8, borderRadius: "50%", background: group.color, opacity: off ? 0.3 : 1 } }),
            group.name,
            " ",
            /* @__PURE__ */ jsx9("span", { style: { color: T.faint }, children: group.count })
          ]
        },
        group.key
      );
    }) }),
    matches.length > 0 && /* @__PURE__ */ jsxs9("div", { "aria-label": searched ? t("map.matches") : t("map.found"), role: "group", style: { display: "flex", flexDirection: "column", gap: 1, padding: "0 10px 6px" }, children: [
      matches.slice(0, MATCHES_LISTED).map((id) => {
        const sound = model.sounds[id];
        return /* @__PURE__ */ jsxs9(
          "button",
          {
            "aria-pressed": selected === id,
            onClick: () => onPick(id),
            type: "button",
            style: { ...S.row, gap: 6, font: "inherit", fontSize: 11.5, padding: "1px 6px", borderRadius: 3, border: "none", textAlign: "left", cursor: "pointer", color: T.text, background: selected === id ? T.card : "transparent" },
            children: [
              /* @__PURE__ */ jsx9("span", { style: { ...S.ellipsis, flex: 1 }, children: clean(sound?.name ?? id, 80) }),
              sound?.class && /* @__PURE__ */ jsx9("span", { style: { color: T.faint }, children: clean(sound.class, 16) })
            ]
          },
          id
        );
      }),
      matches.length > MATCHES_LISTED && /* @__PURE__ */ jsx9("span", { style: { ...S.faint, padding: "0 6px" }, children: t("map.more", matches.length - MATCHES_LISTED) })
    ] }),
    /* @__PURE__ */ jsxs9("div", { ref: boxRef, style: { position: "relative", flex: 1, minHeight: 160, overflow: "hidden" }, children: [
      /* @__PURE__ */ jsx9(
        "canvas",
        {
          ref: canvasRef,
          "aria-label": t("map.canvas"),
          onPointerDown: (event) => {
            if (!view) return;
            event.currentTarget.setPointerCapture?.(event.pointerId);
            const [x, y] = at(event);
            drag.current = { x, y, moved: false, view };
          },
          onPointerMove: (event) => {
            const [x, y] = at(event);
            const held = drag.current;
            if (held) {
              if (Math.abs(x - held.x) + Math.abs(y - held.y) > 3) held.moved = true;
              if (held.moved) setView({ ...held.view, dx: held.view.dx + x - held.x, dy: held.view.dy + y - held.y });
              return;
            }
            if (!view) return;
            const index = pick(model, view, mapSize, x, y, 8, visible);
            setHover(index >= 0 ? { index, x, y } : null);
          },
          onPointerUp: (event) => {
            const held = drag.current;
            drag.current = null;
            if (!held || held.moved || !view) return;
            const [x, y] = at(event);
            const index = pick(model, view, mapSize, x, y, 10, visible);
            if (index >= 0) onPick(model.points[index].id);
          },
          onPointerLeave: () => setHover(null),
          onDoubleClick: () => setView(fitView(size.width, size.height)),
          style: { position: "absolute", inset: 0, width: "100%", height: "100%", cursor: hover ? "pointer" : drag.current?.moved ? "grabbing" : "grab", touchAction: "none" }
        }
      ),
      hovered && hoveredSound && hover && /* @__PURE__ */ jsxs9(
        "div",
        {
          style: { position: "absolute", left: Math.min(hover.x + 12, Math.max(0, size.width - 220)), top: Math.max(0, hover.y - 34), maxWidth: 220, pointerEvents: "none", padding: "3px 7px", borderRadius: 3, background: T.card, border: `1px solid ${T.border}`, fontSize: 11.5, color: T.text, ...S.ellipsis },
          children: [
            clean(hoveredSound.name, 48),
            " ",
            /* @__PURE__ */ jsxs9("span", { style: { color: T.muted }, children: [
              "· ",
              clean(hoveredSound.class ?? "", 16)
            ] })
          ]
        }
      )
    ] }),
    selected && chosen && /* @__PURE__ */ jsxs9("div", { style: { ...S.row, justifyContent: "space-between", gap: 8, padding: "8px 10px", borderTop: `1px solid ${T.border}` }, children: [
      /* @__PURE__ */ jsxs9("div", { style: { display: "flex", flexDirection: "column", minWidth: 0 }, children: [
        /* @__PURE__ */ jsx9("span", { style: { ...S.text, ...S.ellipsis, fontWeight: 600 }, children: clean(chosen.name, 80) }),
        /* @__PURE__ */ jsxs9("span", { style: { ...S.row, gap: 4, marginTop: 2 }, children: [
          chosen.class && /* @__PURE__ */ jsx9(Chip, { children: clean(chosen.class, 16) }),
          chosen.kind && /* @__PURE__ */ jsx9(Chip, { children: clean(chosen.kind, 16) }),
          typeof chosen.duration === "number" && /* @__PURE__ */ jsx9(Chip, { children: seconds(chosen.duration) }),
          chosen.folder && /* @__PURE__ */ jsx9("span", { style: { ...S.faint, ...S.ellipsis }, children: clean(chosen.folder, 60) })
        ] })
      ] }),
      /* @__PURE__ */ jsx9("div", { style: { ...S.row, gap: 6 }, children: actions?.(selected, chosen) })
    ] })
  ] });
}

// src/desktop/map.tsx
import { Fragment as Fragment2, jsx as jsx10, jsxs as jsxs10 } from "react/jsx-runtime";
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
function MapWorkspace({ ctx, store, audition }) {
  const t = useT();
  useWant(store, "map");
  const state = useKumi(store);
  const playing = usePlaying(audition);
  const [selected, setSelected] = useState4(null);
  const play = (id) => {
    audition.play(id).catch((error) => host4.notify({ kind: "warning", message: t("map.playFailed", errorText(error)) }));
  };
  const map = state.map;
  const empty = state.available === false ? t("map.agentOff") : map === null ? t("map.reading") : t("map.empty");
  return /* @__PURE__ */ jsx10(
    MapCanvas,
    {
      actions: (id, sound) => {
        const request = soundRequest(id, sound.name, map?.revision);
        return /* @__PURE__ */ jsxs10(Fragment2, { children: [
          /* @__PURE__ */ jsx10(TextButton, { onClick: () => playing === id ? audition.stop() : play(id), title: t("map.playTitle"), children: playing === id ? t("map.stop") : t("map.play") }),
          request && /* @__PURE__ */ jsx10(
            TextButton,
            {
              onClick: () => void ask(ctx, request, { send: false, session: ownerSession(state) }),
              strong: true,
              title: t("map.useTitle"),
              children: t("map.use")
            }
          )
        ] });
      },
      empty,
      found: state.found,
      map,
      onPick: (id) => {
        setSelected(id);
        play(id);
      },
      playing,
      selected
    }
  );
}

// src/desktop/measurements.tsx
import { Fragment as Fragment3, jsx as jsx11, jsxs as jsxs11 } from "react/jsx-runtime";
function MeasurementsWorkspace({ store, audition }) {
  const t = useT();
  const { measured } = useKumi(store);
  const note = (text) => /* @__PURE__ */ jsx11("span", { style: S.muted, children: text });
  return /* @__PURE__ */ jsx11("div", { style: { display: "flex", flexDirection: "column", gap: 10, padding: 16, color: T.text, overflowY: "auto", height: "100%" }, children: measured === null ? note(t("measurements.nothing")) : measured.result === null ? note(t("measurements.tooMuch")) : /* @__PURE__ */ jsxs11(Fragment3, { children: [
    /* @__PURE__ */ jsx11("span", { style: S.faint, children: t("measurements.measured", ago(measured.at, Date.now(), t)) }),
    /* @__PURE__ */ jsx11(ListenResultCard, { audition, fallback: note(t("measurements.inReply")), result: measured.result })
  ] }) });
}

// src/desktop/settings.tsx
import { host as host5 } from "@hermes/plugin-sdk";
import { useState as useState5 } from "react";
import { Fragment as Fragment4, jsx as jsx12, jsxs as jsxs12 } from "react/jsx-runtime";
var OPEN_MAP_ON_FIND = "openMapOnFind";
var STATES2 = /* @__PURE__ */ new Set(["ready", "needs_runtime", "outdated_runtime", "damaged_runtime", "needs_daw_component", "no_pin", "unsupported_platform", "unknown"]);
var COMPONENT_STATES = /* @__PURE__ */ new Set(["not_installed", "installed", "outdated", "unknown"]);
function Line({ label, children }) {
  return /* @__PURE__ */ jsxs12("div", { style: { display: "grid", gridTemplateColumns: "9rem 1fr", gap: 8, alignItems: "baseline", padding: "5px 0", borderTop: `1px solid ${T.border}` }, children: [
    /* @__PURE__ */ jsx12("span", { style: S.muted, children: label }),
    /* @__PURE__ */ jsx12("span", { style: { ...S.text, minWidth: 0 }, children })
  ] });
}
function megabytes(bytes) {
  return typeof bytes === "number" && bytes > 0 ? `${(bytes / 1048576).toFixed(0)} MB` : null;
}
function SettingsPage({ ctx, store, onOpenMap }) {
  const t = useT();
  useWant(store, "setup");
  const state = useKumi(store);
  const [openOnFind, setOpenOnFind] = useState5(() => ctx.storage.get(OPEN_MAP_ON_FIND, true));
  const setup = state.setup;
  const step = nextStep(state, t);
  const daw = state.status?.daw ?? setup?.daw;
  const runtime = setup?.runtime;
  const component = setup?.daw_component;
  const setupState = setup?.state ?? "unknown";
  const componentState = component?.state ?? "unknown";
  return /* @__PURE__ */ jsxs12("div", { style: { display: "flex", flexDirection: "column", gap: 18, maxWidth: 640, color: T.text }, children: [
    /* @__PURE__ */ jsxs12("div", { style: { display: "flex", flexDirection: "column", gap: 4 }, children: [
      /* @__PURE__ */ jsx12("span", { style: { fontSize: 15, fontWeight: 600 }, children: "Kumi" }),
      /* @__PURE__ */ jsx12("span", { style: S.muted, children: t("settings.intro", knownDaw(daw) ?? "") })
    ] }),
    state.available === false ? /* @__PURE__ */ jsx12("span", { style: S.text, children: t("settings.agentOff") }) : /* @__PURE__ */ jsxs12(Fragment4, { children: [
      /* @__PURE__ */ jsxs12("div", { children: [
        /* @__PURE__ */ jsx12("span", { style: { ...S.heading, display: "block", marginBottom: 4 }, children: t("settings.setup") }),
        /* @__PURE__ */ jsx12(Line, { label: t("settings.state"), children: STATES2.has(setupState) ? t(`settings.states.${setupState}`) : clean(setupState, 40) }),
        setup?.message && /* @__PURE__ */ jsx12(Line, { label: t("settings.next"), children: clean(setup.message, 300) }),
        runtime && /* @__PURE__ */ jsxs12(Line, { label: t("settings.runtime"), children: [
          runtime.installed ? t("settings.installed", clean(runtime.version ?? "", 20)) : t("settings.notInstalled"),
          setup?.pinned_version && /* @__PURE__ */ jsxs12("span", { style: S.faint, children: [
            " · ",
            t("settings.pinned", clean(setup.pinned_version, 20))
          ] }),
          setup?.target && /* @__PURE__ */ jsxs12("span", { style: S.faint, children: [
            " · ",
            clean(setup.target, 40)
          ] })
        ] }),
        runtime && !runtime.installed && megabytes(setup?.download_bytes) && /* @__PURE__ */ jsxs12(Line, { label: t("settings.download"), children: [
          megabytes(setup?.download_bytes),
          megabytes(setup?.unpacked_bytes) && /* @__PURE__ */ jsxs12("span", { style: S.faint, children: [
            " ",
            t("settings.unpacked", megabytes(setup?.unpacked_bytes))
          ] })
        ] }),
        component && /* @__PURE__ */ jsxs12(Line, { label: t("settings.part", dawName(daw, t)), children: [
          t(
            "settings.componentLine",
            clean(component.name ?? t("settings.dawComponent"), 40),
            COMPONENT_STATES.has(componentState) ? t(`settings.componentStates.${componentState}`) : clean(componentState, 20).replace("_", " ")
          ),
          component.path && /* @__PURE__ */ jsx12("div", { style: { ...S.faint, marginTop: 2 }, children: clean(component.path, 200) })
        ] }),
        setup?.existing_kumi?.path && /* @__PURE__ */ jsxs12(Line, { label: t("settings.standalone"), children: [
          clean(setup.existing_kumi.path, 120),
          /* @__PURE__ */ jsxs12("span", { style: S.faint, children: [
            " · ",
            setup.existing_kumi.adopted ? t("settings.adopted") : t("settings.leftAsIs")
          ] })
        ] }),
        setup?.compressed_audio?.message && /* @__PURE__ */ jsx12(Line, { label: t("settings.audioFormats"), children: /* @__PURE__ */ jsx12("span", { style: setup.compressed_audio.state === "available" ? void 0 : S.muted, children: clean(setup.compressed_audio.message, 400) }) })
      ] }),
      /* @__PURE__ */ jsxs12("div", { children: [
        /* @__PURE__ */ jsx12("span", { style: { ...S.heading, display: "block", marginBottom: 4 }, children: t("settings.connection") }),
        /* @__PURE__ */ jsx12(Line, { label: t("settings.now"), children: (() => {
          const words = chipWords(state.status, state.focus?.transport?.tempo, t);
          return /* @__PURE__ */ jsx12(Chip, { tone: words.tone === "muted" ? void 0 : words.tone, children: words.text });
        })() }),
        state.status?.owner?.session_id && /* @__PURE__ */ jsx12(Line, { label: t("settings.chat"), children: clean(state.status.owner.session_id, 80) })
      ] }),
      /* @__PURE__ */ jsxs12("div", { style: { ...S.row, gap: 8, flexWrap: "wrap" }, children: [
        step && /* @__PURE__ */ jsx12(
          TextButton,
          {
            onClick: () => void ask(ctx, step.request, { send: true, session: state.status?.connected ? ownerSession(state) : null }),
            strong: true,
            children: step.label
          }
        ),
        /* @__PURE__ */ jsx12(TextButton, { onClick: onOpenMap, children: t("settings.openMap") }),
        /* @__PURE__ */ jsx12(TextButton, { onClick: () => host5.navigate("/settings?tab=plugins&agent=kumi"), children: t("settings.agentSettings") })
      ] })
    ] }),
    /* @__PURE__ */ jsxs12("div", { children: [
      /* @__PURE__ */ jsx12("span", { style: { ...S.heading, display: "block", marginBottom: 4 }, children: t("settings.thisWindow") }),
      /* @__PURE__ */ jsxs12("label", { style: { ...S.row, gap: 8, padding: "5px 0", borderTop: `1px solid ${T.border}`, cursor: "pointer" }, children: [
        /* @__PURE__ */ jsx12(
          "input",
          {
            checked: openOnFind,
            onChange: (event) => {
              setOpenOnFind(event.target.checked);
              ctx.storage.set(OPEN_MAP_ON_FIND, event.target.checked);
            },
            type: "checkbox"
          }
        ),
        /* @__PURE__ */ jsx12("span", { style: S.text, children: t("settings.openOnFind") })
      ] })
    ] })
  ] });
}

// src/desktop/plugin.tsx
import { jsx as jsx13 } from "react/jsx-runtime";
var PLUGIN_ID = "kumi";
var MAP_WORKSPACE = "kumi-sound-map";
var MEASUREMENTS_WORKSPACE = "kumi-measurements";
var LISTEN_FALLBACK_MS = 2e3;
var POLL_MS = 5e3;
var JOB_POLL_MS = 2e3;
var usePluginWords = typeof sdk.usePluginI18n === "function" ? sdk.usePluginI18n : null;
function WithT({ children }) {
  const t = usePluginWords ? usePluginWords(PLUGIN_ID) : english;
  return /* @__PURE__ */ jsx13(I18n.Provider, { value: t, children });
}
var plugin = {
  id: PLUGIN_ID,
  name: "Kumi",
  description: "Your DAW beside the chat: FOCUS, HISTORY with undo, a sound map of your sample folders, and Kumi's audio measurements.",
  register(ctx) {
    ctx.i18n?.register(MESSAGES);
    const store = new KumiStore((path) => ctx.rest(path, { timeoutMs: 15e3 }));
    const audition = new Audition((id) => ctx.rest(`/audio?id=${encodeURIComponent(id)}`, { timeoutMs: 6e4 }));
    let closeMap = null;
    let mapPane = null;
    const mapView = () => /* @__PURE__ */ jsx13(WithT, { children: /* @__PURE__ */ jsx13(MapWorkspace, { audition, ctx, store }) });
    const openMap = () => {
      if (typeof host6.openWorkspace === "function") {
        closeMap = host6.openWorkspace(MAP_WORKSPACE, {
          title: wordsOf(ctx)("map.tab"),
          minWidth: "26rem",
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
    let closeMeasurements = null;
    let measurementsPane = null;
    const openMeasurements = () => {
      const render = () => /* @__PURE__ */ jsx13(WithT, { children: /* @__PURE__ */ jsx13(MeasurementsWorkspace, { audition, store }) });
      const title = wordsOf(ctx)("measurements.tab");
      if (typeof host6.openWorkspace === "function") {
        closeMeasurements = host6.openWorkspace(MEASUREMENTS_WORKSPACE, {
          title,
          minWidth: "24rem",
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
    ctx.setInterval(() => {
      if (document.visibilityState !== "hidden") void store.refreshJobs();
    }, JOB_POLL_MS);
    ctx.onEvent(`plugin.${PLUGIN_ID}.job.progress`, ours((payload) => store.trackJob(payload.job_id)));
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
    const showFound = (id, revision, ids) => {
      if (id) {
        if (calls.has(id)) return;
        calls.add(id);
      }
      if (!ids.length) return;
      store.found(revision, ids);
      if (ctx.storage.get(OPEN_MAP_ON_FIND, true)) openMap();
    };
    ctx.onEvent(`plugin.${PLUGIN_ID}.sounds.found`, (event) => {
      const payload = payloadOf(event);
      const id = callId(payload.tool_call_id);
      if (!id || !forChatInFront(payload)) return;
      const ids = Array.isArray(payload.sound_ids) ? payload.sound_ids.filter((value) => typeof value === "string") : [];
      showFound(id, typeof payload.map_revision === "string" ? payload.map_revision : null, ids.slice(0, 200));
    });
    ctx.onEvent(`plugin.${PLUGIN_ID}.listen.done`, (event) => {
      const payload = payloadOf(event);
      const id = callId(payload.tool_call_id);
      if (!id || !forChatInFront(payload) || calls.has(id) || waiting.has(id)) return;
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
    ctx.onEvent("tool.complete", (event) => {
      const payload = payloadOf(event);
      const name = typeof payload.name === "string" ? payload.name : "";
      if (!name.startsWith("kumi_")) return;
      refresh();
      store.trackJob(jobOf(payload.result));
      const id = callId(payload.tool_id);
      if (name === "kumi_listen" && id) {
        calls.add(id);
        waiting.get(id)?.();
        waiting.delete(id);
        return;
      }
      if (name !== "kumi_sounds") return;
      const found = foundSounds(payload.result);
      if (found) showFound(id, found.revision, found.ids);
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
    const contributions = [
      {
        id: "pane",
        area: PANES_AREA,
        title: "Kumi",
        data: { placement: "right", width: "320px" },
        render: () => /* @__PURE__ */ jsx13(WithT, { children: /* @__PURE__ */ jsx13(KumiPane, { ctx, onOpenMap: openMap, store }) })
      },
      {
        id: "status",
        area: STATUSBAR_AREAS.right,
        order: 60,
        render: () => /* @__PURE__ */ jsx13(WithT, { children: /* @__PURE__ */ jsx13(StatusChip, { ctx, onOpenMap: openMap, onOpenSettings: openSettings, store }) })
      }
    ];
    if (typeof sdk.TOOL_MEDIA_RENDERER_AREA === "string") {
      contributions.push({
        id: "listen-card",
        area: sdk.TOOL_MEDIA_RENDERER_AREA,
        data: {
          tools: ["kumi_listen"],
          render: (props) => /* @__PURE__ */ jsx13(WithT, { children: /* @__PURE__ */ jsx13(ListenCard, { audition, media: props }) })
        }
      });
      contributions.push({
        id: "generate-card",
        area: sdk.TOOL_MEDIA_RENDERER_AREA,
        data: {
          tools: ["kumi_job"],
          render: (props) => /* @__PURE__ */ jsx13(WithT, { children: /* @__PURE__ */ jsx13(GenerateCard, { audition, ctx, media: props, store }) })
        }
      });
    }
    if (typeof sdk.COMPOSER_AREAS?.underside === "string") {
      contributions.push({
        id: "focus-hint",
        area: sdk.COMPOSER_AREAS.underside,
        render: () => /* @__PURE__ */ jsx13(WithT, { children: /* @__PURE__ */ jsx13(FocusHint, { store }) })
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
            const last = state.history?.items.find((item) => item.state !== "undone");
            if (!last || !canUndo(state.status, last)) {
              const unsure = Boolean(state.status?.unsure) || last?.state === "unsure" || last?.state === "partial";
              host6.notify({ kind: "info", message: wordsOf(ctx)(unsure ? "notify.undoUnsure" : "notify.nothingToUndo") });
              return;
            }
            void ask(ctx, undoRequest(last), { send: true, session: ownerSession(state) });
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
      render: () => /* @__PURE__ */ jsx13(WithT, { children: /* @__PURE__ */ jsx13(SettingsPage, { ctx, onOpenMap: openMap, store }) })
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
