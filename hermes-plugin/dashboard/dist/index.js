// Built from hermes-plugin/ui/src/ by `npm run build` in hermes-plugin/ui/. Edit ui/src/, not this file.
"use strict";
(() => {
  // src/dashboard/theme.ts
  var T = {
    text: "var(--color-foreground)",
    secondary: "var(--color-card-foreground)",
    muted: "var(--color-muted-foreground)",
    faint: "var(--color-muted-foreground)",
    border: "var(--color-border)",
    card: "var(--color-card)",
    chip: "var(--color-muted)",
    accent: "var(--color-primary)",
    good: "var(--color-success)",
    warn: "var(--color-warning)",
    bad: "var(--color-destructive)",
    focus: "var(--color-ring, var(--color-primary))"
  };
  var CANVAS = {
    text: "--color-card-foreground",
    faint: "--color-muted-foreground",
    border: "--color-border",
    accent: "--color-primary",
    strong: "--color-foreground"
  };

  // src/dashboard/react.ts
  var sdk = window.__HERMES_PLUGIN_SDK__;
  if (!sdk) throw new Error("Kumi: the dashboard plugin SDK is missing");
  var SDK = sdk;
  var React = sdk.React;
  var react_default = React;
  var {
    Fragment,
    createContext,
    createElement,
    useCallback,
    useContext,
    useEffect,
    useMemo,
    useRef,
    useState,
    useSyncExternalStore
  } = React;

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
      roledescription: "sound map",
      keys: "Arrow keys move to the nearest sound that way. Shift with left or right steps through every sound in turn, Shift with up or down zooms, and Home shows the whole map. Tab goes on to Play and Use… for the sound picked.",
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
      cost: (credits, estimate) => `${credits} (est. ${estimate})`,
      reserved: (credits) => `${credits} reserved`,
      unknownCharge: (credits) => `charge not known yet · ${credits} reserved`,
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
      roledescription: "サンプルマップ",
      keys: "矢印キーでその方向の一番近いサウンドへ移動します。Shift と左右キーで全サウンドを順に移動、Shift と上下キーでズーム、Home でマップ全体を表示します。Tab で選んだサウンドの再生と使用に移れます。",
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
      cost: (credits, estimate) => `${credits}（見積 ${estimate}）`,
      reserved: (credits) => `${credits} を確保`,
      unknownCharge: (credits) => `請求額はまだ不明 · ${credits} を確保`,
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
      roledescription: "采样地图",
      keys: "方向键移到该方向最近的声音。Shift 加左右键按顺序经过每个声音，Shift 加上下键缩放，Home 显示整张地图。按 Tab 可到所选声音的播放和使用。",
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
      cost: (credits, estimate) => `${credits}（预估 ${estimate}）`,
      reserved: (credits) => `已预留 ${credits}`,
      unknownCharge: (credits) => `费用尚不清楚 · 已预留 ${credits}`,
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
  var QUOTE_MARKS = /["“”„‟″‶〃＂«»「」『』〝〞〟｢｣﹁﹂﹃﹄]/g;
  function quotable(name, max = 80) {
    return clean(typeof name === "string" ? name.replace(QUOTE_MARKS, " ") : name, max).replace(QUOTE_MARKS, " ").replace(/\s+/g, " ").trim();
  }
  function soundRequest(soundId, name, revision) {
    const id = plainId(soundId);
    if (!id) return null;
    const from = revision ? plainId(revision) : null;
    const said = quotable(name);
    const which = `sound ${id}${from ? ` from map ${from}` : ""}`;
    return said ? `Use “${said}” (${which}) on the selected track` : `Use ${which} on the selected track`;
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

  // src/dashboard/jsx-runtime.ts
  var Fragment2 = react_default.Fragment;
  function jsx(type, props, key) {
    return react_default.createElement(type, key === void 0 ? props : { ...props, key });
  }
  var jsxs = jsx;

  // src/shared/ui.tsx
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
    return /* @__PURE__ */ jsx("style", { children: `.${FOCUSABLE}:focus-visible{outline:2px solid ${T.focus};outline-offset:2px;border-radius:3px}` });
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
        /* @__PURE__ */ jsx("h3", { style: { ...S.row, gap: 6, margin: 0, fontSize: "inherit", fontWeight: "inherit" }, children: title }),
        right
      ] }),
      children
    ] });
  }

  // src/shared/HistoryView.tsx
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
      return /* @__PURE__ */ jsx("span", { style: S.muted, children: t("focus.nothing") });
    }
    const transport = focus.transport;
    const path = focusPath(focus);
    const parameter = focusParameter(focus);
    const busy = nameOf(focus.focus?.busy ?? void 0);
    const meter = nameOf(transport?.meter ?? void 0);
    return /* @__PURE__ */ jsxs("div", { style: { display: "flex", flexDirection: "column", gap: 6, opacity: focus.stale ? 0.6 : 1 }, children: [
      /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 7 }, children: [
        /* @__PURE__ */ jsx(Dot, { color: colorOf(focus.focus?.track) ?? T.faint, size: 9 }),
        /* @__PURE__ */ jsx("span", { style: { ...S.text, ...S.ellipsis, fontWeight: 600 }, children: path.length ? path.join(" › ") : t("focus.nothingSelected") })
      ] }),
      parameter && /* @__PURE__ */ jsx("span", { style: { ...S.muted, ...S.ellipsis }, children: parameter }),
      /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 4, flexWrap: "wrap" }, children: [
        focus.project?.name && /* @__PURE__ */ jsx(Chip, { title: t("focus.project"), children: clean(focus.project.name, 40) }),
        focusPlace(focus).map((place, index) => /* @__PURE__ */ jsx(Chip, { children: place }, index)),
        typeof transport?.playing === "boolean" && /* @__PURE__ */ jsx(Chip, { tone: transport.playing ? "good" : void 0, children: transport.playing ? t("focus.playing") : t("focus.stopped") }),
        bpm(transport?.tempo) && /* @__PURE__ */ jsx(Chip, { children: bpm(transport?.tempo) }),
        meter && /* @__PURE__ */ jsx(Chip, { children: clean(meter, 12) }),
        busy && /* @__PURE__ */ jsx(Chip, { tone: "warn", title: t("focus.busy"), children: clean(busy, 40) }),
        focus.stale && (focus.running === false ? /* @__PURE__ */ jsx(Chip, { children: t("focus.kumiStopped") }) : /* @__PURE__ */ jsx(Chip, { tone: "warn", children: t("focus.notAnswering") }))
      ] }),
      catchUp.lines.length > 0 && /* @__PURE__ */ jsxs("div", { style: { display: "flex", flexDirection: "column", gap: 2 }, children: [
        /* @__PURE__ */ jsx("span", { style: S.faint, children: t("focus.since") }),
        catchUp.lines.map((line, index) => /* @__PURE__ */ jsx("span", { style: { ...S.muted, ...S.ellipsis }, children: line }, index)),
        catchUp.more > 0 && /* @__PURE__ */ jsx("span", { style: S.faint, children: t("focus.more", catchUp.more) })
      ] }),
      focus.stale && typeof focus.heard_at === "number" && /* @__PURE__ */ jsx("span", { style: S.faint, children: t("focus.lastHeard", ago(focus.heard_at, now, t)) })
    ] });
  }
  function HistoryView({ history, now, action }) {
    const t = useT();
    const items = history?.items ?? [];
    if (!items.length) {
      return /* @__PURE__ */ jsx("span", { style: S.muted, children: t("history.empty") });
    }
    return /* @__PURE__ */ jsx("ol", { style: { listStyle: "none", margin: 0, padding: 0, display: "flex", flexDirection: "column", gap: 2 }, children: items.map((item) => {
      const state = stateOf(item, t);
      const track = nameOf(item.track);
      return /* @__PURE__ */ jsxs("li", { style: { ...S.row, gap: 7, padding: "4px 0", borderTop: `1px solid ${T.border}`, opacity: item.state === "undone" ? 0.7 : 1 }, children: [
        /* @__PURE__ */ jsx("span", { "aria-label": state.word, title: state.word, style: { width: 14, textAlign: "center", color: TONE[state.tone], flex: "none" }, children: state.mark }),
        /* @__PURE__ */ jsxs("span", { style: { display: "flex", flexDirection: "column", minWidth: 0, flex: 1 }, children: [
          /* @__PURE__ */ jsx("span", { style: { ...S.text, ...S.ellipsis, textDecoration: item.state === "undone" ? "line-through" : void 0 }, title: clean(item.title ?? "", 300), children: clean(item.title ?? item.label ?? item.id, 120) }),
          /* @__PURE__ */ jsxs("span", { style: { ...S.row, gap: 5 }, children: [
            track && /* @__PURE__ */ jsx(Dot, { color: colorOf(item.track) ?? T.faint, size: 6 }),
            /* @__PURE__ */ jsx("span", { style: { ...S.faint, ...S.ellipsis }, children: [track && clean(track, 24), item.family && clean(item.family, 16), item.label && clean(item.label, 12), ago(item.at, now, t)].filter(Boolean).join(" · ") })
          ] }),
          item.state === "unsure" || item.state === "partial" ? /* @__PURE__ */ jsx("span", { style: { ...S.muted, color: T.warn, whiteSpace: "normal" }, children: [clean(item.reason ?? item.note ?? t("history.unconfirmed"), 200), cleanupWords(item.cleanup, t), t("history.lookFirst")].filter(Boolean).join(" ") }) : (
            // The runtime's own words for any other row: why an undo didn't happen, say.
            item.note && /* @__PURE__ */ jsx("span", { style: { ...S.muted, whiteSpace: "normal" }, children: clean(item.note, 200) })
          )
        ] }),
        action?.(item)
      ] }, item.id);
    }) });
  }

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
    const count = model.points.length;
    let index = from ?? (delta > 0 ? -1 : count);
    for (let tried = 0; tried < count; tried++) {
      index = ((index + delta) % count + count) % count;
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
  var maps = 0;
  function MapCanvas({ map, found, playing, selected, onPick, onSelect, actions, empty }) {
    const t = useT();
    const model = useMemo(() => buildModel(map), [map]);
    const canvasRef = useRef(null);
    const boxRef = useRef(null);
    const [size, setSize] = useState({ width: 0, height: 0 });
    const [view, setView] = useState(null);
    const [query, setQuery] = useState("");
    const [hidden, setHidden] = useState(() => /* @__PURE__ */ new Set());
    const [hover, setHover] = useState(null);
    const drag = useRef(null);
    const [said, setSaid] = useState("");
    const [hintId] = useState(() => `kumi-map-keys-${maps += 1}`);
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
    const [painted, setPainted] = useState("");
    useEffect(() => {
      const canvas = canvasRef.current;
      if (!canvas) return;
      const timer = setInterval(() => {
        const now = Object.values(resolve(canvas)).join();
        setPainted((current) => current === now ? current : now);
      }, 1e3);
      return () => clearInterval(timer);
    }, [hasPoints]);
    useEffect(() => {
      const box = boxRef.current;
      if (!box) return;
      const observer = new ResizeObserver((entries) => {
        const rect = entries[0]?.contentRect;
        if (rect) setSize({ width: Math.floor(rect.width), height: Math.floor(rect.height) });
      });
      observer.observe(box);
      return () => observer.disconnect();
    }, [hasPoints]);
    useEffect(() => {
      if (size.width > 0 && size.height > 0) setView((current) => current ?? fitView(size.width, size.height));
    }, [size.width, size.height]);
    useEffect(() => setView(size.width > 0 ? fitView(size.width, size.height) : null), [model.revision]);
    useEffect(() => {
      if (!foundIds || size.width === 0) return;
      const framed = frame(model, foundIds, size.width, size.height);
      if (framed) setView(framed);
    }, [foundIds, model, size.width, size.height]);
    useEffect(() => {
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
    useEffect(() => {
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
      const count = model.points.length;
      const base = count <= 40 ? 5 : count <= 300 ? 3.2 : 2.2;
      const radius = Math.max(1.5, Math.min(base + 3, base + Math.log2(view.scale) * 0.8));
      const groupColor2 = new Map(model.groups.map((group) => [group.key, group.color]));
      const labelled = [];
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
          if (pass) labelled.push([x, y, model.sounds[point.id]?.name ?? ""]);
        });
      }
      if (labelled.length <= 40) {
        context.globalAlpha = 0.85;
        context.fillStyle = colors.text;
        context.font = `11px ${getComputedStyle(canvas).fontFamily || "sans-serif"}`;
        context.textBaseline = "middle";
        const placed = [];
        for (const [x, y, name] of labelled) {
          const text = mapLabel(name);
          if (!text) continue;
          const box = [x + radius + 4, y - 7, context.measureText(text).width, 14];
          if (placed.some(([left, top, width, height]) => box[0] < left + width && left < box[0] + box[2] && box[1] < top + height && top < box[1] + box[3])) continue;
          placed.push(box);
          context.fillText(text, box[0], y);
        }
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
    }, [model, indexOf, view, size, visible, foundIds, selected, playing, mapSize, painted]);
    const at = (event) => {
      const rect = canvasRef.current?.getBoundingClientRect();
      return rect ? [event.clientX - rect.left, event.clientY - rect.top] : [0, 0];
    };
    const onKey = (event) => {
      const key = mapKey(event);
      if (!view || !key) return;
      event.preventDefault();
      const from = selected === null ? void 0 : indexOf.get(selected);
      const picked = from !== void 0 && visible(from) ? from : void 0;
      const middle = [size.width / 2, size.height / 2];
      if ("fit" in key) {
        setView(fitView(size.width, size.height));
      } else if ("zoom" in key) {
        const [x, y] = picked === void 0 ? middle : toScreen(view, mapSize, model.points[picked].x, model.points[picked].y);
        setView(zoomAt(view, key.zoom, x, y));
      } else {
        const next = moveWith(model, view, mapSize, picked, key, middle, visible);
        if (next < 0) return;
        const point = model.points[next];
        const [x, y] = toScreen(view, mapSize, point.x, point.y);
        setView(keepInView(view, x, y, size.width, size.height));
        onSelect(point.id);
        const sound = model.sounds[point.id];
        setSaid([clean(sound?.name ?? point.id, 80), clean(sound?.class, 16), clean(sound?.folder, 60)].filter(Boolean).join(", "));
      }
    };
    if (!model.points.length) {
      return /* @__PURE__ */ jsx("div", { style: { ...S.muted, padding: 16 }, children: empty });
    }
    const chosen = selected ? model.sounds[selected] : void 0;
    const hovered = hover ? model.points[hover.index] : void 0;
    const hoveredSound = hovered ? model.sounds[hovered.id] : void 0;
    const shown = model.points.reduce((count, _, index) => count + (visible(index) ? 1 : 0), 0);
    return /* @__PURE__ */ jsxs("div", { style: { display: "flex", flexDirection: "column", height: "100%", minHeight: 240, color: T.text }, children: [
      /* @__PURE__ */ jsxs("div", { style: { ...S.row, flexWrap: "wrap", padding: "8px 10px", gap: 6, borderBottom: `1px solid ${T.border}` }, children: [
        /* @__PURE__ */ jsx(
          "input",
          {
            "aria-label": t("map.search"),
            className: FOCUSABLE,
            onChange: (event) => setQuery(event.target.value),
            placeholder: t("map.searchHint"),
            style: { flex: "1 1 180px", minWidth: 140, font: "inherit", fontSize: 12, padding: "3px 8px", borderRadius: 3, border: `1px solid ${T.border}`, background: "transparent", color: T.text },
            value: query
          }
        ),
        /* @__PURE__ */ jsx("span", { style: S.faint, title: model.revision ? t("map.revision", model.revision) : void 0, children: shown === model.points.length ? t("map.count", num(model.points.length, 0)) : t("map.countOf", num(shown, 0), num(model.points.length, 0)) })
      ] }),
      /* @__PURE__ */ jsx("div", { style: { ...S.row, flexWrap: "wrap", gap: 4, padding: "6px 10px" }, children: model.groups.map((group) => {
        const off = hidden.has(group.key);
        return /* @__PURE__ */ jsxs(
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
              /* @__PURE__ */ jsx("span", { style: { width: 8, height: 8, borderRadius: "50%", background: group.color, opacity: off ? 0.3 : 1 } }),
              group.name,
              " ",
              /* @__PURE__ */ jsx("span", { style: { color: T.faint }, children: group.count })
            ]
          },
          group.key
        );
      }) }),
      matches.length > 0 && /* @__PURE__ */ jsxs("div", { "aria-label": searched ? t("map.matches") : t("map.found"), role: "group", style: { display: "flex", flexDirection: "column", gap: 1, padding: "0 10px 6px" }, children: [
        matches.slice(0, MATCHES_LISTED).map((id) => {
          const sound = model.sounds[id];
          return /* @__PURE__ */ jsxs(
            "button",
            {
              "aria-pressed": selected === id,
              className: FOCUSABLE,
              onClick: () => onPick(id),
              type: "button",
              style: { ...S.row, gap: 6, font: "inherit", fontSize: 11.5, padding: "1px 6px", borderRadius: 3, border: "none", textAlign: "left", cursor: "pointer", color: T.text, background: selected === id ? T.card : "transparent" },
              children: [
                /* @__PURE__ */ jsx("span", { style: { ...S.ellipsis, flex: 1 }, children: clean(sound?.name ?? id, 80) }),
                sound?.class && /* @__PURE__ */ jsx("span", { style: { color: T.faint }, children: clean(sound.class, 16) })
              ]
            },
            id
          );
        }),
        matches.length > MATCHES_LISTED && /* @__PURE__ */ jsx("span", { style: { ...S.faint, padding: "0 6px" }, children: t("map.more", matches.length - MATCHES_LISTED) })
      ] }),
      /* @__PURE__ */ jsxs("div", { ref: boxRef, style: { position: "relative", flex: 1, minHeight: 160, overflow: "hidden" }, children: [
        /* @__PURE__ */ jsx(
          "canvas",
          {
            ref: canvasRef,
            "aria-describedby": hintId,
            "aria-label": t("map.canvas"),
            "aria-roledescription": t("map.roledescription"),
            className: FOCUSABLE,
            onKeyDown: onKey,
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
            role: "application",
            tabIndex: 0,
            style: { position: "absolute", inset: 0, width: "100%", height: "100%", outlineOffset: -3, cursor: hover ? "pointer" : drag.current?.moved ? "grabbing" : "grab", touchAction: "none" }
          }
        ),
        /* @__PURE__ */ jsx("span", { id: hintId, style: S.hidden, children: t("map.keys") }),
        /* @__PURE__ */ jsx("span", { "aria-live": "polite", style: S.hidden, children: said }),
        hovered && hoveredSound && hover && /* @__PURE__ */ jsxs(
          "div",
          {
            style: { position: "absolute", left: Math.min(hover.x + 12, Math.max(0, size.width - 220)), top: Math.max(0, hover.y - 34), maxWidth: 220, pointerEvents: "none", padding: "3px 7px", borderRadius: 3, background: T.card, border: `1px solid ${T.border}`, fontSize: 11.5, color: T.text, ...S.ellipsis },
            children: [
              clean(hoveredSound.name, 48),
              " ",
              /* @__PURE__ */ jsxs("span", { style: { color: T.muted }, children: [
                "· ",
                clean(hoveredSound.class ?? "", 16)
              ] })
            ]
          }
        )
      ] }),
      selected && chosen && /* @__PURE__ */ jsxs("div", { style: { ...S.row, justifyContent: "space-between", gap: 8, padding: "8px 10px", borderTop: `1px solid ${T.border}` }, children: [
        /* @__PURE__ */ jsxs("div", { style: { display: "flex", flexDirection: "column", minWidth: 0 }, children: [
          /* @__PURE__ */ jsx("span", { style: { ...S.text, ...S.ellipsis, fontWeight: 600 }, children: clean(chosen.name, 80) }),
          /* @__PURE__ */ jsxs("span", { style: { ...S.row, gap: 4, marginTop: 2 }, children: [
            chosen.class && /* @__PURE__ */ jsx(Chip, { children: clean(chosen.class, 16) }),
            chosen.kind && /* @__PURE__ */ jsx(Chip, { children: clean(chosen.kind, 16) }),
            typeof chosen.duration === "number" && /* @__PURE__ */ jsx(Chip, { children: seconds(chosen.duration) }),
            chosen.folder && /* @__PURE__ */ jsx("span", { style: { ...S.faint, ...S.ellipsis }, children: clean(chosen.folder, 60) })
          ] })
        ] }),
        /* @__PURE__ */ jsx("div", { style: { ...S.row, gap: 6 }, children: actions?.(selected, chosen) })
      ] })
    ] });
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

  // src/dashboard/index.tsx
  var BASE = "/api/plugins/kumi";
  var POLL_MS = 5e3;
  function useNow(every = 3e4) {
    const [now, setNow] = useState(() => Date.now());
    useEffect(() => {
      const timer = setInterval(() => setNow(Date.now()), every);
      return () => clearInterval(timer);
    }, [every]);
    return now;
  }
  function CopyButton({ text, label }) {
    const t = useT();
    const [done, setDone] = useState(false);
    return /* @__PURE__ */ jsx(
      TextButton,
      {
        onClick: () => {
          void navigator.clipboard?.writeText(text).then(() => {
            setDone(true);
            setTimeout(() => setDone(false), 1500);
          });
        },
        title: t("dashboard.copyTitle", text),
        children: done ? t("dashboard.copied") : label
      }
    );
  }
  function KumiDashboard() {
    const locale = SDK.useI18n?.()?.locale;
    const t = useMemo(() => translator(locale), [locale]);
    return /* @__PURE__ */ jsxs(I18n.Provider, { value: t, children: [
      /* @__PURE__ */ jsx(FocusStyle, {}),
      /* @__PURE__ */ jsx(Dashboard, {})
    ] });
  }
  function Dashboard() {
    const t = useT();
    const store = useMemo(() => new KumiStore((path) => SDK.fetchJSON(`${BASE}${path}`)), []);
    const audition = useMemo(() => new Audition((id) => SDK.fetchJSON(`${BASE}/audio?id=${encodeURIComponent(id)}`)), []);
    useWant(store, "map");
    const state = useKumi(store);
    const playing = usePlaying(audition);
    const now = useNow();
    const [selected, setSelected] = useState(null);
    useEffect(() => {
      void store.refreshStatus();
      const timer = setInterval(() => {
        if (document.visibilityState !== "hidden") void store.refreshStatus();
      }, POLL_MS);
      return () => {
        clearInterval(timer);
        audition.dispose();
      };
    }, [store, audition]);
    const words = chipWords(state.status, state.focus?.transport?.tempo, t);
    const tone = { good: T.good, warn: T.warn, bad: T.bad, muted: T.faint }[words.tone];
    const connected = Boolean(state.status?.connected);
    if (state.available === false) {
      return /* @__PURE__ */ jsxs("div", { style: { padding: 24, color: T.text }, children: [
        /* @__PURE__ */ jsx("p", { style: S.text, children: t("dashboard.agentOff") }),
        /* @__PURE__ */ jsx("p", { style: S.muted, children: t("dashboard.agentOffHow") })
      ] });
    }
    return /* @__PURE__ */ jsxs("div", { style: { display: "flex", flexDirection: "column", gap: 12, color: T.text, minHeight: "calc(100vh - 160px)" }, children: [
      /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 10, padding: "4px 2px" }, children: [
        /* @__PURE__ */ jsx(Dot, { color: tone, size: 9 }),
        /* @__PURE__ */ jsx("span", { style: { fontSize: 15, fontWeight: 600 }, children: words.text }),
        state.status?.runtime?.version && /* @__PURE__ */ jsxs("span", { style: S.faint, children: [
          "Kumi ",
          state.status.runtime.version
        ] }),
        state.setup?.message && state.setup.state && state.setup.state !== "ready" && state.setup.state !== "unknown" && !dawPartWorks(state) && /* @__PURE__ */ jsx("span", { style: S.muted, children: state.setup.message })
      ] }),
      state.status?.unsure && /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 10, padding: "0 2px", alignItems: "flex-start" }, children: [
        /* @__PURE__ */ jsx("span", { style: { ...S.muted, color: T.warn, flex: 1 }, children: [
          t("pane.unsure", clean(state.status.unsure.op ?? t("pane.unsureOp"), 40), state.status.unsure.reason ? clean(state.status.unsure.reason, 160) : ""),
          cleanupWords(state.status.unsure.cleanup, t),
          t("dashboard.askLook")
        ].filter(Boolean).join(" ") }),
        /* @__PURE__ */ jsx(CopyButton, { label: t("dashboard.copyRequest"), text: LOOK_AGAIN_REQUEST })
      ] }),
      /* @__PURE__ */ jsxs("div", { style: { display: "grid", gridTemplateColumns: "minmax(260px, 1fr) minmax(320px, 2fr)", gap: 12, alignItems: "stretch" }, children: [
        /* @__PURE__ */ jsxs("div", { style: { border: `1px solid ${T.border}`, borderRadius: 6, background: T.card, overflow: "hidden", alignSelf: "start" }, children: [
          /* @__PURE__ */ jsx(Section, { title: t("pane.focus"), children: /* @__PURE__ */ jsx(FocusView, { focus: state.focus, now }) }),
          /* @__PURE__ */ jsx(Section, { title: t("pane.history"), children: /* @__PURE__ */ jsx(
            HistoryView,
            {
              action: (item) => canUndo(state.status, item) ? /* @__PURE__ */ jsx(CopyButton, { label: t("dashboard.copyUndo"), text: undoRequest(item) }) : null,
              history: state.history,
              now
            }
          ) })
        ] }),
        /* @__PURE__ */ jsx("div", { style: { border: `1px solid ${T.border}`, borderRadius: 6, background: T.card, overflow: "hidden", minHeight: 420, display: "flex", flexDirection: "column" }, children: /* @__PURE__ */ jsx(
          MapCanvas,
          {
            actions: (id, sound) => {
              const request = soundRequest(id, sound.name, state.map?.revision);
              return /* @__PURE__ */ jsxs(Fragment2, { children: [
                /* @__PURE__ */ jsx(TextButton, { onClick: () => playing === id ? audition.stop() : void audition.play(id).catch(() => void 0), children: playing === id ? t("map.stop") : t("map.play") }),
                request && /* @__PURE__ */ jsx(CopyButton, { label: t("dashboard.copyRequest"), text: request })
              ] });
            },
            empty: state.map === null ? t("map.reading") : t("dashboard.mapEmpty"),
            found: state.found,
            map: state.map,
            onPick: (id) => {
              setSelected(id);
              void audition.play(id).catch(() => void 0);
            },
            onSelect: setSelected,
            playing,
            selected
          }
        ) })
      ] })
    ] });
  }
  window.__HERMES_PLUGINS__?.register("kumi", KumiDashboard);
})();
