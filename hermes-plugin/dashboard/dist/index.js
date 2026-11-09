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
    raised: "var(--color-popover, var(--color-card))",
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
      more: (n) => `and ${n} more on the map`,
      play: "▶ Play",
      stop: "■ Stop",
      playTitle: "Play it here (nothing changes in your DAW)",
      use: "Use…",
      useTitle: "Put a request to use this sound in the chat, to finish and send",
      moreLike: "More like this",
      moreLikeTitle: "Put a request for sounds like this one in the chat, to send when you like",
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
      tilesTitle: "Each sound as a square tile in its group's colour, near where the map put it"
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
      lines: (count) => count === 1 ? "1 line" : `${count} lines`,
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
        inpaint: (span, source) => `Replaced ${span} of ${source}`
      },
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
      empty: "Nothing in your palette yet. Pick sounds on the map (drag a box with Shift held, or turn on Select) and press Add to palette.",
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
      clearPicks: "Clear",
      clearPicksTitle: "Pick nothing",
      hover: "Hover to play",
      hoverTitle: "Play each sound as the pointer rests on it",
      select: "Select",
      selectTitle: "Drag a box to pick sounds (or hold Shift while you drag); Command or Ctrl adds to the picks",
      firstOnly: (n) => `The first 10,000 of ${n} picked went in.`,
      pickAll: (n) => n === 1 ? "Pick it" : `Pick all ${n}`,
      pickAllTitle: "Pick every sound this search keeps",
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
      firstOnly: (most, n) => `The first ${most} of ${n} go in.`,
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
      more: (n) => `ほか ${n} 個はマップ上に`,
      play: "▶ 再生",
      stop: "■ 停止",
      playTitle: "ここで再生（DAW は何も変わりません）",
      use: "使う…",
      useTitle: "このサウンドを使う依頼をチャットに入れます。仕上げて送信してください",
      moreLike: "似たサウンド",
      moreLikeTitle: "これに似たサウンドを探す依頼をチャットに入れます。送信はご自身で",
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
      tilesTitle: "各サウンドを、マップ上の位置の近くに、グループの色の四角いタイルとして表示します"
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
      lines: (count) => `${count} 行`,
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
        inpaint: (span, source) => `${source} のうち ${span} を差し替え`
      },
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
      empty: "パレットはまだ空です。マップでサウンドを選び（Shift を押しながらドラッグで囲むか、「選択」をオン）、「パレットに追加」を押してください。",
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
      clearPicks: "解除",
      clearPicksTitle: "選択をすべて解除",
      hover: "ホバーで再生",
      hoverTitle: "ポインタを止めたサウンドを再生します",
      select: "選択",
      selectTitle: "ドラッグで囲んでサウンドを選びます（Shift を押しながらのドラッグでも）。Command か Ctrl で選択に加えます",
      firstOnly: (n) => `選んだ ${n} 個のうち、最初の 10,000 個を入れました。`,
      pickAll: (n) => `${n} 個をすべて選ぶ`,
      pickAllTitle: "この検索で残ったサウンドをすべて選びます",
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
      firstOnly: (most, n) => `${n} 個のうち最初の ${most} 個を使います。`,
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
      more: (n) => `地图上还有 ${n} 个`,
      play: "▶ 播放",
      stop: "■ 停止",
      playTitle: "在这里播放（DAW 不会有任何改动）",
      use: "使用…",
      useTitle: "把使用这个声音的请求放进对话，补充完整后发送",
      moreLike: "更多类似的",
      moreLikeTitle: "把寻找类似声音的请求放进对话，由你决定何时发送",
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
      tilesTitle: "把每个声音显示为它所在分组颜色的方块，放在地图上它原来位置的附近"
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
      lines: (count) => `${count} 行`,
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
        inpaint: (span, source) => `替换了 ${source} 中的 ${span}`
      },
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
      empty: "调色板还是空的。在地图上选择声音（按住 Shift 拖出一个框，或打开“选择”），然后按“加入调色板”。",
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
      clearPicks: "清除",
      clearPicksTitle: "取消全部选择",
      hover: "悬停播放",
      hoverTitle: "指针停在哪个声音上就播放它",
      select: "选择",
      selectTitle: "拖出一个框来选择声音（或按住 Shift 拖动）；按住 Command 或 Ctrl 可追加选择",
      firstOnly: (n) => `所选的 ${n} 个中，前 10,000 个已加入。`,
      pickAll: (n) => `选择全部 ${n} 个`,
      pickAllTitle: "选择这次搜索保留的所有声音",
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
      firstOnly: (most, n) => `${n} 个声音中只用前 ${most} 个。`,
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
    return rows.find((row) => row.state !== "undone");
  }
  function canUndoChange(status, rows) {
    return canUndo(status, undoRowOf(rows));
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

  // src/shared/design.ts
  var MOST_SOURCES = 16;
  var MOST_STACKED = 8;
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
  function mostFor(card) {
    return card.op === "stack" ? MOST_STACKED : MOST_SOURCES;
  }
  function cardNote(card, count) {
    if (count < 2) return { key: "make.needTwo", least: 2 };
    const most = mostFor(card);
    return count > most ? { key: "make.firstOnly", most, of: count } : null;
  }
  function requestFor(card, sources, options = {}, seed) {
    const kept = {};
    for (const key of card.options) {
      const value = options[key];
      if (key === "attack_from") {
        if (typeof value === "number" && Number.isInteger(value) && value >= 0 && value < sources.length) kept.attack_from = value;
      } else if (key !== "split" && typeof value === "number" && Number.isFinite(value)) {
        const range = RANGES[key];
        kept[key] = Math.min(range.max, Math.max(range.min, value));
      }
    }
    return {
      op: card.op,
      ...card.mode ? { mode: card.mode } : {},
      sources: sources.slice(0, mostFor(card)),
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
  var finite = (value) => typeof value === "number" && Number.isFinite(value);
  function fileName(analysis, fallback) {
    const file = typeof analysis?.file === "string" ? analysis.file.split(/[\\/]/).pop() : void 0;
    return clean(file || fallback, 60);
  }
  function displayReferenceName(reference) {
    return clean((reference.origin?.name ?? reference.name ?? "").split(/[\\/]/).pop() || "Reference", 160);
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
     *  (CC-23: an event's `matches` or a result's `items`) and a near search's seed. */
    found(revision, ids, said = {}) {
      const kept = ids.slice(0, 200);
      const near = typeof said.near === "string" && said.near ? said.near : null;
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
  var HOVER_REST_MS = 70;
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
     *  first `preview` seconds, faded out. */
    async play(id, options = {}) {
      this.stop();
      const turn = this.turn;
      this.changed(id);
      try {
        const url = await this.url(id);
        if (turn !== this.turn) return;
        const audio = new Audio(url);
        this.audio = audio;
        this.previewing = options.preview !== void 0;
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
    const timer = useRef(void 0);
    const played = useRef(play);
    played.current = play;
    useEffect(() => () => clearTimeout(timer.current), []);
    useEffect(() => {
      if (!enabled) audition.stopPreview();
    }, [audition, enabled]);
    return useCallback(
      (id) => {
        clearTimeout(timer.current);
        if (!enabled) return;
        if (id === null) {
          audition.stopPreview();
          return;
        }
        timer.current = setTimeout(() => played.current(id), HOVER_REST_MS);
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

  // src/shared/HistoryView.tsx
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
  function undoKeys(target) {
    const on = typeof target === "string" && target ? target : globalThis.navigator?.userAgent ?? "";
    return /apple-darwin|Mac/.test(on) ? "Cmd+Z" : "Ctrl+Z";
  }
  function DawUndo({ daw, how, keys }) {
    const t = useT();
    const app = dawName(daw, t);
    if (how === "buried") {
      return /* @__PURE__ */ jsx("span", { title: t("history.byHandTitle", app), style: { ...S.muted, color: T.warn, whiteSpace: "normal" }, children: t("history.byHand", app) });
    }
    return /* @__PURE__ */ jsx("span", { title: t("history.dawUndoTitle", app, keys), style: { ...S.muted, whiteSpace: "normal" }, children: t("history.dawUndo", app) });
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
      return /* @__PURE__ */ jsx("span", { style: S.muted, children: t("history.empty") });
    }
    return /* @__PURE__ */ jsx("ol", { style: { listStyle: "none", margin: 0, padding: 0, display: "flex", flexDirection: "column", gap: 2 }, children: items.map((item) => {
      const state = stateOf(item, t, live);
      const track = nameOf(item.track);
      const dawUndo = item.state === "applied" && (item.daw_undo === "next" || item.daw_undo === "buried") ? item.daw_undo : null;
      return /* @__PURE__ */ jsxs("li", { style: { ...S.row, gap: 7, padding: "4px 0", borderTop: `1px solid ${T.border}`, opacity: item.state === "undone" ? 0.7 : 1 }, children: [
        /* @__PURE__ */ jsx("span", { "aria-label": state.word, title: state.word, style: { width: 14, textAlign: "center", color: TONE[state.tone], flex: "none" }, children: state.mark }),
        /* @__PURE__ */ jsxs("span", { style: { display: "flex", flexDirection: "column", minWidth: 0, flex: 1 }, children: [
          /* @__PURE__ */ jsx("span", { style: { ...S.text, ...S.ellipsis, textDecoration: item.state === "undone" ? "line-through" : void 0 }, title: clean(item.title ?? "", 300), children: clean(item.title ?? item.label ?? item.id, 120) }),
          /* @__PURE__ */ jsxs("span", { style: { ...S.row, gap: 5 }, children: [
            track && /* @__PURE__ */ jsx(Dot, { color: colorOf(item.track) ?? T.faint, size: 6 }),
            /* @__PURE__ */ jsx("span", { style: { ...S.faint, ...S.ellipsis }, children: [track && clean(track, 24), item.family && clean(item.family, 16), item.label && clean(item.label, 12), ago(item.at, now, t)].filter(Boolean).join(" · ") })
          ] }),
          item.state === "unsure" || item.state === "partial" ? /* @__PURE__ */ jsx("span", { style: { ...S.muted, color: T.warn, whiteSpace: "normal" }, children: [clean(item.reason ?? item.note ?? t("history.unconfirmed"), 200), cleanupWords(item.cleanup, t), t("history.lookFirst")].filter(Boolean).join(" ") }) : (
            // The runtime's own words for any other row (why an undo didn't happen, say), why a script's can't (Kumi
            // never undoes one, CC-19), or why a restart's can't.
            item.note ? /* @__PURE__ */ jsx("span", { style: { ...S.muted, whiteSpace: "normal" }, children: clean(item.note, 400) }) : item.family === "script" ? /* @__PURE__ */ jsx("span", { style: { ...S.muted, whiteSpace: "normal" }, children: t("history.scriptUndo", dawName(daw, t)) }) : restartedSince(item, live) && /* @__PURE__ */ jsx("span", { style: { ...S.muted, whiteSpace: "normal" }, children: t("history.restartedSince", dawName(daw, t)) })
          ),
          item.copy?.file ? /* @__PURE__ */ jsx("span", { style: { ...S.muted, whiteSpace: "normal" }, children: t("history.copyKept", clean(item.copy.file.split(/[/\\]/).pop() ?? "", 120)) }) : null,
          dawUndo && /* @__PURE__ */ jsx(DawUndo, { daw, how: dawUndo, keys: undoKeys(target) }),
          detail?.(item)
        ] }),
        dawUndo ? null : action?.(item)
      ] }, item.id);
    }) });
  }

  // src/shared/MakePanel.tsx
  var GLYPHS = {
    blend: /* @__PURE__ */ jsxs(Fragment2, { children: [
      /* @__PURE__ */ jsx("circle", { cx: "11", cy: "10", r: "6.5", fill: "currentColor", fillOpacity: "0.35" }),
      /* @__PURE__ */ jsx("circle", { cx: "17", cy: "10", r: "6.5", fill: "currentColor", fillOpacity: "0.35" })
    ] }),
    attack: /* @__PURE__ */ jsx("path", { d: "M2 17 L7 17 L8.5 2 L10 12 Q14 16 26 17", fill: "none", stroke: "currentColor", strokeWidth: "1.6", strokeLinejoin: "round" }),
    resonate: /* @__PURE__ */ jsxs(Fragment2, { children: [
      /* @__PURE__ */ jsx("circle", { cx: "9", cy: "10", r: "2", fill: "currentColor" }),
      /* @__PURE__ */ jsx("path", { d: "M13 5 Q16 10 13 15 M17 3 Q21.5 10 17 17 M21 1.5 Q27 10 21 18.5", fill: "none", stroke: "currentColor", strokeWidth: "1.4", strokeLinecap: "round" })
    ] }),
    modulate: /* @__PURE__ */ jsx("path", { d: "M1 10 Q3 2 5 10 T9 10 Q11 5 13 10 T17 10 Q19 8 21 10 T25 10 Q26 9.5 27 10", fill: "none", stroke: "currentColor", strokeWidth: "1.6", strokeLinecap: "round" }),
    texture: /* @__PURE__ */ jsx(Fragment2, { children: [
      [3, 6],
      [8, 13],
      [12, 4],
      [16, 11],
      [20, 6],
      [24, 14],
      [6, 17],
      [18, 17],
      [25, 3]
    ].map(([x, y], index) => /* @__PURE__ */ jsx("circle", { cx: x, cy: y, fill: "currentColor", fillOpacity: 0.45 + index % 3 * 0.2, r: 1.4 + index % 2 * 0.6 }, index)) }),
    loop: /* @__PURE__ */ jsx("path", { d: "M8 6 H19 A4 4 0 0 1 19 14 H9 M11 11 L8 14 L11 17 M8 6 A4 4 0 0 0 6.5 13", fill: "none", stroke: "currentColor", strokeWidth: "1.6", strokeLinecap: "round", strokeLinejoin: "round" }),
    smear: /* @__PURE__ */ jsx("path", { d: "M1 10 Q3 3 5 10 Q7 15 10 10 Q14 6 18 10 Q22 12.5 27 10", fill: "none", stroke: "currentColor", strokeWidth: "1.6", strokeLinecap: "round" }),
    stack: /* @__PURE__ */ jsxs(Fragment2, { children: [
      /* @__PURE__ */ jsx("rect", { fill: "currentColor", fillOpacity: "0.85", height: "3", rx: "1.5", width: "20", x: "4", y: "4" }),
      /* @__PURE__ */ jsx("rect", { fill: "currentColor", fillOpacity: "0.6", height: "3", rx: "1.5", width: "20", x: "4", y: "9" }),
      /* @__PURE__ */ jsx("rect", { fill: "currentColor", fillOpacity: "0.35", height: "3", rx: "1.5", width: "20", x: "4", y: "14" })
    ] })
  };
  function Glyph({ card }) {
    return /* @__PURE__ */ jsx("svg", { "aria-hidden": true, height: "20", style: { flex: "none" }, viewBox: "0 0 28 20", width: "28", children: GLYPHS[card] });
  }
  function flyToDock(from) {
    const dock = document.querySelector("[data-kumi-dock-chips]") ?? document.querySelector("[data-kumi-dock]");
    if (!from || !dock || window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const start = from.getBoundingClientRect();
    const end = dock.getBoundingClientRect();
    const canvas = from.querySelector("canvas");
    const ghost = document.createElement("div");
    ghost.setAttribute("aria-hidden", "true");
    Object.assign(ghost.style, {
      position: "fixed",
      left: `${start.left}px`,
      top: `${start.top}px`,
      width: `${start.width}px`,
      height: `${start.height}px`,
      borderRadius: "8px",
      pointerEvents: "none",
      zIndex: "2147483000",
      background: canvas ? `center / 100% 100% no-repeat url(${canvas.toDataURL()})` : T.accent,
      boxShadow: `0 10px 28px -10px ${T.accent}`
    });
    document.body.appendChild(ghost);
    const dx = end.left + Math.min(64, end.width / 2) - (start.left + start.width / 2);
    const dy = end.top + Math.min(14, end.height / 2) - (start.top + start.height / 2);
    const flight = ghost.animate(
      [
        { transform: "translate(0, 0) scale(1)", opacity: 1 },
        { transform: `translate(${dx * 0.45}px, ${dy * 0.45 - 36}px) scale(0.5)`, opacity: 0.95, offset: 0.5 },
        { transform: `translate(${dx}px, ${dy}px) scale(0.12)`, opacity: 0.3 }
      ],
      { duration: 640, easing: "cubic-bezier(.35,.6,.4,1)" }
    );
    flight.onfinish = () => ghost.remove();
    flight.oncancel = () => ghost.remove();
  }
  function Wave({ peaks, playing, duration }) {
    const canvas = useRef(null);
    const box = useRef(null);
    const [width, setWidth] = useState(0);
    const [drawn, setDrawn] = useState(false);
    const [startedAt, setStartedAt] = useState(null);
    const [now, setNow] = useState(0);
    useEffect(() => {
      const element = box.current;
      if (!element) return;
      const observer = new ResizeObserver((entries) => setWidth(Math.floor(entries[0]?.contentRect.width ?? 0)));
      observer.observe(element);
      return () => observer.disconnect();
    }, []);
    useEffect(() => {
      const frame2 = requestAnimationFrame(() => setDrawn(true));
      return () => cancelAnimationFrame(frame2);
    }, []);
    useEffect(() => setStartedAt(playing ? performance.now() : null), [playing]);
    useEffect(() => {
      if (startedAt === null) return;
      const frame2 = requestAnimationFrame(setNow);
      return () => cancelAnimationFrame(frame2);
    }, [startedAt, now]);
    useEffect(() => {
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
    return /* @__PURE__ */ jsxs("div", { "data-wave": "", ref: box, style: { position: "relative", height: 56, color: T.accent }, children: [
      /* @__PURE__ */ jsx(
        "canvas",
        {
          "aria-hidden": true,
          ref: canvas,
          style: { width: "100%", height: 56, display: "block", clipPath: drawn ? "inset(0 0 0 0)" : "inset(0 100% 0 0)", transition: "clip-path 520ms cubic-bezier(.2,.8,.2,1)" }
        }
      ),
      progress !== null && progress > 0 && progress < 1 && /* @__PURE__ */ jsx("span", { "aria-hidden": true, style: { position: "absolute", top: 0, bottom: 0, left: `${progress * 100}%`, width: 2, borderRadius: 1, background: T.text, opacity: 0.8 } })
    ] });
  }
  function OptionSlider({ name, label, value, onChange }) {
    const t = useT();
    const range = RANGES[name];
    const shown = value ?? (range.min + range.max) / 2;
    return /* @__PURE__ */ jsxs("label", { style: { display: "flex", flexDirection: "column", gap: 2, minWidth: 150, flex: "1 1 150px" }, children: [
      /* @__PURE__ */ jsxs("span", { style: { ...S.row, gap: 6, fontSize: 11.5, color: T.secondary }, children: [
        /* @__PURE__ */ jsx("span", { style: { flex: 1 }, children: value === void 0 ? t(`make.options.${name}`) : label(value) }),
        /* @__PURE__ */ jsx(
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
      /* @__PURE__ */ jsx(
        "input",
        {
          "aria-label": t(`make.options.${name}`),
          className: FOCUSABLE,
          max: range.max,
          min: range.min,
          onChange: (event) => onChange(Number(event.target.value)),
          step: range.step,
          style: { accentColor: T.accent, opacity: value === void 0 ? 0.45 : 1 },
          type: "range",
          value: shown
        }
      )
    ] });
  }
  function MakePanel({ sources, colorOf: colorOf2, tempo, results, onMake, onClose, playing, onPlay, onStop, onKeep, kept, use, inline = false }) {
    const t = useT();
    const [cardKey, setCardKey] = useState("blend");
    const [options, setOptions] = useState(() => typeof tempo === "number" && tempo >= 40 && tempo <= 240 ? { bpm: Math.round(tempo) } : {});
    const [busy, setBusy] = useState(null);
    const [error, setError] = useState(null);
    const requests = useRef(/* @__PURE__ */ new Map());
    const [shown, setShown] = useState({});
    const [fresh, setFresh] = useState(() => /* @__PURE__ */ new Set());
    const [arrived, setArrived] = useState(null);
    const list = useRef(null);
    useEffect(() => {
      if (!arrived) return;
      const take = list.current?.querySelector(`[data-made="${arrived}"]`);
      take?.scrollIntoView({ block: "nearest", behavior: window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth" });
      setArrived(null);
    }, [arrived, results]);
    const [open, setOpen] = useState(false);
    useEffect(() => {
      const frame2 = requestAnimationFrame(() => setOpen(true));
      return () => cancelAnimationFrame(frame2);
    }, []);
    const card = CARDS.find((entry) => entry.key === cardKey) ?? CARDS[0];
    const note = cardNote(card, sources.length);
    const refused = note?.key === "make.needTwo";
    const used = sources.slice(0, mostFor(card));
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
        return /* @__PURE__ */ jsxs("label", { style: { display: "flex", flexDirection: "column", gap: 2, minWidth: 150, flex: "1 1 150px", fontSize: 11.5, color: T.secondary }, children: [
          t("make.options.attack_from"),
          /* @__PURE__ */ jsxs(
            "select",
            {
              className: FOCUSABLE,
              onChange: (event) => setOptions((current) => ({ ...current, attack_from: event.target.value === "" ? void 0 : Number(event.target.value) })),
              style: { font: "inherit", fontSize: 12, padding: "1px 4px", borderRadius: 3, border: `1px solid ${T.border}`, background: "transparent", color: T.text },
              value: options.attack_from ?? "",
              children: [
                /* @__PURE__ */ jsx("option", { value: "", children: t("make.auto") }),
                used.map((source, index) => /* @__PURE__ */ jsx("option", { value: index, children: clean(source.name ?? source.sound_id ?? source.artifact_id ?? "", 40) }, `${index}`))
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
      return /* @__PURE__ */ jsx(OptionSlider, { label: labels[name], name, onChange: (next) => setOptions((current) => ({ ...current, [name]: next })), value: options[name] }, name);
    });
    const group = (op) => /* @__PURE__ */ jsxs("div", { style: { display: "flex", flexDirection: "column", gap: 4 }, children: [
      /* @__PURE__ */ jsxs("span", { style: { ...S.heading, fontSize: 10.5 }, children: [
        t(`make.ops.${op}`),
        " ",
        /* @__PURE__ */ jsx("span", { style: { textTransform: "none", letterSpacing: 0, color: T.faint }, children: t(`make.opHints.${op}`) })
      ] }),
      /* @__PURE__ */ jsx("div", { style: { display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(124px, 1fr))", gap: 6 }, children: CARDS.filter((entry) => entry.op === op).map((entry) => {
        const chosen = entry.key === card.key;
        const blocked = refused;
        return /* @__PURE__ */ jsxs(
          "button",
          {
            "aria-pressed": chosen,
            className: FOCUSABLE,
            disabled: Boolean(blocked),
            onClick: () => setCardKey(entry.key),
            title: t(`make.cards.${entry.key}.hint`),
            type: "button",
            style: {
              display: "flex",
              flexDirection: "column",
              alignItems: "flex-start",
              gap: 3,
              textAlign: "left",
              font: "inherit",
              padding: "7px 8px",
              borderRadius: 8,
              border: `1px solid ${chosen ? T.accent : T.border}`,
              background: chosen ? `color-mix(in srgb, ${T.accent} 13%, transparent)` : "transparent",
              boxShadow: chosen ? `0 0 0 1px ${T.accent}, 0 6px 18px -10px ${T.accent}` : "none",
              color: blocked ? T.faint : T.text,
              cursor: blocked ? "default" : "pointer",
              opacity: blocked ? 0.55 : 1,
              transition: "border-color 140ms, background-color 140ms, box-shadow 200ms, transform 140ms",
              transform: chosen ? "translateY(-1px)" : "none"
            },
            children: [
              /* @__PURE__ */ jsxs("span", { style: { ...S.row, gap: 6, color: chosen ? T.accent : T.secondary }, children: [
                /* @__PURE__ */ jsx(Glyph, { card: entry.key }),
                /* @__PURE__ */ jsx("span", { style: { fontSize: 12.5, fontWeight: 600, color: blocked ? T.faint : T.text }, children: t(`make.cards.${entry.key}.title`) })
              ] }),
              /* @__PURE__ */ jsx("span", { style: { fontSize: 11, lineHeight: 1.35, color: T.muted }, children: t(`make.cards.${entry.key}.hint`) })
            ]
          },
          entry.key
        );
      }) })
    ] }, op);
    return /* @__PURE__ */ jsxs(
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
          /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 8, padding: "10px 12px 6px", borderBottom: `1px solid ${T.border}` }, children: [
            /* @__PURE__ */ jsx("span", { style: { fontSize: 13.5, fontWeight: 650 }, children: t("make.title") }),
            /* @__PURE__ */ jsx("span", { style: S.faint, children: t("make.from", sources.length) }),
            /* @__PURE__ */ jsx("span", { style: { flex: 1 } }),
            /* @__PURE__ */ jsx(TextButton, { label: t("make.close"), onClick: onClose, title: t("make.close"), children: "×" })
          ] }),
          /* @__PURE__ */ jsxs("div", { style: { overflowY: "auto", padding: "8px 12px 12px", display: "flex", flexDirection: "column", gap: 10 }, children: [
            /* @__PURE__ */ jsxs("div", { "aria-label": t("make.sources"), role: "list", style: { ...S.row, flexWrap: "nowrap", gap: 4, overflowX: "auto", paddingBottom: 2, scrollbarWidth: "thin" }, children: [
              used.map((source, index) => /* @__PURE__ */ jsxs("span", { role: "listitem", style: { ...S.row, gap: 4, flex: "none", fontSize: 11, padding: "1px 7px 1px 5px", borderRadius: 10, border: `1px solid ${T.border}`, color: T.secondary, maxWidth: 160 }, children: [
                /* @__PURE__ */ jsx("span", { style: { width: 7, height: 7, borderRadius: "50%", background: colorOf2(source), flex: "none" } }),
                /* @__PURE__ */ jsx("span", { style: S.ellipsis, children: clean(source.name ?? source.sound_id ?? source.artifact_id ?? "", 40) })
              ] }, `${index}-${source.sound_id ?? source.artifact_id}`)),
              sources.length > used.length && /* @__PURE__ */ jsxs("span", { style: { ...S.faint, flex: "none" }, children: [
                "+",
                sources.length - used.length
              ] })
            ] }),
            group("cross"),
            group("fuse"),
            group("stack"),
            card.options.length > 0 && /* @__PURE__ */ jsx("div", { style: { ...S.row, flexWrap: "wrap", gap: 10, alignItems: "flex-end" }, children: optionRow(card) }),
            /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 8, flexWrap: "wrap" }, children: [
              /* @__PURE__ */ jsx(
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
              busy === "make" && /* @__PURE__ */ jsx("span", { "aria-hidden": true, className: "kumi-spin", style: { width: 14, height: 14, borderRadius: "50%", border: `2px solid ${T.accent}`, borderRightColor: "transparent" } }),
              note && /* @__PURE__ */ jsx("span", { style: S.faint, children: note.key === "make.needTwo" ? t(note.key, note.least) : t(note.key, note.most, note.of) }),
              /* @__PURE__ */ jsx("span", { "aria-live": "polite", style: error ? { ...S.faint, color: T.warn, flex: "1 1 14rem" } : S.hidden, children: error ?? "" })
            ] }),
            takes.length > 0 && /* @__PURE__ */ jsx("div", { "aria-label": t("make.results"), ref: list, role: "list", style: { display: "flex", flexDirection: "column", gap: 8 }, children: takes.map((take) => {
              const at = Math.min(take.results.length - 1, shown[take.key] ?? take.results.length - 1);
              const result = take.results[at];
              const isPlaying = playing === result.artifact_id;
              const inPalette = kept.has(result.artifact_id);
              return /* @__PURE__ */ jsxs(
                "div",
                {
                  className: fresh.has(result.artifact_id) ? "kumi-pop" : void 0,
                  "data-made": result.artifact_id,
                  role: "listitem",
                  style: { display: "flex", flexDirection: "column", gap: 4, padding: "8px 10px", borderRadius: 10, border: `1px solid ${isPlaying ? T.accent : T.border}`, background: "transparent" },
                  children: [
                    /* @__PURE__ */ jsx("span", { style: { ...S.text, fontSize: 12, lineHeight: 1.35 }, title: result.recipe, children: clean(result.recipe, 220) || resultName(result) }),
                    /* @__PURE__ */ jsx(Wave, { duration: result.duration, peaks: result.peaks, playing: isPlaying }),
                    /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 6, flexWrap: "wrap" }, children: [
                      /* @__PURE__ */ jsxs("span", { style: S.faint, children: [
                        t(`make.ops.${result.op}`),
                        result.mode ? ` · ${t(`make.cards.${result.mode}.title`)}` : "",
                        " · ",
                        seconds(result.duration)
                      ] }),
                      /* @__PURE__ */ jsxs("span", { style: { ...S.faint, fontVariantNumeric: "tabular-nums" }, title: t("make.seedTitle"), children: [
                        "#",
                        result.seed
                      ] }),
                      take.results.length > 1 && /* @__PURE__ */ jsxs("span", { role: "group", "aria-label": t("make.variations"), style: { ...S.row, gap: 2 }, children: [
                        /* @__PURE__ */ jsx(TextButton, { disabled: at === 0, label: t("make.previous"), onClick: () => setShown((current) => ({ ...current, [take.key]: at - 1 })), children: "‹" }),
                        /* @__PURE__ */ jsx("span", { style: { ...S.faint, fontVariantNumeric: "tabular-nums" }, children: t("make.variation", at + 1, take.results.length) }),
                        /* @__PURE__ */ jsx(TextButton, { disabled: at === take.results.length - 1, label: t("make.next"), onClick: () => setShown((current) => ({ ...current, [take.key]: at + 1 })), children: "›" })
                      ] }),
                      /* @__PURE__ */ jsx("span", { style: { flex: 1 } }),
                      /* @__PURE__ */ jsx(TextButton, { label: isPlaying ? t("make.stop") : t("make.play"), onClick: () => isPlaying ? onStop() : onPlay(result.artifact_id), children: isPlaying ? "■" : "▶" }),
                      /* @__PURE__ */ jsx(TextButton, { disabled: busy !== null, onClick: () => void make(reroll(take.request), take.key), title: t("make.rerollTitle"), children: busy === take.key ? t("make.rerolling") : t("make.reroll") }),
                      /* @__PURE__ */ jsx(
                        TextButton,
                        {
                          disabled: inPalette,
                          onClick: () => {
                            flyToDock(list.current?.querySelector(`[data-made="${result.artifact_id}"] [data-wave]`));
                            void onKeep(result);
                          },
                          pressed: inPalette,
                          title: t("make.keepTitle"),
                          children: inPalette ? t("make.kept") : t("make.keep")
                        }
                      ),
                      use?.(result)
                    ] })
                  ]
                },
                take.key
              );
            }) }),
            takes.length === 0 && results !== null && /* @__PURE__ */ jsx("span", { style: S.faint, children: t("make.none") })
          ] })
        ]
      }
    );
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
  var TILE_ROOM = 1.12;
  function tileLayout(model) {
    const points = model.points;
    const count = points.length;
    const COARSE = 40;
    const covered = new Uint8Array(COARSE * COARSE);
    for (const point of points) covered[Math.min(COARSE - 1, Math.floor(point.y * COARSE)) * COARSE + Math.min(COARSE - 1, Math.floor(point.x * COARSE))] = 1;
    const area = Math.max(0.05, covered.reduce((sum, cell) => sum + cell, 0) / (COARSE * COARSE));
    const side = Math.max(2, Math.ceil(Math.sqrt(Math.max(1, count) * TILE_ROOM / area)));
    const span = side + 1;
    const right = new Int32Array(side * span);
    const left = new Int32Array(side * span);
    for (let r = 0; r < side; r++) {
      for (let c = 0; c < span; c++) {
        right[r * span + c] = c;
        left[r * span + c] = c;
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
    const col = new Int32Array(count);
    const row = new Int32Array(count);
    const cells = new Int32Array(side * side).fill(-1);
    let cx = 0;
    let cy = 0;
    for (const point of points) {
      cx += point.x / Math.max(1, count);
      cy += point.y / Math.max(1, count);
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
          const base = r * span;
          const after = find(right, base, c0);
          if (after < side) consider(r, after);
          const before = find(left, base, c0 + 1) - 1;
          if (before >= 0) consider(r, before);
        }
      }
      col[index] = bestCol;
      row[index] = bestRow;
      cells[bestRow * side + bestCol] = index;
      right[bestRow * span + bestCol] = bestCol + 1;
      left[bestRow * span + bestCol + 1] = bestCol;
    }
    return { side, col, row, cells };
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
    const span = Math.max(x1 - x0, y1 - y0, 0.04);
    const scale = Math.min(40, Math.max(1, (Math.min(width, height) - 2 * margin) / (span * size)));
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
  function MapCanvas({ map, found, playing, selected, onPick, onSelect, actions, empty, leaving, hiddenActions, marked, picked, onPicked, onHover, tools, pickedActions, tiles = false, onTiles, flying }) {
    const t = useT();
    const model = useMemo(() => buildModel(map), [map]);
    const canvasRef = useRef(null);
    const boxRef = useRef(null);
    const [size, setSize] = useState({ width: 0, height: 0 });
    const [view, setView] = useState(null);
    const [query, setQuery] = useState("");
    const [searchFor, setSearchFor] = useState("");
    const [hidden, setHidden] = useState(() => /* @__PURE__ */ new Set());
    const [hover, setHover] = useState(null);
    const drag = useRef(null);
    const [box, setBox] = useState(null);
    const [selecting, setSelecting] = useState(false);
    const rested = useRef(null);
    const rest = (id) => {
      if (rested.current === id) return;
      rested.current = id;
      onHover?.(id);
    };
    const viewRef = useRef(null);
    viewRef.current = view;
    const nextView = useRef(null);
    const frameRef = useRef(0);
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
    useEffect(() => () => cancelAnimationFrame(frameRef.current), []);
    const backing = useRef("");
    const [said, setSaid] = useState("");
    const [hintId] = useState(() => `kumi-map-keys-${maps += 1}`);
    const mapSize = Math.max(1, Math.min(size.width, size.height) - 32);
    const hasPoints = model.points.length > 0;
    useEffect(() => {
      if (query === searchFor) return;
      const timer = setTimeout(() => setSearchFor(query), query ? 150 : 0);
      return () => clearTimeout(timer);
    }, [query, searchFor]);
    const searched = useMemo(() => searchSounds(model, searchFor), [model, searchFor]);
    const indexOf = useMemo(() => new Map(model.points.map((point, index) => [point.id, index])), [model]);
    const foundIds = useMemo(
      () => found && (found.revision === null || found.revision === model.revision) ? new Set(found.ids) : null,
      [found, model.revision]
    );
    const mask = useMemo(() => {
      const out = new Uint8Array(model.points.length);
      model.points.forEach((point, index) => {
        out[index] = leaving?.has(point.id) ? 2 : hidden.size > 0 && hidden.has(point.group) || searched !== null && !searched.has(point.id) ? 0 : 1;
      });
      return out;
    }, [model, hidden, searched, leaving]);
    const visible = useMemo(() => (index) => mask[index] === 1, [mask]);
    const shown = useMemo(() => mask.reduce((count, keep) => count + (keep === 1 ? 1 : 0), 0), [mask]);
    const leavingCount = useMemo(() => mask.reduce((count, keep) => count + (keep === 2 ? 1 : 0), 0), [mask]);
    const still = useMemo(() => typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches, []);
    const leftAt = useRef(/* @__PURE__ */ new Map());
    const introAt = useRef(-Infinity);
    const until = useRef(0);
    const [clock, setClock] = useState(0);
    const animate = (ms) => {
      until.current = Math.max(until.current, performance.now() + ms);
      setClock(performance.now());
    };
    useEffect(() => {
      const now = performance.now();
      for (const id of leaving ?? []) if (!leftAt.current.has(id)) leftAt.current.set(id, now);
      for (const id of [...leftAt.current.keys()]) if (!leaving?.has(id)) leftAt.current.delete(id);
      if (leaving?.size) animate(FADE_MS);
    }, [leaving]);
    useEffect(() => {
      if (still || !model.revision) return;
      introAt.current = performance.now();
      animate(INTRO_MS);
    }, [model.revision]);
    const [morph, setMorph] = useState(() => ({ from: tiles ? 1 : 0, to: tiles ? 1 : 0, at: -Infinity }));
    useEffect(() => {
      const now = performance.now();
      setMorph((current) => current.to === (tiles ? 1 : 0) ? current : { from: still ? tiles ? 1 : 0 : morphAt(current, now), to: tiles ? 1 : 0, at: now });
      if (!still) animate(MORPH_MS);
    }, [tiles]);
    useEffect(() => {
      if (flying && !still) animate(FLY_MS + FLY_STAGGER * FLY_MOST);
    }, [flying]);
    const playingHere = playing !== null && indexOf.has(playing) && !still;
    useEffect(() => {
      if (performance.now() >= until.current && !playingHere) return;
      const frame2 = requestAnimationFrame(setClock);
      return () => cancelAnimationFrame(frame2);
    }, [clock, playingHere]);
    const layout = useMemo(() => tiles || morph.from > 0 ? tileLayout(model) : null, [model, tiles, morph.from > 0]);
    const tiled = useMemo(() => layout ? tileModel(model, layout) : null, [model, layout]);
    const active = tiles && tiled ? tiled : model;
    const spread = useMemo(() => {
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
    const matches = useMemo(() => {
      const ids = searched ?? foundIds;
      return ids ? [...ids].filter((id) => indexOf.has(id) && visible(indexOf.get(id))) : [];
    }, [searched, foundIds, indexOf, visible]);
    const pickedIds = useMemo(() => picked && picked.size > 0 ? model.points.filter((point) => picked.has(point.id)).map((point) => point.id) : [], [model, picked]);
    const nearName = !searched && foundIds && found?.near && model.sounds[found.near] ? clean(model.sounds[found.near].name ?? "", 60) : "";
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
      const box2 = boxRef.current;
      if (!box2) return;
      const observer = new ResizeObserver((entries) => {
        const rect = entries[0]?.contentRect;
        if (rect) setSize({ width: Math.floor(rect.width), height: Math.floor(rect.height) });
      });
      observer.observe(box2);
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
        const base = nextView.current ?? viewRef.current;
        if (base) queueView(zoomAt(base, factor, event.clientX - rect.left, event.clientY - rect.top));
      };
      canvas.addEventListener("wheel", wheel, { passive: false });
      return () => canvas.removeEventListener("wheel", wheel);
    }, [hasPoints]);
    const hoverAt = hover ? hover.index : null;
    useEffect(() => {
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
      const count = points.length;
      const base = count <= 40 ? 5 : count <= 300 ? 3.2 : 2.2;
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
        for (let index = 0; index < count; index++) {
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
      if (hovered2 !== null && hovered2 < count && mask[hovered2] === 1) {
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
          context.arc(x, y, Math.max(1.2, (radius + 1.5) * (1 - 0.55 * progress)), 0, Math.PI * 2);
          context.fill();
        });
        context.globalAlpha = 1;
      }
    }, [model, indexOf, view, size, mask, foundIds, selected, playing, mapSize, painted, clock, marked, picked, tiled, layout, morph, spread, flying, still, hoverAt]);
    const at = (event) => {
      const rect = canvasRef.current?.getBoundingClientRect();
      return rect ? [event.clientX - rect.left, event.clientY - rect.top] : [0, 0];
    };
    const reachPx = (base) => tiles && layout && view ? Math.max(base, mapSize * view.scale / layout.side * 0.62) : base;
    const press = (event) => {
      if (!view) return;
      event.currentTarget.setPointerCapture?.(event.pointerId);
      const [x, y] = at(event);
      const boxing = Boolean(onPicked) && (event.shiftKey || selecting);
      drag.current = { x, y, moved: false, view, box: boxing, add: event.metaKey || event.ctrlKey };
      rest(null);
      setHover(null);
      if (boxing) setBox([x, y, x, y]);
    };
    const release = (event) => {
      const held = drag.current;
      drag.current = null;
      setBox(null);
      if (!held || !view) return;
      const [x, y] = at(event);
      if (held.box && held.moved) {
        const inside = pointsInBox(active, view, mapSize, [held.x, held.y, x, y], visible).map((index2) => model.points[index2].id);
        onPicked?.(inside, held.add ? "add" : "replace");
        return;
      }
      if (held.moved) return;
      const index = pick(active, view, mapSize, x, y, reachPx(10), visible);
      if (index < 0) return;
      if (onPicked && (event.metaKey || event.ctrlKey)) onPicked([model.points[index].id], "toggle");
      else onPick(model.points[index].id);
    };
    const onKey = (event) => {
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
      return /* @__PURE__ */ jsxs("div", { style: { ...S.muted, padding: 16 }, children: [
        empty,
        (map?.hidden ?? 0) > 0 && /* @__PURE__ */ jsxs("div", { role: "status", style: { ...S.row, flexWrap: "wrap", gap: 6, marginTop: 8 }, children: [
          /* @__PURE__ */ jsx("span", { style: { ...S.faint, flex: "1 1 14rem" }, children: t("map.hidden", map?.hidden ?? 0) }),
          hiddenActions
        ] })
      ] });
    }
    const chosen = selected ? model.sounds[selected] : void 0;
    const hovered = hover ? model.points[hover.index] : void 0;
    const hoveredSound = hovered ? model.sounds[hovered.id] : void 0;
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
        /* @__PURE__ */ jsx("span", { style: S.faint, title: model.revision ? t("map.revision", model.revision) : void 0, children: shown === model.points.length - leavingCount ? t("map.count", num(shown, 0)) : t("map.countOf", num(shown, 0), num(model.points.length - leavingCount, 0)) }),
        onPicked && searched !== null && matches.length > 0 && // Native's `a`: every sound the search keeps.
        /* @__PURE__ */ jsx(TextButton, { onClick: () => onPicked(matches, "replace"), title: t("dock.pickAllTitle"), children: t("dock.pickAll", matches.length) }),
        onPicked && /* @__PURE__ */ jsx(TextButton, { onClick: () => setSelecting(!selecting), pressed: selecting, title: t("dock.selectTitle"), children: t("dock.select") }),
        onTiles && /* @__PURE__ */ jsx(TextButton, { onClick: () => onTiles(!tiles), pressed: tiles, title: t("map.tilesTitle"), children: t("map.tiles") }),
        tools
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
      matches.length > 0 && /* @__PURE__ */ jsxs("div", { "aria-label": searched ? t("map.matches") : nearName ? t("map.near", nearName) : t("map.found"), role: "group", style: { display: "flex", flexDirection: "column", gap: 1, padding: "0 10px 6px" }, children: [
        nearName && /* @__PURE__ */ jsx("span", { style: { ...S.muted, padding: "0 6px" }, children: t("map.near", nearName) }),
        matches.slice(0, MATCHES_LISTED).map((id) => {
          const sound = model.sounds[id];
          const indexed = indexedValues(sound, t);
          const said2 = searched ? "" : foundLine(found?.matches?.[id], nearName !== "", t);
          return /* @__PURE__ */ jsxs(
            "button",
            {
              "aria-pressed": selected === id,
              className: FOCUSABLE,
              onClick: () => onPick(id),
              type: "button",
              style: { display: "flex", flexDirection: "column", alignItems: "stretch", gap: 0, font: "inherit", fontSize: 11.5, padding: "1px 6px", borderRadius: 3, border: "none", textAlign: "left", cursor: "pointer", color: T.text, background: selected === id ? T.card : "transparent" },
              children: [
                /* @__PURE__ */ jsxs("span", { style: { ...S.row, gap: 6 }, children: [
                  /* @__PURE__ */ jsx("span", { style: { ...S.ellipsis, flex: 1 }, children: clean(sound?.name ?? id, 80) }),
                  indexed && /* @__PURE__ */ jsx("span", { style: { color: T.faint, whiteSpace: "nowrap" }, title: t("map.indexedTitle"), children: t("map.indexed", indexed) }),
                  sound?.class && /* @__PURE__ */ jsx("span", { style: { color: T.faint }, children: clean(sound.class, 16) })
                ] }),
                said2 && /* @__PURE__ */ jsx("span", { style: { ...S.ellipsis, color: T.faint, fontSize: 10.5 }, title: nearName ? t("map.closenessTitle") : void 0, children: said2 })
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
            },
            onPointerLeave: () => {
              setHover(null);
              rest(null);
            },
            onDoubleClick: () => setView(fitView(size.width, size.height)),
            role: "application",
            tabIndex: 0,
            style: { position: "absolute", inset: 0, width: "100%", height: "100%", outlineOffset: -3, cursor: selecting || box ? "crosshair" : hover ? "pointer" : drag.current?.moved ? "grabbing" : "grab", touchAction: "none" }
          }
        ),
        box && Math.abs(box[2] - box[0]) + Math.abs(box[3] - box[1]) > 3 && // The box being drawn, its edge marching (`kumi-ants`, in FocusStyle).
        /* @__PURE__ */ jsx(
          "svg",
          {
            "aria-hidden": true,
            height: Math.abs(box[3] - box[1]),
            style: { position: "absolute", left: Math.min(box[0], box[2]), top: Math.min(box[1], box[3]), overflow: "visible", pointerEvents: "none" },
            width: Math.abs(box[2] - box[0]),
            children: /* @__PURE__ */ jsx(
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
      pickedIds.length > 0 && /* @__PURE__ */ jsxs("div", { role: "group", "aria-label": t("dock.picked", pickedIds.length), style: { ...S.row, flexWrap: "wrap", gap: 6, padding: "6px 10px", borderTop: `1px solid ${T.border}` }, children: [
        /* @__PURE__ */ jsx("span", { style: { ...S.text, fontWeight: 600, flex: "1 1 8rem" }, children: t("dock.picked", pickedIds.length) }),
        pickedActions?.(pickedIds)
      ] }),
      (map?.hidden ?? 0) + leavingCount > 0 && // One quiet line for every sound that couldn't play here, never a notice per sound.
      /* @__PURE__ */ jsxs("div", { role: "status", style: { ...S.row, flexWrap: "wrap", gap: 6, padding: "4px 10px", borderTop: `1px solid ${T.border}` }, children: [
        /* @__PURE__ */ jsx("span", { style: { ...S.faint, flex: "1 1 14rem" }, children: t("map.hidden", (map?.hidden ?? 0) + leavingCount) }),
        hiddenActions
      ] }),
      selected && chosen && // Beside the chat the map can be narrow: the name and the actions wrap rather than push actions out of view.
      /* @__PURE__ */ jsxs("div", { style: { ...S.row, justifyContent: "space-between", flexWrap: "wrap", gap: 8, padding: "8px 10px", borderTop: `1px solid ${T.border}` }, children: [
        /* @__PURE__ */ jsxs("div", { style: { display: "flex", flexDirection: "column", minWidth: 0 }, children: [
          /* @__PURE__ */ jsx("span", { style: { ...S.text, ...S.ellipsis, fontWeight: 600 }, children: clean(chosen.name, 80) }),
          /* @__PURE__ */ jsxs("span", { style: { ...S.row, gap: 4, marginTop: 2 }, children: [
            chosen.class && /* @__PURE__ */ jsx(Chip, { children: clean(chosen.class, 16) }),
            chosen.kind && /* @__PURE__ */ jsx(Chip, { children: clean(chosen.kind, 16) }),
            typeof chosen.duration === "number" && /* @__PURE__ */ jsx(Chip, { children: seconds(chosen.duration) }),
            chosen.folder && /* @__PURE__ */ jsx("span", { style: { ...S.faint, ...S.ellipsis }, children: clean(chosen.folder, 60) })
          ] })
        ] }),
        /* @__PURE__ */ jsx("div", { style: { ...S.row, gap: 6, flexWrap: "wrap", justifyContent: "flex-end", marginLeft: "auto" }, children: actions?.(selected, chosen) })
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
  function peaksOf(samples, bins) {
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
      return peaksOf(mixed, BARS);
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
    const canvas = useRef(null);
    const [peaks, setPeaks] = useState(() => id ? peaksKept.get(id) ?? null : null);
    useEffect(() => {
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
    useEffect(() => {
      const element = canvas.current;
      const context = element?.getContext("2d");
      if (!element || !context) return;
      const ratio = window.devicePixelRatio || 1;
      const [width, height] = [44, 16];
      element.width = width * ratio;
      element.height = height * ratio;
      context.setTransform(ratio, 0, 0, ratio, 0, 0);
      context.clearRect(0, 0, width, height);
      context.fillStyle = color;
      const bars = peaks ?? new Array(BARS).fill(0.08);
      const step2 = width / bars.length;
      bars.forEach((peak, bar) => {
        const tall = Math.max(1, peak * (height - 2));
        context.globalAlpha = peaks ? 0.95 : 0.35;
        context.fillRect(bar * step2, (height - tall) / 2, Math.max(1, step2 - 0.6), tall);
      });
    }, [peaks, color]);
    return /* @__PURE__ */ jsx("canvas", { "aria-hidden": true, ref: canvas, style: { width: 44, height: 16, flex: "none" } });
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
    const [over, setOver] = useState(false);
    const keys = (event) => {
      if (event.altKey && (event.key === "ArrowLeft" || event.key === "ArrowRight")) {
        event.preventDefault();
        onMove(index + (event.key === "ArrowLeft" ? -1 : 1));
      } else if (event.key === "Delete") {
        event.preventDefault();
        onRemove();
      }
    };
    return /* @__PURE__ */ jsxs(
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
          /* @__PURE__ */ jsxs(
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
                /* @__PURE__ */ jsx(Waveform, { audio, color, id }),
                /* @__PURE__ */ jsx("span", { style: S.ellipsis, children: name })
              ]
            }
          ),
          /* @__PURE__ */ jsx(
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
    const [text, setText] = useState(initial);
    const field = useRef(null);
    const done = useRef(false);
    useEffect(() => {
      field.current?.focus();
      field.current?.select();
    }, []);
    const finish = (name) => {
      if (done.current) return;
      done.current = true;
      onDone(name);
    };
    return /* @__PURE__ */ jsx(
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
  function PaletteDock({ palette, groups, playing, onPlay, onStop, onDo, said, audio, kit, make }) {
    const t = useT();
    const current = palette?.current ?? null;
    const [editing, setEditing] = useState(null);
    const [forgetting, setForgetting] = useState(false);
    const [open, setOpen] = useState(true);
    const [dragging, setDragging] = useState(null);
    const [order, setOrder] = useState(null);
    useEffect(() => setOrder(null), [palette]);
    const known = useRef(null);
    const arrived = useMemo(() => {
      const keys = new Set((current?.items ?? []).map((item) => item.key));
      const before = known.current;
      const fresh = before && before.id === current?.id ? (current?.items ?? []).filter((item) => !before.keys.has(item.key)).map((item) => item.key) : [];
      known.current = { id: current?.id ?? null, keys };
      return new Map(fresh.map((key, order2) => [key, order2]));
    }, [current]);
    const [now, setNow] = useState(() => Date.now());
    const shownFor = said?.failed ? 8e3 : 4e3;
    useEffect(() => {
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
    const header = /* @__PURE__ */ jsxs("div", { style: { ...S.row, flexWrap: "wrap", gap: 6 }, children: [
      /* @__PURE__ */ jsxs(
        "button",
        {
          "aria-expanded": open,
          className: FOCUSABLE,
          onClick: () => setOpen(!open),
          title: open ? t("dock.collapse") : t("dock.expand"),
          type: "button",
          style: { ...S.heading, font: "inherit", fontSize: 11, padding: 0, border: "none", background: "transparent", cursor: "pointer" },
          children: [
            /* @__PURE__ */ jsx("span", { "aria-hidden": true, style: { display: "inline-block", transform: open ? "rotate(90deg)" : "none", transition: "transform 120ms" }, children: "▸" }),
            t("dock.title")
          ]
        }
      ),
      editing ? /* @__PURE__ */ jsx(
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
      ) : /* @__PURE__ */ jsxs(Fragment2, { children: [
        palette && palette.palettes.length > 0 && /* @__PURE__ */ jsxs(
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
              !current && /* @__PURE__ */ jsx("option", { value: "", children: t("dock.choose") }),
              palette.palettes.map((entry) => /* @__PURE__ */ jsx("option", { value: entry.id, children: `${clean(entry.name, 40)} (${num(entry.items, 0)})` }, entry.id)),
              /* @__PURE__ */ jsx("option", { value: "\0new", children: t("dock.newPalette") })
            ]
          }
        ),
        (!palette || palette.palettes.length === 0) && /* @__PURE__ */ jsx("span", { style: S.faint, children: t("dock.none") }),
        current && /* @__PURE__ */ jsx(TextButton, { onClick: () => setEditing("rename"), title: t("dock.renameTitle"), children: t("dock.rename") }),
        palette && palette.palettes.length === 0 && /* @__PURE__ */ jsx(TextButton, { onClick: () => setEditing("new"), title: t("dock.newName"), children: t("dock.newPalette") })
      ] }),
      current && /* @__PURE__ */ jsxs("span", { role: "group", "aria-label": t("dock.mode"), style: { ...S.row, gap: 0 }, children: [
        /* @__PURE__ */ jsx(TextButton, { onClick: () => current.reach !== 0 && void run({ action: "reach", reach: 0 }), pressed: current.reach === 0, title: t("dock.strictTitle"), children: t("dock.strict") }),
        /* @__PURE__ */ jsx(TextButton, { onClick: () => current.reach === 0 && void run({ action: "reach", reach: LOOSE }), pressed: current.reach > 0, title: t("dock.looseTitle", current.reach || LOOSE), children: current.reach > 0 ? t("dock.looseN", current.reach) : t("dock.loose") })
      ] }),
      current && /* @__PURE__ */ jsxs("span", { style: S.faint, children: [
        t("dock.count", current.items.length),
        current.excluded > 0 ? ` · ${t("dock.excluded", current.excluded)}` : ""
      ] }),
      /* @__PURE__ */ jsx("span", { style: { flex: 1 } }),
      current && current.items.length > 1 && make && /* @__PURE__ */ jsx(TextButton, { onClick: () => make(current), title: t("make.openTitle"), children: t("make.open") }),
      current && current.items.length > 0 && kit?.(current),
      current && !forgetting && /* @__PURE__ */ jsx(TextButton, { onClick: () => setForgetting(true), title: t("dock.forgetTitle", clean(current.name, 40)), children: t("dock.forget") })
    ] });
    return /* @__PURE__ */ jsxs("section", { "aria-label": t("dock.title"), "data-kumi-dock": "", style: { display: "flex", flexDirection: "column", gap: 6, padding: "8px 10px", borderTop: `1px solid ${T.border}`, color: T.text }, children: [
      header,
      current && forgetting && /* @__PURE__ */ jsxs("div", { role: "alert", style: { ...S.row, flexWrap: "wrap", gap: 6 }, children: [
        /* @__PURE__ */ jsx("span", { style: S.muted, children: t("dock.forgetAsk", clean(current.name, 40), current.items.length) }),
        /* @__PURE__ */ jsx(
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
        /* @__PURE__ */ jsx(TextButton, { onClick: () => setForgetting(false), children: t("dock.keep") })
      ] }),
      open && current && items.length > 0 && /* @__PURE__ */ jsxs("div", { "aria-label": t("dock.sounds"), "data-kumi-dock-chips": "", role: "list", style: { display: "flex", flexWrap: "wrap", gap: 5, maxHeight: 116, overflowY: "auto", paddingBottom: 1 }, children: [
        items.slice(0, CHIPS_SHOWN).map((item, index) => /* @__PURE__ */ jsx(
          Chip2,
          {
            audio,
            color: colorOf2(item.group),
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
        items.length > CHIPS_SHOWN && /* @__PURE__ */ jsx("span", { style: { ...S.faint, alignSelf: "center" }, children: t("dock.more", items.length - CHIPS_SHOWN) })
      ] }),
      open && (!current || items.length === 0) && /* @__PURE__ */ jsx("span", { style: S.faint, children: t("dock.empty") }),
      /* @__PURE__ */ jsx("span", { "aria-live": "polite", style: saying ? { ...S.faint, color: saying.failed ? T.warn : T.muted } : S.hidden, children: saying ? saying.failed ? t("dock.failed", saying.text) : saying.text : "" })
    ] });
  }

  // src/shared/paletteMap.ts
  function usePaletteMap(store, state, audition, t, { hover, other, left }) {
    const revision = state.map?.revision ?? null;
    const [picks, setPicks] = useState({ revision, ids: /* @__PURE__ */ new Set() });
    const picked = picks.revision === revision ? picks.ids : /* @__PURE__ */ new Set();
    useEffect(() => setPicks((current) => current.revision === revision ? current : { revision, ids: /* @__PURE__ */ new Set() }), [revision]);
    const marked = useMemo(() => marksOf(state.palette, revision), [state.palette, revision]);
    const [flying, setFlying] = useState(null);
    const [making, setMaking] = useState(null);
    const kept = useMemo(() => new Set((state.palette?.current?.items ?? []).flatMap((item) => item.artifact_id ? [item.artifact_id] : [])), [state.palette]);
    const groupOf = useMemo(() => new Map((state.map?.points ?? []).map(([id, , , group]) => [id, group])), [state.map]);
    useEffect(() => setMaking((current) => current?.some((source) => source.sound_id && source.map_revision !== revision) ? null : current), [revision]);
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

  // src/dashboard/index.tsx
  var BASE = "/api/plugins/kumi";
  var POLL_MS = 5e3;
  var HOVER_PLAY = "kumi.mapHover";
  var TILES = "kumi.mapTiles";
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
    const store = useMemo(
      () => new KumiStore(
        (path, init) => SDK.fetchJSON(`${BASE}${path}`, init && { method: init.method, headers: { "Content-Type": "application/json" }, body: JSON.stringify(init.body) })
      ),
      []
    );
    const audition = useMemo(() => new Audition((id) => SDK.fetchJSON(`${BASE}/audio?id=${encodeURIComponent(id)}`)), []);
    useWant(store, "map");
    useWant(store, "palette");
    const state = useKumi(store);
    const playing = usePlaying(audition);
    const now = useNow();
    const [selected, setSelected] = useState(null);
    const [hover, setHover] = useState(() => globalThis.localStorage?.getItem(HOVER_PLAY) !== "off");
    const [tiles, setTiles] = useState(() => globalThis.localStorage?.getItem(TILES) === "on");
    const palette = usePaletteMap(store, state, audition, t, { hover, left: (id) => setSelected((current) => current === id ? null : current) });
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
              action: (item) => {
                const rows = changesOf(state.history?.items ?? []).find((change) => change.includes(item));
                return rows && undoRowOf(rows) === item && canUndoChange(state.status, rows) ? /* @__PURE__ */ jsx(CopyButton, { label: t("dashboard.copyUndo"), text: undoRequest(item) }) : null;
              },
              daw: state.status?.daw,
              history: state.history,
              live: Boolean(state.status?.connected) && state.status?.stale !== true,
              now,
              target: state.setup?.target
            }
          ) })
        ] }),
        /* @__PURE__ */ jsxs("div", { style: { border: `1px solid ${T.border}`, borderRadius: 6, background: T.card, overflow: "hidden", minHeight: 640, display: "flex", flexDirection: "column" }, children: [
          /* @__PURE__ */ jsx(
            MapCanvas,
            {
              actions: (id, sound) => {
                const request = soundRequest(id, sound.name, state.map?.revision);
                return /* @__PURE__ */ jsxs(Fragment2, { children: [
                  /* @__PURE__ */ jsx(TextButton, { onClick: () => playing === id ? audition.stop() : palette.play(id), children: playing === id ? t("map.stop") : t("map.play") }),
                  /* @__PURE__ */ jsx(TextButton, { disabled: palette.marked.has(id), onClick: () => void palette.gather([id]), title: t("dock.addTitle"), children: palette.marked.has(id) ? t("dock.inPalette") : t("dock.add") }),
                  request && /* @__PURE__ */ jsx(CopyButton, { label: t("dashboard.copyRequest"), text: request })
                ] });
              },
              empty: state.map === null ? t("map.reading") : t("dashboard.mapEmpty"),
              found: state.found,
              hiddenActions: /* @__PURE__ */ jsxs(Fragment2, { children: [
                /* @__PURE__ */ jsx(TextButton, { onClick: () => void store.showHidden(), title: t("map.showHiddenTitle"), children: t("map.showHidden") }),
                /* @__PURE__ */ jsx(CopyButton, { label: t("map.rescan"), text: RESCAN_REQUEST })
              ] }),
              leaving: state.leaving,
              map: state.map,
              marked: palette.marked,
              onHover: palette.onHover,
              onPick: (id) => {
                setSelected(id);
                palette.play(id);
              },
              onPicked: palette.onPicked,
              onSelect: setSelected,
              picked: palette.picked,
              pickedActions: (ids) => /* @__PURE__ */ jsxs(Fragment2, { children: [
                /* @__PURE__ */ jsx(TextButton, { onClick: () => void palette.gather(ids), strong: true, title: t("dock.addTitle"), children: t("dock.addPicked", ids.length) }),
                ids.length > 1 && /* @__PURE__ */ jsx(TextButton, { onClick: () => palette.makeFromPicks(ids), title: t("make.openTitle"), children: t("make.open") }),
                /* @__PURE__ */ jsx(TextButton, { onClick: palette.clearPicks, title: t("dock.clearPicksTitle"), children: t("dock.clearPicks") })
              ] }),
              playing,
              selected,
              flying: palette.flying,
              onTiles: (on) => {
                globalThis.localStorage?.setItem(TILES, on ? "on" : "off");
                setTiles(on);
              },
              tiles,
              tools: /* @__PURE__ */ jsx(
                TextButton,
                {
                  onClick: () => {
                    globalThis.localStorage?.setItem(HOVER_PLAY, hover ? "off" : "on");
                    setHover(!hover);
                  },
                  pressed: hover,
                  title: t("dock.hoverTitle"),
                  children: t("dock.hover")
                }
              )
            }
          ),
          /* @__PURE__ */ jsx(
            PaletteDock,
            {
              audio: (id) => audition.url(id),
              groups: state.map?.groups,
              kit: (current) => /* @__PURE__ */ jsx(CopyButton, { label: t("dock.kitCopy"), text: kitRequest(current.name) }),
              make: palette.makeFromPalette,
              onDo: (action) => store.paletteDo(action),
              onPlay: (id) => palette.play(id),
              onStop: () => audition.stop(),
              palette: state.palette,
              playing,
              said: state.paletteSaid
            }
          )
        ] }),
        palette.making && /* @__PURE__ */ jsx("div", { style: { gridColumn: 2, minWidth: 0 }, children: /* @__PURE__ */ jsx(
          MakePanel,
          {
            colorOf: palette.colorOf,
            inline: true,
            kept: palette.kept,
            onClose: palette.closeMake,
            onKeep: palette.keep,
            onMake: (request) => store.design(request),
            onPlay: (id) => void audition.play(id).catch(() => {
            }),
            onStop: () => audition.stop(),
            playing,
            results: state.designs,
            sources: palette.making,
            tempo: state.focus?.transport?.tempo,
            use: (result) => {
              const request = useRequest(result);
              return request ? /* @__PURE__ */ jsx(CopyButton, { label: t("make.copyUse"), text: request }) : null;
            }
          }
        ) })
      ] })
    ] });
  }
  window.__HERMES_PLUGINS__?.register("kumi", KumiDashboard);
})();
