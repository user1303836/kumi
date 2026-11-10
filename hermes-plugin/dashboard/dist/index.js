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
    strong: "--color-foreground",
    ground: "--color-card"
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
    useLayoutEffect,
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
      percent: (title, percent3) => `${title}: ${percent3}%`
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
      useTitle: "Put its name and id in the chat, then say what to do with it",
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
      scan: "Scan my samples…",
      scanTitle: "Put a request to scan your sample folders in the chat (Kumi asks which folder to add if none is set up), to send when you like",
      scanCopy: "Copy the request",
      setupFirst: "Kumi's runtime isn't set up yet. Once it is, Kumi builds the map from your sample folders.",
      scanningNow: "Kumi is scanning your sample folders; the map appears when it's done.",
      tiles: "Tiles",
      tilesTitle: "Each sound as a square tile in its group's colour, near where the map put it",
      hand: "Move map",
      handTitle: "Drag to move the map instead of drawing a box. The middle button moves it too, and so does a trackpad's two-finger scroll.",
      pickTool: "Pick",
      pickToolTitle: "Drag a box to pick sounds. Shift adds the box to the picks; a click with Command or Ctrl picks one sound more, or lets it go.",
      fit: "Fit",
      fitTitle: "Show the whole map (a double-click on it does this too)",
      tools: "Map tools",
      legend: "Groups: click one to hide or show it",
      player: (name) => `Picked sound: ${name}`,
      letGo: "Close"
    },
    // The map's layouts: similarity, then serve's arrangements by id (their labels are data; these are UI words).
    arrange: {
      label: "Layout",
      similar: { name: "Similarity", title: "Sounds that sound alike sit together" },
      character: { name: "Character", title: "Dark to bright, punchy to soft", left: "Dark", right: "Bright", top: "Punchy", bottom: "Soft" },
      texture: { name: "Texture", title: "Tonal to noisy, high to low", left: "Tonal", right: "Noisy", top: "High", bottom: "Low" },
      keys: { name: "Keys", title: "Round the circle of fifths: majors outside, minors inside, untuned sounds in the middle" },
      tempo: { name: "Tempo", title: "By BPM, a lane for each kind; sounds with no tempo along the bottom", left: "Slower", right: "Faster", bottom: "No tempo" },
      families: { name: "Families", title: "An island for each kind, alike sounds together inside it" },
      length: { name: "Length", title: "A spiral from the shortest sound in the middle out to the longest" },
      folders: { name: "Folders", title: "An island for each top folder, alike sounds together inside it" }
    },
    // Grow: one sound grown toward another along a vine on the map (A2).
    grow: {
      tool: "Grow",
      toolTitle: "Grow one sound toward another: click the sound to start from, then the one to grow toward",
      from: "Click the sound to grow from",
      to: (name) => `Now click the sound to grow ${name} toward`,
      cancel: "Cancel",
      title: (from, to) => `${from} → ${to}`,
      bud: (percent3) => `${percent3}% of the way`,
      handle: "Grow any point along the vine",
      hint: "Click a bud to hear it, or drag the handle along the vine",
      growing: (percent3) => `Growing the sound ${percent3}% of the way…`,
      failed: (why) => `It didn't grow: ${why}`,
      again: "Grow from here",
      againTitle: "A new vine from this sound toward the same one",
      reroll: "Another path",
      rerollTitle: "The same two sounds by another path: every point grown again",
      close: "Close the vine",
      between: "Grow between",
      betweenTitle: "Grow the first pick toward the second: the sounds between them, along a vine"
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
      fold: "Fold",
      showAll: (n) => `Show all ${n}`,
      gripTitle: "Drag to make the palette taller or shorter; click to fold it",
      made: "Made in Kumi",
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
      openTitle: "Make a new sound from these: cross, fuse, loop or stack them",
      title: "Make a sound",
      from: (n) => n === 1 ? "from 1 sound" : `from ${n} sounds`,
      close: "Close",
      sources: "The sounds it makes from",
      way: "How to make it",
      results: "Sounds made",
      none: "Sounds you make show here: play them, reroll them, and keep the ones you like.",
      ops: { cross: "Cross", fuse: "Fuse", loop: "Loop", stack: "Stack", grow: "Grow" },
      opHints: { cross: "one sound with the character of several", fuse: "something longer, made of them all", loop: "a beat or a melody out of them, in time", stack: "layered into one" },
      cards: {
        blend: { title: "Blend", hint: "One body morphed from them all" },
        attack: { title: "Attack", hint: "One's attack on the blend of the rest" },
        resonate: { title: "Resonate", hint: "Half ring through the other half" },
        modulate: { title: "Modulate", hint: "They modulate each other" },
        texture: { title: "Texture", hint: "A loop that moves between them" },
        loop: { title: "Loop", hint: "Their hits as a beat, or their tones as a melody, in time" },
        smear: { title: "Smear", hint: "A long granular stretch" },
        stack: { title: "Stack", hint: "Printed as one hit" }
      },
      options: { seconds: "Length", bpm: "Tempo", bars: "Bars", depth: "Depth", stretch: "Stretch", attack_from: "Attack from" },
      optionValues: {
        bpm: (value) => `${value} BPM`,
        bars: (n) => n === 1 ? "1 bar" : `${n} bars`
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
      useTitle: "Put its name and id in the chat, then say what to do with it",
      copyUse: "Copy its reference",
      back: "Back",
      backTitle: (n) => n === 1 ? "Make from the sound before these again" : `Make from the ${n} sounds before these again`,
      made: "Made by Kumi",
      stages: "How it was modulated",
      choose: "Choose",
      chooseTitle: "Choose it to make from next, alone or with other results",
      chosen: (n) => n === 1 ? "1 chosen" : `${n} chosen`,
      fromThese: "Make from these",
      fromTheseTitle: "Make new sounds from the chosen results: they become the sounds the panel makes from, and Back returns",
      addToSounds: "Add to the sounds",
      addToSoundsTitle: "Add the chosen results to the sounds the panel makes from",
      clearChosen: "Clear",
      clearChosenTitle: "Choose none",
      resonate: {
        pairing: "Who rings through whom",
        pairings: {
          halves: "Halves",
          "in-order": "In order",
          interleave: "Interleave",
          "bright-dark": "Bright → dark",
          "dark-bright": "Dark → bright",
          "hits-tones": "Hits → tones",
          "tones-hits": "Tones → hits",
          "one-all": "First → all",
          "all-one": "All → last",
          chain: "Chain"
        },
        pairingHints: {
          halves: "A half drawn by the seed rings through the other half",
          "in-order": "The first half, as picked, rings through the second",
          interleave: "The 1st, 3rd, 5th… ring through the 2nd, 4th…",
          "bright-dark": "The brighter half rings through the darker",
          "dark-bright": "The darker half rings through the brighter",
          "hits-tones": "Short, punchy sounds ring through long, tonal ones",
          "tones-hits": "Long, tonal sounds ring through short, punchy ones",
          "one-all": "The first rings through all the others",
          "all-one": "All the others ring through the last",
          chain: "Each rings through the next: A rings B, that rings C…"
        },
        ring: "What rings",
        rings: { partials: "Partials", body: "Body", both: "Both" },
        decay: "Ring time",
        short: "Short",
        long: "Long"
      },
      modulate: {
        carrier: "Played and shaped",
        carriers: { random: "Any", first: "First", last: "Last", brightest: "Brightest", darkest: "Darkest", longest: "Longest", shortest: "Shortest", tonal: "Most tonal", noisy: "Noisiest" },
        kinds: "Stages it may use",
        allKinds: "all, at the usual odds",
        kindNames: { phase: "Phase", ring: "Ring", am: "AM", fm: "FM", filter: "Filter", gain: "Gain", vocode: "Vocode" },
        kindHints: {
          phase: "One sound moves the other's phase",
          ring: "Ring modulation: the two multiplied, metallic",
          am: "One sound's loudness moves the other's",
          fm: "One sound bends the other's pitch",
          filter: "A filter follows one sound's level",
          gain: "One sound gates the other",
          vocode: "One sound's spectrum shapes the other"
        },
        stages: "Stages",
        stagesTitle: "How many stages shape the played sound, one after another",
        retrigger: "Retrigger",
        retriggerTitle: "The chance a short modulator repeats along the played sound"
      },
      loop: {
        style: "Style",
        styles: { beat: "Beat", melody: "Melody" },
        styleHints: { beat: "The picks' hits, played as a beat", melody: "The picks repitched into a melody, in key" },
        tempoFrom: (bpm2) => `${bpm2} BPM, your DAW's`,
        tempoTitle: "The loop's tempo: your DAW's until you move it (120 with none)",
        groups: { rhythm: "Rhythm", voices: "Voices", feel: "Feel", harmony: "Harmony", line: "Line" },
        groupHints: { rhythm: "Where the hits fall", voices: "How busy each part is", feel: "How it moves", harmony: "Key, scale and tuning", line: "The melody's shape" },
        controls: {
          meter: "Meter",
          grid: "Grid",
          density: "Density",
          syncopation: "Syncopation",
          lean: "Lean",
          fills: "Fills",
          ratchet: "Rolls",
          poly: "Polyrhythm",
          variation: "Variation",
          kick: "Kick",
          snare: "Snare",
          hats: "Hats",
          perc: "Percussion",
          open: "Open hats",
          ghosts: "Ghost notes",
          swing: "Swing",
          humanize: "Humanize",
          accent: "Accents",
          key: "Key",
          scale: "Scale",
          tuning: "Tuning",
          micro: "Microtones",
          chords: "Chords",
          drone: "Drone",
          contour: "Contour",
          octaves: "Range",
          register: "Register",
          glide: "Glide",
          gate: "Note length"
        },
        hints: {
          density: "How many steps have a hit",
          syncopation: "Hits pulled onto the off-beats",
          lean: "Hits gathered toward the start or the end of the loop",
          fills: "More happening at each phrase's end",
          ratchet: "Quick repeats of a hit",
          poly: "Voices cycling in other lengths, like 3 against 4",
          variation: "How much the bars differ",
          kick: "How busy the kick is",
          snare: "How busy the snare is",
          hats: "How busy the hats are",
          perc: "How busy the other percussion is",
          open: "The chance a hat rings open",
          ghosts: "Quiet ghost notes between the hits",
          swing: "Every other step pushed late",
          humanize: "Timing and level loosened, like a player",
          accent: "Strong and weak hits further apart",
          micro: "Notes bent off the scale by the tuning's smallest step",
          chords: "Notes stacked into chords",
          drone: "A held note underneath",
          octaves: "How many octaves the melody spans",
          register: "Higher or lower",
          glide: "Notes sliding into each other",
          gate: "How long each note holds"
        },
        grids: { 2: "8ths", 3: "Triplets", 4: "16ths", 6: "16th triplets", 8: "32nds" },
        scales: {
          major: "Major",
          minor: "Minor",
          dorian: "Dorian",
          phrygian: "Phrygian",
          lydian: "Lydian",
          mixolydian: "Mixolydian",
          locrian: "Locrian",
          "harmonic-minor": "Harmonic minor",
          "melodic-minor": "Melodic minor",
          pentatonic: "Pentatonic",
          "minor-pentatonic": "Minor pentatonic",
          blues: "Blues",
          "whole-tone": "Whole tone",
          chromatic: "Chromatic",
          hirajoshi: "Hirajōshi",
          "in-sen": "In-sen",
          "phrygian-dominant": "Phrygian dominant",
          "hungarian-minor": "Hungarian minor",
          pelog: "Pelog",
          slendro: "Slendro"
        },
        tunings: {
          "12-tet": "12 equal (standard)",
          "19-tet": "19 equal",
          "22-tet": "22 equal",
          "24-tet": "24 equal (quarter tones)",
          "31-tet": "31 equal",
          just: "Just intonation",
          pythagorean: "Pythagorean",
          "bohlen-pierce": "Bohlen–Pierce",
          harmonic: "Harmonic series"
        },
        contours: { up: "Up", down: "Down", "up-down": "Up and down", walk: "Walk", leaps: "Leaps", random: "Random" },
        registerValue: (shift) => `${shift} oct`,
        octavesValue: (n) => n === 1 ? "1 octave" : `${n} octaves`,
        early: "Early",
        late: "Late",
        reset: "Auto all",
        resetTitle: "Leave every control here to the seed"
      },
      steer: {
        title: "Steer",
        hint: "Shape what comes out. A control left alone changes nothing; Surprise moves a few.",
        count: (n) => `${n} set`,
        reset: "Reset",
        resetTitle: "Put every control back where it changes nothing",
        roll: "Surprise",
        rollTitle: "Move a few controls a good way, at random",
        sound: "The sound",
        layers: "Each layer",
        layersHint: "Each sound draws its own value inside the range from the seed, so a reroll draws again",
        controls: {
          bright: "Tone",
          body: "Body",
          filter: "Filter",
          resonance: "Resonance",
          noise: "Texture",
          punch: "Punch",
          length: "Length",
          drive: "Drive",
          crush: "Crush",
          width: "Width",
          space: "Space",
          pitch: "Pitch",
          reverse: "Reverse",
          start: "Start",
          delay: "Delay",
          pan: "Pan",
          level: "Level"
        },
        ends: {
          bright: { low: "Dark", high: "Bright" },
          body: { low: "Thin", high: "Full" },
          filter: { low: "Low-pass", high: "High-pass" },
          noise: { low: "Tonal", high: "Noisy" },
          punch: { low: "Soft", high: "Punchy" },
          length: { low: "Shorter", high: "Longer" },
          width: { low: "Mono", high: "Wide" }
        },
        hints: {
          bright: "A tilt about 1 kHz: darker or brighter, up to about 10 dB",
          body: "A low shelf about 200 Hz: thinner or fuller",
          filter: "Left closes a low-pass toward 150 Hz; right opens a high-pass toward 5 kHz",
          resonance: "The filter's peak at its cutoff (nothing while Filter is centred)",
          noise: "Spectral peaks sharpened (tonal) or flattened (noisy)",
          punch: "Transients down (soft) or up (punchy), about 12 dB",
          length: "Decays up to 4× faster, or rings up to 2× longer",
          drive: "Saturation, level-matched",
          crush: "Lo-fi: fewer bits, a lower rate",
          width: "From mono to wide",
          space: "From dry to a room, then a hall",
          pitch: "Each sound repitched, tape-style, somewhere in the range",
          reverse: "The chance each sound plays backwards",
          start: "Each sound starts this far into itself",
          delay: "Each sound comes in this much later: flams and smears",
          pan: "Each sound panned somewhere inside the width",
          level: "Each sound turned down by up to 18 dB"
        },
        values: {
          semitones: (low, high) => `${low}…${high} st`,
          ms: (low, high) => `${low}–${high} ms`,
          percentRange: (low, high) => `${low}–${high}%`,
          down: (db) => `down to −${db} dB`,
          pan: (percent3) => `±${percent3}%`
        }
      }
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
      copyReference: "Copy its reference",
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
      percent: (title, percent3) => `${title}: ${percent3}%`
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
      useTitle: "名前と ID をチャットに入れます。どうするかはそのあとで伝えてください",
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
      scan: "サンプルをスキャン…",
      scanTitle: "サンプルフォルダをスキャンする依頼をチャットに入れます（まだフォルダがなければ、どれを加えるか Kumi が尋ねます）。送信はご自身で",
      scanCopy: "依頼をコピー",
      setupFirst: "Kumi のランタイムはまだセットアップされていません。セットアップすると、サンプルフォルダからマップを作ります。",
      scanningNow: "Kumi がサンプルフォルダをスキャンしています。終わるとマップが表示されます。",
      tiles: "タイル",
      tilesTitle: "各サウンドを、マップ上の位置の近くに、グループの色の四角いタイルとして表示します",
      hand: "マップを移動",
      handTitle: "ドラッグで囲む代わりに、ドラッグでマップを動かします。中ボタンのドラッグや、トラックパッドの2本指スクロールでも動かせます。",
      pickTool: "選択",
      pickToolTitle: "ドラッグで囲んだサウンドを選びます。Shift を押すと選択に追加し、Command または Ctrl を押してクリックすると1つ追加するか、選択を外します。",
      fit: "全体",
      fitTitle: "マップ全体を表示します（マップをダブルクリックしても同じです）",
      tools: "マップのツール",
      legend: "グループ：クリックで表示と非表示を切り替えます",
      player: (name) => `選んだサウンド：${name}`,
      letGo: "閉じる"
    },
    arrange: {
      label: "レイアウト",
      similar: { name: "似ている順", title: "似た響きのサウンドどうしが近くに並びます" },
      character: { name: "キャラクター", title: "暗い〜明るい、パンチのある〜やわらかい", left: "暗い", right: "明るい", top: "パンチ", bottom: "やわらか" },
      texture: { name: "テクスチャ", title: "音程感〜ノイズ、高い〜低い", left: "音程感", right: "ノイズ", top: "高い", bottom: "低い" },
      keys: { name: "キー", title: "五度圏に沿って並びます。外側にメジャー、内側にマイナー、中央にキーのないサウンド" },
      tempo: { name: "テンポ", title: "BPM 順に、種類ごとのレーンで並びます。テンポのないサウンドは下段に", left: "遅い", right: "速い", bottom: "テンポなし" },
      families: { name: "種類", title: "種類ごとの島。島の中では似たサウンドどうしが近くに並びます" },
      length: { name: "長さ", title: "中央の最も短いサウンドから、外側の最も長いサウンドへのらせん" },
      folders: { name: "フォルダ", title: "最上位フォルダごとの島。島の中では似たサウンドどうしが近くに並びます" }
    },
    grow: {
      tool: "グロウ",
      toolTitle: "あるサウンドを別のサウンドへ育てます。始点のサウンドをクリックし、次に向かう先のサウンドをクリックします",
      from: "育て始めるサウンドをクリック",
      to: (name) => `${name} を育てる先のサウンドをクリック`,
      cancel: "キャンセル",
      title: (from, to) => `${from} → ${to}`,
      bud: (percent3) => `${percent3}% の地点`,
      handle: "つるに沿って好きな地点を育てる",
      hint: "つぼみをクリックして試聴するか、ハンドルをつるに沿ってドラッグします",
      growing: (percent3) => `${percent3}% の地点のサウンドを育てています…`,
      failed: (why) => `育てられませんでした: ${why}`,
      again: "ここから育てる",
      againTitle: "このサウンドから同じ先へ、新しいつるを育てます",
      reroll: "別の道筋",
      rerollTitle: "同じ2つのサウンドを別の道筋で。すべての地点を育て直します",
      close: "つるを閉じる",
      between: "間を育てる",
      betweenTitle: "1つ目の選択を2つ目へ育てます。その間のサウンドがつるに沿って並びます"
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
      fold: "折りたたむ",
      showAll: (n) => `すべて表示（${n}）`,
      gripTitle: "ドラッグでパレットの高さを変えます。クリックで折りたたみます",
      made: "Kumi で作成",
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
      openTitle: "これらから新しいサウンドを作ります（クロス、フューズ、ループ、スタック）",
      title: "サウンドを作る",
      from: (n) => `${n} 個のサウンドから`,
      close: "閉じる",
      sources: "元にするサウンド",
      way: "作り方",
      results: "作ったサウンド",
      none: "作ったサウンドはここに並びます。再生し、リロールし、気に入ったものはキープしてパレットに入れられます。",
      ops: { cross: "クロス", fuse: "フューズ", loop: "ループ", stack: "スタック", grow: "グロウ" },
      opHints: { cross: "複数のキャラクターを持つ 1 つの音", fuse: "すべてから作る長めの音", loop: "それらからビートかメロディを、テンポに合わせて", stack: "重ねて 1 つに" },
      cards: {
        blend: { title: "ブレンド", hint: "すべてを混ぜ合わせた 1 つのボディ" },
        attack: { title: "アタック", hint: "1 つのアタックを残りのブレンドに" },
        resonate: { title: "レゾネート", hint: "半分がもう半分を通して鳴る" },
        modulate: { title: "モジュレート", hint: "互いに変調し合う" },
        texture: { title: "テクスチャ", hint: "それらの間を動くループ" },
        loop: { title: "ループ", hint: "ヒットをビートに、または音程をメロディに、テンポに合わせて" },
        smear: { title: "スミア", hint: "長いグラニュラー・ストレッチ" },
        stack: { title: "スタック", hint: "1 つのヒットに書き出す" }
      },
      options: { seconds: "長さ", bpm: "テンポ", bars: "小節", depth: "深さ", stretch: "ストレッチ", attack_from: "アタックの元" },
      optionValues: {
        bpm: (value) => `${value} BPM`,
        bars: (n) => `${n} 小節`
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
      useTitle: "名前と ID をチャットに入れます。どうするかはそのあとで伝えてください",
      copyUse: "参照をコピー",
      back: "戻る",
      backTitle: (n) => `ひとつ前の ${n} 個のサウンドから作り直します`,
      made: "Kumi が作ったサウンド",
      stages: "変調のしかた",
      choose: "選ぶ",
      chooseTitle: "次に作るときの元に選びます（単独でも、ほかの結果と一緒でも）",
      chosen: (n) => `${n} 個を選択中`,
      fromThese: "これらから作る",
      fromTheseTitle: "選んだ結果から新しいサウンドを作ります。それらが元のサウンドになり、「戻る」で元に戻せます",
      addToSounds: "元のサウンドに加える",
      addToSoundsTitle: "選んだ結果を、元にするサウンドに加えます",
      clearChosen: "解除",
      clearChosenTitle: "何も選ばない状態にします",
      resonate: {
        pairing: "どれがどれを通して鳴るか",
        pairings: {
          halves: "半分ずつ",
          "in-order": "選んだ順",
          interleave: "交互",
          "bright-dark": "明 → 暗",
          "dark-bright": "暗 → 明",
          "hits-tones": "ヒット → トーン",
          "tones-hits": "トーン → ヒット",
          "one-all": "最初 → 全部",
          "all-one": "全部 → 最後",
          chain: "連鎖"
        },
        pairingHints: {
          halves: "シードで決まる半分が、もう半分を通して鳴ります",
          "in-order": "選んだ順の前半が後半を通して鳴ります",
          interleave: "1、3、5 番目…が 2、4 番目…を通して鳴ります",
          "bright-dark": "明るい半分が暗い半分を通して鳴ります",
          "dark-bright": "暗い半分が明るい半分を通して鳴ります",
          "hits-tones": "短くパンチのある音が、長く音程のある音を通して鳴ります",
          "tones-hits": "長く音程のある音が、短くパンチのある音を通して鳴ります",
          "one-all": "最初の音がほかのすべてを通して鳴ります",
          "all-one": "ほかのすべてが最後の音を通して鳴ります",
          chain: "それぞれが次を通して鳴ります（A が B を、B が C を…）"
        },
        ring: "鳴る部分",
        rings: { partials: "倍音", body: "ボディ", both: "両方" },
        decay: "響きの長さ",
        short: "短い",
        long: "長い"
      },
      modulate: {
        carrier: "鳴らして変える音",
        carriers: { random: "どれでも", first: "最初", last: "最後", brightest: "最も明るい", darkest: "最も暗い", longest: "最も長い", shortest: "最も短い", tonal: "最も音程的", noisy: "最もノイジー" },
        kinds: "使える段",
        allKinds: "すべて、いつもの確率で",
        kindNames: { phase: "フェイズ", ring: "リング", am: "AM", fm: "FM", filter: "フィルター", gain: "ゲイン", vocode: "ボコード" },
        kindHints: {
          phase: "ひとつの音がもうひとつの位相を動かします",
          ring: "リングモジュレーション：2 つを掛け合わせた金属的な音",
          am: "ひとつの音の大きさがもうひとつの大きさを動かします",
          fm: "ひとつの音がもうひとつの音程を曲げます",
          filter: "フィルターがひとつの音のレベルを追います",
          gain: "ひとつの音がもうひとつをゲートします",
          vocode: "ひとつの音のスペクトルがもうひとつを形づくります"
        },
        stages: "段数",
        stagesTitle: "鳴らす音を順に変えていく段の数",
        retrigger: "リトリガー",
        retriggerTitle: "短いモジュレーターが鳴らす音に沿って繰り返される確率"
      },
      loop: {
        style: "スタイル",
        styles: { beat: "ビート", melody: "メロディ" },
        styleHints: { beat: "選んだ音のヒットをビートとして鳴らします", melody: "選んだ音を音程を変えてキーに合ったメロディにします" },
        tempoFrom: (bpm2) => `${bpm2} BPM（DAW のテンポ）`,
        tempoTitle: "ループのテンポ：動かすまでは DAW のテンポ（DAW がなければ 120）",
        groups: { rhythm: "リズム", voices: "パート", feel: "ノリ", harmony: "ハーモニー", line: "ライン" },
        groupHints: { rhythm: "ヒットの置きどころ", voices: "各パートの忙しさ", feel: "動き方", harmony: "キー、スケール、チューニング", line: "メロディの形" },
        controls: {
          meter: "拍子",
          grid: "グリッド",
          density: "密度",
          syncopation: "シンコペーション",
          lean: "寄せ",
          fills: "フィル",
          ratchet: "ロール",
          poly: "ポリリズム",
          variation: "変化",
          kick: "キック",
          snare: "スネア",
          hats: "ハット",
          perc: "パーカッション",
          open: "オープンハット",
          ghosts: "ゴーストノート",
          swing: "スウィング",
          humanize: "ヒューマナイズ",
          accent: "アクセント",
          key: "キー",
          scale: "スケール",
          tuning: "チューニング",
          micro: "微分音",
          chords: "コード",
          drone: "ドローン",
          contour: "輪郭",
          octaves: "音域",
          register: "音の高さ",
          glide: "グライド",
          gate: "音の長さ"
        },
        hints: {
          density: "ヒットのあるステップの多さ",
          syncopation: "ヒットを裏拍に寄せます",
          lean: "ヒットをループの頭寄り、または終わり寄りに集めます",
          fills: "フレーズの終わりで起きることを増やします",
          ratchet: "ヒットの素早い繰り返し",
          poly: "別の長さで巡るパート（3 対 4 など）",
          variation: "小節ごとの違いの大きさ",
          kick: "キックの忙しさ",
          snare: "スネアの忙しさ",
          hats: "ハットの忙しさ",
          perc: "ほかのパーカッションの忙しさ",
          open: "ハットがオープンで鳴る確率",
          ghosts: "ヒットの間に入る小さなゴーストノート",
          swing: "1 つおきのステップを後ろにずらします",
          humanize: "演奏者のようにタイミングと音量をゆらします",
          accent: "強いヒットと弱いヒットの差を広げます",
          micro: "チューニングの最小ステップだけスケールから外れる音",
          chords: "音を重ねてコードにします",
          drone: "下で鳴り続ける音",
          octaves: "メロディが広がるオクターブ数",
          register: "高め、または低め",
          glide: "音が次の音へなめらかに移ります",
          gate: "各音を伸ばす長さ"
        },
        grids: { 2: "8 分", 3: "3 連符", 4: "16 分", 6: "16 分 3 連符", 8: "32 分" },
        scales: {
          major: "メジャー",
          minor: "マイナー",
          dorian: "ドリアン",
          phrygian: "フリジアン",
          lydian: "リディアン",
          mixolydian: "ミクソリディアン",
          locrian: "ロクリアン",
          "harmonic-minor": "ハーモニック・マイナー",
          "melodic-minor": "メロディック・マイナー",
          pentatonic: "ペンタトニック",
          "minor-pentatonic": "マイナー・ペンタトニック",
          blues: "ブルース",
          "whole-tone": "ホールトーン",
          chromatic: "クロマチック",
          hirajoshi: "平調子",
          "in-sen": "陰旋法",
          "phrygian-dominant": "フリジアン・ドミナント",
          "hungarian-minor": "ハンガリアン・マイナー",
          pelog: "ペロッグ",
          slendro: "スレンドロ"
        },
        tunings: {
          "12-tet": "12 平均律（標準）",
          "19-tet": "19 平均律",
          "22-tet": "22 平均律",
          "24-tet": "24 平均律（四分音）",
          "31-tet": "31 平均律",
          just: "純正律",
          pythagorean: "ピタゴラス音律",
          "bohlen-pierce": "ボーレン＝ピアース",
          harmonic: "倍音列"
        },
        contours: { up: "上がる", down: "下がる", "up-down": "上がって下がる", walk: "歩く", leaps: "跳ぶ", random: "ランダム" },
        registerValue: (shift) => `${shift} オクターブ`,
        octavesValue: (n) => `${n} オクターブ`,
        early: "頭寄り",
        late: "終わり寄り",
        reset: "すべて自動",
        resetTitle: "ここのコントロールをすべてシードに任せます"
      },
      steer: {
        title: "ステア",
        hint: "出てくる音を形づくります。触れていないコントロールは何も変えません。「おまかせ」はいくつかを動かします。",
        count: (n) => `${n} 個設定`,
        reset: "リセット",
        resetTitle: "すべてのコントロールを何も変えない位置に戻します",
        roll: "おまかせ",
        rollTitle: "いくつかのコントロールをランダムに大きく動かします",
        sound: "音そのもの",
        layers: "各レイヤー",
        layersHint: "それぞれの音がシードから範囲内の値を引くので、リロールで引き直されます",
        controls: {
          bright: "音色",
          body: "ボディ",
          filter: "フィルター",
          resonance: "レゾナンス",
          noise: "質感",
          punch: "パンチ",
          length: "長さ",
          drive: "ドライブ",
          crush: "クラッシュ",
          width: "広がり",
          space: "空間",
          pitch: "ピッチ",
          reverse: "リバース",
          start: "開始位置",
          delay: "ディレイ",
          pan: "パン",
          level: "レベル"
        },
        ends: {
          bright: { low: "暗い", high: "明るい" },
          body: { low: "細い", high: "太い" },
          filter: { low: "ローパス", high: "ハイパス" },
          noise: { low: "音程的", high: "ノイジー" },
          punch: { low: "やわらか", high: "パンチ" },
          length: { low: "短く", high: "長く" },
          width: { low: "モノ", high: "ワイド" }
        },
        hints: {
          bright: "約 1 kHz を中心とした傾き：暗く、または明るく（最大約 10 dB）",
          body: "約 200 Hz のローシェルフ：細く、または太く",
          filter: "左でローパスが 150 Hz に向かって閉じ、右でハイパスが 5 kHz に向かって開きます",
          resonance: "カットオフでのフィルターのピーク（フィルターが中央の間は効きません）",
          noise: "スペクトルのピークを鋭く（音程的）、または平らに（ノイジー）",
          punch: "トランジェントを下げて（やわらか）、または上げて（パンチ）、約 12 dB",
          length: "減衰を最大 4 倍速く、または響きを最大 2 倍長く",
          drive: "サチュレーション（レベルはそろえます）",
          crush: "ローファイ：少ないビット、低いレート",
          width: "モノからワイドまで",
          space: "ドライから部屋、そしてホールへ",
          pitch: "それぞれの音を範囲内のどこかへテープ式にピッチ変更",
          reverse: "それぞれの音が逆再生される確率",
          start: "それぞれの音を、その長さのこの位置から鳴らします",
          delay: "それぞれの音をこれだけ遅らせて入れます（フラムやスミア）",
          pan: "それぞれの音を広がりの中のどこかにパン",
          level: "それぞれの音を最大 18 dB 下げます"
        },
        values: {
          semitones: (low, high) => `${low}…${high} 半音`,
          ms: (low, high) => `${low}–${high} ms`,
          percentRange: (low, high) => `${low}–${high}%`,
          down: (db) => `最大 −${db} dB`,
          pan: (percent3) => `±${percent3}%`
        }
      }
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
      copyReference: "参照をコピー",
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
      percent: (title, percent3) => `${title}：${percent3}%`
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
      useTitle: "把它的名称和 ID 放进对话，然后说要怎么用它",
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
      scan: "扫描我的采样…",
      scanTitle: "把扫描采样文件夹的请求放进对话（还没有文件夹时，Kumi 会问你要加哪个），由你决定何时发送",
      scanCopy: "复制请求",
      setupFirst: "Kumi 的运行时还没有设置好。设置好后，Kumi 会根据你的采样文件夹生成地图。",
      scanningNow: "Kumi 正在扫描你的采样文件夹，扫描完成后地图就会出现。",
      tiles: "方块",
      tilesTitle: "把每个声音显示为它所在分组颜色的方块，放在地图上它原来位置的附近",
      hand: "移动地图",
      handTitle: "拖动来移动地图，而不是拉框选择。用中键拖动或触控板双指滚动也能移动地图。",
      pickTool: "选择",
      pickToolTitle: "拖出一个框来选择声音。按住 Shift 会加入已选的声音；按住 Command 或 Ctrl 点击，可以多选一个声音或取消选择它。",
      fit: "全图",
      fitTitle: "显示整张地图（双击地图也可以）",
      tools: "地图工具",
      legend: "分组：点击可隐藏或显示",
      player: (name) => `选中的声音：${name}`,
      letGo: "关闭"
    },
    arrange: {
      label: "布局",
      similar: { name: "相似度", title: "听起来相似的声音挨在一起" },
      character: { name: "性格", title: "从暗到亮，从有冲击力到柔和", left: "暗", right: "亮", top: "有冲击力", bottom: "柔和" },
      texture: { name: "质感", title: "从有音高到噪声，从高到低", left: "有音高", right: "噪声", top: "高", bottom: "低" },
      keys: { name: "调性", title: "沿五度圈排列：大调在外圈，小调在内圈，没有调的声音在中间" },
      tempo: { name: "速度", title: "按 BPM 排列，每种声音一条通道；没有速度的声音排在底部", left: "更慢", right: "更快", bottom: "没有速度" },
      families: { name: "类别", title: "每个类别一座岛，岛里相似的声音挨在一起" },
      length: { name: "长度", title: "一条螺旋：最短的声音在中心，越往外越长" },
      folders: { name: "文件夹", title: "每个顶层文件夹一座岛，岛里相似的声音挨在一起" }
    },
    grow: {
      tool: "生长",
      toolTitle: "让一个声音朝另一个声音生长：先点击起点的声音，再点击要长向的声音",
      from: "点击要开始生长的声音",
      to: (name) => `再点击 ${name} 要长向的声音`,
      cancel: "取消",
      title: (from, to) => `${from} → ${to}`,
      bud: (percent3) => `走到 ${percent3}% 的地方`,
      handle: "沿着藤蔓生长任意一点",
      hint: "点击花苞试听，或沿着藤蔓拖动手柄",
      growing: (percent3) => `正在生长 ${percent3}% 处的声音…`,
      failed: (why) => `没有长出来：${why}`,
      again: "从这里生长",
      againTitle: "从这个声音朝同一个目标长出一条新藤蔓",
      reroll: "换一条路径",
      rerollTitle: "同样两个声音，换一条路径：每一点都重新生长",
      close: "关闭藤蔓",
      between: "在两者之间生长",
      betweenTitle: "让第一个选中的声音长向第二个：它们之间的声音沿着藤蔓排开"
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
      fold: "收起",
      showAll: (n) => `全部显示（${n}）`,
      gripTitle: "拖动来调整调色板的高度；点击可收起",
      made: "在 Kumi 中生成",
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
      openTitle: "用这些声音制作一个新声音：交叉、熔合、循环或叠加",
      title: "制作声音",
      from: (n) => `来自 ${n} 个声音`,
      close: "关闭",
      sources: "用来制作的声音",
      way: "做法",
      results: "做出的声音",
      none: "做出的声音会列在这里：可以播放、重掷，把喜欢的保留在调色板里。",
      ops: { cross: "交叉", fuse: "熔合", loop: "循环", stack: "叠加", grow: "生长" },
      opHints: { cross: "一个声音，带着几个声音的特点", fuse: "用它们做成更长的声音", loop: "用它们做成合拍的节拍或旋律", stack: "叠成一个" },
      cards: {
        blend: { title: "融合", hint: "把它们揉成一个声音主体" },
        attack: { title: "起音", hint: "一个声音的起音接上其余声音的混合" },
        resonate: { title: "共鸣", hint: "一半声音通过另一半鸣响" },
        modulate: { title: "调制", hint: "它们相互调制" },
        texture: { title: "纹理", hint: "在它们之间游走的循环" },
        loop: { title: "循环", hint: "把击打组成节拍，或把音高组成旋律，合着速度" },
        smear: { title: "涂抹", hint: "长长的颗粒拉伸" },
        stack: { title: "叠加", hint: "合成一个击打" }
      },
      options: { seconds: "长度", bpm: "速度", bars: "小节", depth: "深度", stretch: "拉伸", attack_from: "起音来自" },
      optionValues: {
        bpm: (value) => `${value} BPM`,
        bars: (n) => `${n} 小节`
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
      useTitle: "把它的名称和 ID 放进对话，然后说要怎么用它",
      copyUse: "复制引用",
      back: "返回",
      backTitle: (n) => `重新用之前的 ${n} 个声音制作`,
      made: "Kumi 做出的声音",
      stages: "调制的方式",
      choose: "选择",
      chooseTitle: "选它作为下一次制作的素材，可单独使用，也可与其他结果一起",
      chosen: (n) => `已选 ${n} 个`,
      fromThese: "用这些制作",
      fromTheseTitle: "用选中的结果制作新声音：它们会成为面板用来制作的声音，按“返回”可回去",
      addToSounds: "加入素材",
      addToSoundsTitle: "把选中的结果加入面板用来制作的声音",
      clearChosen: "清除",
      clearChosenTitle: "取消所有选择",
      resonate: {
        pairing: "谁通过谁鸣响",
        pairings: {
          halves: "对半",
          "in-order": "按顺序",
          interleave: "交替",
          "bright-dark": "亮 → 暗",
          "dark-bright": "暗 → 亮",
          "hits-tones": "击打 → 音调",
          "tones-hits": "音调 → 击打",
          "one-all": "第一个 → 全部",
          "all-one": "全部 → 最后一个",
          chain: "链式"
        },
        pairingHints: {
          halves: "由种子决定的一半通过另一半鸣响",
          "in-order": "按选择顺序，前一半通过后一半鸣响",
          interleave: "第 1、3、5…个通过第 2、4…个鸣响",
          "bright-dark": "较亮的一半通过较暗的一半鸣响",
          "dark-bright": "较暗的一半通过较亮的一半鸣响",
          "hits-tones": "短而有冲击力的声音通过长而有音调的声音鸣响",
          "tones-hits": "长而有音调的声音通过短而有冲击力的声音鸣响",
          "one-all": "第一个通过其余所有声音鸣响",
          "all-one": "其余所有声音通过最后一个鸣响",
          chain: "每个通过下一个鸣响：A 激起 B，B 激起 C…"
        },
        ring: "鸣响的部分",
        rings: { partials: "泛音", body: "主体", both: "两者" },
        decay: "鸣响时长",
        short: "短",
        long: "长"
      },
      modulate: {
        carrier: "被播放和塑形的声音",
        carriers: { random: "任意", first: "第一个", last: "最后一个", brightest: "最亮", darkest: "最暗", longest: "最长", shortest: "最短", tonal: "最有音调", noisy: "最嘈杂" },
        kinds: "可用的调制级",
        allKinds: "全部，按通常的概率",
        kindNames: { phase: "相位", ring: "环形", am: "AM", fm: "FM", filter: "滤波", gain: "增益", vocode: "声码" },
        kindHints: {
          phase: "一个声音推动另一个的相位",
          ring: "环形调制：两者相乘，带金属感",
          am: "一个声音的响度带动另一个的响度",
          fm: "一个声音弯曲另一个的音高",
          filter: "滤波器跟随一个声音的电平",
          gain: "一个声音为另一个开关门限",
          vocode: "一个声音的频谱塑造另一个"
        },
        stages: "级数",
        stagesTitle: "依次塑造被播放声音的调制级数",
        retrigger: "重触发",
        retriggerTitle: "短的调制声音沿着被播放的声音重复的概率"
      },
      loop: {
        style: "风格",
        styles: { beat: "节拍", melody: "旋律" },
        styleHints: { beat: "把选中声音的击打当作节拍来演奏", melody: "把选中的声音变调成合调的旋律" },
        tempoFrom: (bpm2) => `${bpm2} BPM（DAW 的速度）`,
        tempoTitle: "循环的速度：在你调整之前跟随 DAW（没有 DAW 时为 120）",
        groups: { rhythm: "节奏", voices: "声部", feel: "律动", harmony: "和声", line: "线条" },
        groupHints: { rhythm: "击打落在哪里", voices: "每个声部有多忙", feel: "它如何流动", harmony: "调、音阶和律制", line: "旋律的形状" },
        controls: {
          meter: "拍号",
          grid: "网格",
          density: "密度",
          syncopation: "切分",
          lean: "偏向",
          fills: "加花",
          ratchet: "滚奏",
          poly: "复节奏",
          variation: "变化",
          kick: "底鼓",
          snare: "军鼓",
          hats: "踩镲",
          perc: "打击乐",
          open: "开镲",
          ghosts: "鬼音",
          swing: "摇摆",
          humanize: "人性化",
          accent: "重音",
          key: "调",
          scale: "音阶",
          tuning: "律制",
          micro: "微分音",
          chords: "和弦",
          drone: "持续音",
          contour: "轮廓",
          octaves: "音域",
          register: "音区",
          glide: "滑音",
          gate: "音符长度"
        },
        hints: {
          density: "有击打的步数多少",
          syncopation: "把击打拉到反拍上",
          lean: "把击打集中在循环的开头或结尾",
          fills: "每个乐句结尾发生得更多",
          ratchet: "击打的快速重复",
          poly: "以其他长度循环的声部，比如 3 对 4",
          variation: "各小节之间的差异",
          kick: "底鼓有多忙",
          snare: "军鼓有多忙",
          hats: "踩镲有多忙",
          perc: "其他打击乐有多忙",
          open: "踩镲敞开鸣响的概率",
          ghosts: "击打之间轻轻的鬼音",
          swing: "每隔一步往后推",
          humanize: "像演奏者一样放松时值和力度",
          accent: "强弱击打的差距更大",
          micro: "音符按律制的最小音级偏离音阶",
          chords: "把音符叠成和弦",
          drone: "底下持续的一个音",
          octaves: "旋律跨越的八度数",
          register: "更高或更低",
          glide: "音符彼此滑入",
          gate: "每个音符持续多久"
        },
        grids: { 2: "八分音符", 3: "三连音", 4: "十六分音符", 6: "十六分三连音", 8: "三十二分音符" },
        scales: {
          major: "大调",
          minor: "小调",
          dorian: "多利亚",
          phrygian: "弗里几亚",
          lydian: "利底亚",
          mixolydian: "混合利底亚",
          locrian: "洛克里亚",
          "harmonic-minor": "和声小调",
          "melodic-minor": "旋律小调",
          pentatonic: "五声音阶",
          "minor-pentatonic": "小调五声音阶",
          blues: "布鲁斯",
          "whole-tone": "全音音阶",
          chromatic: "半音阶",
          hirajoshi: "平调子",
          "in-sen": "阴旋法",
          "phrygian-dominant": "弗里几亚属音阶",
          "hungarian-minor": "匈牙利小调",
          pelog: "佩罗格",
          slendro: "斯连德罗"
        },
        tunings: {
          "12-tet": "十二平均律（标准）",
          "19-tet": "十九平均律",
          "22-tet": "二十二平均律",
          "24-tet": "二十四平均律（四分音）",
          "31-tet": "三十一平均律",
          just: "纯律",
          pythagorean: "五度相生律",
          "bohlen-pierce": "博伦–皮尔斯律",
          harmonic: "泛音列"
        },
        contours: { up: "上行", down: "下行", "up-down": "上行再下行", walk: "级进", leaps: "跳进", random: "随机" },
        registerValue: (shift) => `${shift} 个八度`,
        octavesValue: (n) => `${n} 个八度`,
        early: "偏前",
        late: "偏后",
        reset: "全部自动",
        resetTitle: "把这里的所有控件交给种子决定"
      },
      steer: {
        title: "引导",
        hint: "塑造做出来的声音。没动过的控件不会改变任何东西；“随机惊喜”会动其中几个。",
        count: (n) => `已设 ${n} 项`,
        reset: "重置",
        resetTitle: "把所有控件放回不改变任何东西的位置",
        roll: "随机惊喜",
        rollTitle: "随机把几个控件推得远一些",
        sound: "声音本身",
        layers: "每一层",
        layersHint: "每个声音都从种子中在范围内取自己的值，所以重掷会重新取",
        controls: {
          bright: "音色",
          body: "厚度",
          filter: "滤波",
          resonance: "共振",
          noise: "质感",
          punch: "冲击",
          length: "长度",
          drive: "驱动",
          crush: "降采样",
          width: "宽度",
          space: "空间",
          pitch: "音高",
          reverse: "反向",
          start: "起点",
          delay: "延迟",
          pan: "声像",
          level: "电平"
        },
        ends: {
          bright: { low: "暗", high: "亮" },
          body: { low: "薄", high: "厚" },
          filter: { low: "低通", high: "高通" },
          noise: { low: "有音调", high: "嘈杂" },
          punch: { low: "柔和", high: "有冲击力" },
          length: { low: "更短", high: "更长" },
          width: { low: "单声道", high: "宽" }
        },
        hints: {
          bright: "以约 1 kHz 为中心倾斜：更暗或更亮，最多约 10 dB",
          body: "约 200 Hz 的低架：更薄或更厚",
          filter: "向左时低通向 150 Hz 收紧；向右时高通向 5 kHz 打开",
          resonance: "滤波器在截止频率处的峰（滤波居中时不起作用）",
          noise: "把频谱峰变尖（有音调）或压平（嘈杂）",
          punch: "瞬态降低（柔和）或提高（有冲击力），约 12 dB",
          length: "衰减最多快 4 倍，或鸣响最多长 2 倍",
          drive: "饱和，电平保持一致",
          crush: "Lo-fi：更少的位数、更低的采样率",
          width: "从单声道到宽",
          space: "从干声到房间，再到大厅",
          pitch: "每个声音像磁带一样变调到范围内的某处",
          reverse: "每个声音反向播放的概率",
          start: "每个声音从自身的这个位置开始",
          delay: "每个声音晚这么多进入：装饰音和涂抹",
          pan: "每个声音放在宽度内的某处",
          level: "每个声音最多降低 18 dB"
        },
        values: {
          semitones: (low, high) => `${low}…${high} 半音`,
          ms: (low, high) => `${low}–${high} ms`,
          percentRange: (low, high) => `${low}–${high}%`,
          down: (db) => `最多 −${db} dB`,
          pan: (percent3) => `±${percent3}%`
        }
      }
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
      copyReference: "复制引用",
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
    return said ? `“${said}” (${which})` : which;
  }
  function artifactReference(artifactId, name) {
    const id = plainId(artifactId);
    if (!id) return null;
    const said = quotable(name);
    return said ? `“${said}” (artifact ${id})` : `artifact ${id}`;
  }
  function setupRequest(daw) {
    return `Set up Kumi for ${dawName(daw)}`;
  }
  function connectRequest(daw) {
    return `Connect Kumi to ${dawName(daw)}`;
  }
  var LOOK_AGAIN_REQUEST = "Kumi, look at my project again before changing anything else";
  var RESCAN_REQUEST = "Kumi, rescan my sample folders";
  var SCAN_REQUEST = "Kumi, scan my sample folders for the sound map, and ask me which folder to add if there are none";
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
  var UNIT = { min: 0, max: 1, step: 0.05 };
  var BIPOLAR = { min: -1, max: 1, step: 0.05, neutral: 0 };
  var PAIRINGS = ["halves", "in-order", "interleave", "bright-dark", "dark-bright", "hits-tones", "tones-hits", "one-all", "all-one", "chain"];
  var RINGS = ["partials", "body", "both"];
  var CARRIERS = ["random", "first", "last", "brightest", "darkest", "longest", "shortest", "tonal", "noisy"];
  var MOD_KINDS = ["phase", "ring", "am", "fm", "filter", "gain", "vocode"];
  var MODULATE_SPANS = { stages: { min: 1, max: 8, step: 1 }, retrigger: UNIT };
  var RESONATE_SPANS = { decay: UNIT };
  var GRIDS = [2, 3, 4, 6, 8];
  var METER_UNITS = [2, 4, 8, 16];
  var KEYS = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
  var SCALES = [
    "major",
    "minor",
    "dorian",
    "phrygian",
    "lydian",
    "mixolydian",
    "locrian",
    "harmonic-minor",
    "melodic-minor",
    "pentatonic",
    "minor-pentatonic",
    "blues",
    "whole-tone",
    "chromatic",
    "hirajoshi",
    "in-sen",
    "phrygian-dominant",
    "hungarian-minor",
    "pelog",
    "slendro"
  ];
  var TUNINGS = ["12-tet", "19-tet", "22-tet", "24-tet", "31-tet", "just", "pythagorean", "bohlen-pierce", "harmonic"];
  var CONTOURS = ["up", "down", "up-down", "walk", "leaps", "random"];
  var LOOP_SPANS = {
    density: UNIT,
    syncopation: UNIT,
    lean: BIPOLAR,
    fills: UNIT,
    ratchet: UNIT,
    poly: UNIT,
    variation: UNIT,
    kick: UNIT,
    snare: UNIT,
    hats: UNIT,
    perc: UNIT,
    open: UNIT,
    ghosts: UNIT,
    swing: { min: 0, max: 0.75, step: 0.05 },
    humanize: UNIT,
    accent: UNIT,
    micro: UNIT,
    chords: UNIT,
    drone: UNIT,
    octaves: { min: 1, max: 4, step: 1 },
    register: { min: -2, max: 2, step: 1, neutral: 0 },
    glide: UNIT,
    gate: { min: 0.05, max: 1, step: 0.05 }
  };
  var LOOP_GROUPS = [
    { key: "rhythm", controls: ["meter", "grid", "density", "syncopation", "lean", "fills", "ratchet", "poly", "variation"] },
    { key: "voices", style: "beat", controls: ["kick", "snare", "hats", "perc", "open", "ghosts"] },
    { key: "feel", controls: ["swing", "humanize", "accent"] },
    { key: "harmony", style: "melody", controls: ["key", "scale", "tuning", "micro", "chords", "drone"] },
    { key: "line", style: "melody", controls: ["contour", "octaves", "register", "glide", "gate"] }
  ];
  var BEAT_ONLY = ["syncopation", "lean"];
  function loopStyleOf(control) {
    if (BEAT_ONLY.includes(control)) return "beat";
    return LOOP_GROUPS.find((group) => group.controls.includes(control))?.style;
  }
  var STEER_SOUND = {
    bright: BIPOLAR,
    body: BIPOLAR,
    filter: BIPOLAR,
    resonance: UNIT,
    noise: BIPOLAR,
    punch: BIPOLAR,
    length: BIPOLAR,
    drive: UNIT,
    crush: UNIT,
    width: BIPOLAR,
    space: UNIT
  };
  var STEER_LAYER = {
    pitch: { min: -24, max: 24, step: 1, neutral: 0 },
    reverse: UNIT,
    start: { min: 0, max: 0.9, step: 0.05, neutral: 0 },
    delay: { min: 0, max: 500, step: 5, neutral: 0 },
    pan: UNIT,
    level: UNIT
  };
  var STEER_RANGES = ["pitch", "start", "delay"];
  var TABS = ["cross", "fuse", "loop", "stack"];
  var CARDS = [
    { key: "blend", tab: "cross", op: "cross", mode: "blend", options: ["seconds"] },
    { key: "attack", tab: "cross", op: "cross", mode: "attack", options: ["attack_from"] },
    { key: "resonate", tab: "cross", op: "cross", mode: "resonate", options: ["depth"] },
    { key: "modulate", tab: "cross", op: "cross", mode: "modulate", options: ["depth"] },
    { key: "texture", tab: "fuse", op: "fuse", mode: "texture", options: ["seconds", "depth"] },
    { key: "smear", tab: "fuse", op: "fuse", mode: "smear", options: ["stretch", "depth"] },
    { key: "loop", tab: "loop", op: "fuse", mode: "loop", options: ["bpm", "bars"] },
    { key: "stack", tab: "stack", op: "stack", options: [] }
  ];
  var RANGES = {
    seconds: { min: 0.5, max: 30, step: 0.5 },
    bpm: { min: 40, max: 240, step: 1 },
    bars: { min: 1, max: 8, step: 1 },
    depth: UNIT,
    stretch: { min: 4, max: 16, step: 1 },
    at: { min: 0, max: 1, step: 0.01 }
  };
  var DEFAULT_BPM = 120;
  function cardNote(count) {
    return count < 2 ? { key: "make.needTwo", least: 2 } : null;
  }
  var clampTo = (span, value) => Math.min(span.max, Math.max(span.min, value));
  var isNumber = (value) => typeof value === "number" && Number.isFinite(value);
  function spanned(span, value) {
    return isNumber(value) ? clampTo(span, value) : void 0;
  }
  function ranged(span, value) {
    if (!Array.isArray(value) || value.length !== 2 || !isNumber(value[0]) || !isNumber(value[1])) return void 0;
    const [low, high] = [clampTo(span, value[0]), clampTo(span, value[1])];
    return low <= high ? [low, high] : [high, low];
  }
  var pick = (choices, value) => choices.includes(value) ? value : void 0;
  var compact = (value) => Object.keys(value).length > 0 ? value : void 0;
  function resonateOf(options) {
    if (!options) return void 0;
    const out = {};
    const pairing = pick(PAIRINGS, options.pairing);
    const ring2 = pick(RINGS, options.ring);
    const decay = spanned(RESONATE_SPANS.decay, options.decay);
    if (pairing) out.pairing = pairing;
    if (ring2) out.ring = ring2;
    if (decay !== void 0) out.decay = decay;
    return compact(out);
  }
  function modulateOf(options) {
    if (!options) return void 0;
    const out = {};
    const carrier = pick(CARRIERS, options.carrier);
    const kinds = Array.isArray(options.kinds) ? MOD_KINDS.filter((kind) => options.kinds?.includes(kind)) : [];
    const stages = spanned(MODULATE_SPANS.stages, options.stages);
    const retrigger = spanned(MODULATE_SPANS.retrigger, options.retrigger);
    if (carrier) out.carrier = carrier;
    if (kinds.length > 0) out.kinds = kinds;
    if (stages !== void 0) out.stages = Math.round(stages);
    if (retrigger !== void 0) out.retrigger = retrigger;
    return compact(out);
  }
  function loopOf(options) {
    const style = options?.style === "melody" ? "melody" : "beat";
    const out = { style };
    if (!options) return out;
    const belongs = (control) => {
      const owner = loopStyleOf(control);
      return owner === void 0 || owner === style;
    };
    const meter = options.meter;
    if (Array.isArray(meter) && Number.isInteger(meter[0]) && meter[0] >= 1 && meter[0] <= 16 && METER_UNITS.includes(meter[1])) out.meter = [meter[0], meter[1]];
    const grid = GRIDS.find((value) => value === options.grid);
    if (grid !== void 0) out.grid = grid;
    for (const [control, choices] of [
      ["key", KEYS],
      ["scale", SCALES],
      ["tuning", TUNINGS],
      ["contour", CONTOURS]
    ]) {
      const value = pick(choices, options[control]);
      if (value && belongs(control)) Object.assign(out, { [control]: value });
    }
    for (const control of Object.keys(LOOP_SPANS)) {
      const value = spanned(LOOP_SPANS[control], options[control]);
      if (value !== void 0 && belongs(control)) out[control] = LOOP_SPANS[control].step === 1 ? Math.round(value) : value;
    }
    return out;
  }
  function steerOf(steer) {
    if (!steer) return void 0;
    const out = {};
    for (const [key, span] of Object.entries({ ...STEER_SOUND, ...STEER_LAYER })) {
      if (STEER_RANGES.includes(key)) {
        const range = ranged(span, steer[key]);
        if (range && !(range[0] === (span.neutral ?? span.min) && range[1] === (span.neutral ?? span.min))) out[key] = range;
      } else {
        const value = spanned(span, steer[key]);
        if (value !== void 0 && value !== (span.neutral ?? span.min)) out[key] = value;
      }
    }
    return compact(out);
  }
  function steerCount(steer) {
    return Object.keys(steerOf(steer) ?? {}).length;
  }
  function rollSteer(random = Math.random) {
    const keys = [...Object.keys(STEER_SOUND), ...Object.keys(STEER_LAYER)];
    const count = 3 + Math.floor(random() * 3);
    const chosen = [];
    while (chosen.length < count) {
      const key = keys[Math.floor(random() * keys.length)];
      if (!chosen.includes(key)) chosen.push(key);
    }
    const out = {};
    for (const key of chosen) {
      const span = key in STEER_SOUND ? STEER_SOUND[key] : STEER_LAYER[key];
      const snap2 = (value) => Math.round(clampTo(span, value) / span.step) * span.step;
      if (STEER_RANGES.includes(key)) {
        const middle = span.neutral ?? span.min;
        const reach = (span.max - span.min) * (0.08 + random() * 0.17);
        const low = snap2(middle === span.min ? span.min + reach * random() * 0.3 : middle - reach * random());
        const high = snap2(middle === span.min ? low + reach : middle + reach * random());
        out[key] = [Math.min(low, high), Math.max(low, high)];
      } else if (span.neutral === 0 && span.min < 0) {
        const value = (0.2 + random() * 0.6) * (random() < 0.5 ? -1 : 1);
        out[key] = snap2(value);
      } else {
        out[key] = snap2(span.min + (span.max - span.min) * (0.2 + random() * 0.5));
      }
    }
    return out;
  }
  function requestFor(card, sources, options = {}, seed) {
    const kept3 = {};
    for (const key of card.options) {
      const value = options[key];
      if (key === "attack_from") {
        if (typeof value === "number" && Number.isInteger(value) && value >= 0 && value < sources.length) kept3.attack_from = value;
      } else {
        const span = spanned(RANGES[key], value);
        if (span !== void 0) kept3[key] = RANGES[key].step === 1 ? Math.round(span) : span;
      }
    }
    const resonate = card.key === "resonate" ? resonateOf(options.resonate) : void 0;
    const modulate = card.key === "modulate" ? modulateOf(options.modulate) : void 0;
    if (resonate) kept3.resonate = resonate;
    if (modulate) kept3.modulate = modulate;
    if (card.key === "loop") kept3.loop = loopOf(options.loop);
    const steer = steerOf(options.steer);
    if (steer) kept3.steer = steer;
    return {
      op: card.op,
      ...card.mode ? { mode: card.mode } : {},
      sources,
      ...seed === void 0 ? {} : { seed },
      ...Object.keys(kept3).length ? { options: kept3 } : {}
    };
  }
  function growRequest(from, to, at, seed, steer) {
    const kept3 = { at: spanned(RANGES.at, at) ?? 0.5 };
    const steered = steerOf(steer);
    if (steered) kept3.steer = steered;
    return { op: "grow", sources: [from, to], ...seed === void 0 ? {} : { seed }, options: kept3 };
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
  function sourceOfResult(result) {
    return { artifact_id: result.artifact_id, name: resultName(result) };
  }
  function stagesOf(recipe) {
    const body = recipe.includes(":") ? recipe.slice(recipe.indexOf(":") + 1) : recipe;
    const stages = body.split(/\s*(?:→|->)\s*/).map((stage) => stage.replace(/[;·].*$/, "").trim()).filter(Boolean);
    return stages.length > 1 ? stages : [];
  }
  function useRequest(result) {
    return artifactReference(result.artifact_id, resultName(result));
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
    const op = raw.op === "cross" || raw.op === "fuse" || raw.op === "stack" || raw.op === "grow" ? raw.op : null;
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

  // src/shared/grow.ts
  var BUDS = [0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875];
  var RENDERS = 2;
  var SCRUB_STEP = 0.04;
  function soundEnd(id, name, revision) {
    const [source] = sourcesOf([{ sound_id: id, name: name ?? id }], revision);
    return source ? { source, id, place: null, name: clean(name ?? id, 80) } : null;
  }
  function vinePoint(a, b, t, bow = 0.22) {
    const [cx, cy] = [(a[0] + b[0]) / 2 - (b[1] - a[1]) * bow, (a[1] + b[1]) / 2 + (b[0] - a[0]) * bow];
    const u = 1 - t;
    return [u * u * a[0] + 2 * u * t * cx + t * t * b[0], u * u * a[1] + 2 * u * t * cy + t * t * b[1]];
  }
  function nearestOnVine(a, b, p, bow = 0.22) {
    let [best, nearest] = [0, Infinity];
    for (let step2 = 0; step2 <= 240; step2++) {
      const [x, y] = vinePoint(a, b, step2 / 240, bow);
      const distance = (x - p[0]) ** 2 + (y - p[1]) ** 2;
      if (distance < nearest) [best, nearest] = [step2 / 240, distance];
    }
    return best;
  }
  function nextToGrow(buds, scrub, running) {
    if (running >= RENDERS) return null;
    if (scrub?.state === "waiting") return scrub;
    return buds.find((bud) => bud.state === "waiting") ?? null;
  }
  var newSeed = () => Math.floor(Math.random() * 2 ** 31);
  var fresh = () => BUDS.map((at) => ({ at, state: "waiting" }));
  function useGrow(design, heard2, revision) {
    const [choosing, setChoosing] = useState(null);
    const [path, setPath] = useState(null);
    const [pending, setPending] = useState(null);
    const [buds, setBuds] = useState([]);
    const [scrub, setScrub] = useState(null);
    const [handle, setHandle] = useState(0.5);
    const [picked, setPicked] = useState(null);
    const [running, setRunning] = useState(0);
    const generation = useRef(0);
    const heardRef = useRef(heard2);
    heardRef.current = heard2;
    const scrubAt = useRef(null);
    const start = (from, to) => {
      generation.current += 1;
      setPath({ from, to, seed: newSeed() });
      setBuds(fresh());
      setScrub(null);
      setHandle(0.5);
      setPicked(null);
      setChoosing(null);
      setPending(null);
    };
    const close = () => {
      generation.current += 1;
      setPath(null);
      setBuds([]);
      setScrub(null);
      setPicked(null);
      setChoosing(null);
      setPending(null);
    };
    useEffect(close, [revision]);
    useEffect(
      () => () => {
        generation.current += 1;
      },
      []
    );
    useEffect(() => {
      if (!path) return;
      const next = nextToGrow(buds, scrub, running);
      if (!next) return;
      const mine = generation.current;
      const isScrub = next === scrub;
      const mark = (bud) => isScrub ? setScrub((current) => current?.at === next.at ? bud : current) : setBuds((current) => current.map((each) => each.at === next.at ? bud : each));
      mark({ at: next.at, state: "growing" });
      setRunning((count) => count + 1);
      void design(growRequest(path.from.source, path.to.source, next.at, path.seed)).then((answer) => {
        setRunning((count) => count - 1);
        if (mine !== generation.current) return;
        if ("result" in answer) {
          mark({ at: next.at, state: "ready", result: answer.result });
          if (isScrub && scrubAt.current === next.at) heardRef.current(answer.result);
        } else {
          mark({ at: next.at, state: "failed", error: answer.error });
        }
      });
    }, [path, buds, scrub, running]);
    return {
      choosing,
      // A new vine's first sound, once named, is the one shown and named in the prompt, even with another vine open.
      from: pending ?? path?.from ?? null,
      to: pending ? null : path?.to ?? null,
      buds,
      handle,
      scrub,
      picked,
      choose: (on) => {
        setChoosing(on ? "from" : null);
        setPending(null);
      },
      name: (end) => {
        if (choosing === "from") {
          setPending(end);
          setChoosing("to");
        } else if (choosing === "to" && pending && (end.id ?? end.source.artifact_id) !== (pending.id ?? pending.source.artifact_id)) {
          start(pending, end);
        }
      },
      between: start,
      scrubTo: (at, done) => {
        setHandle(at);
        if (done) setPicked(at);
        const grown = buds.find((each) => each.at === at);
        if (grown?.state === "ready") {
          scrubAt.current = at;
          if (done) heardRef.current(grown.result);
          return;
        }
        const asks = done || !scrub || scrub.state !== "growing" && scrub.state !== "waiting" && Math.abs(scrub.at - at) >= SCRUB_STEP;
        if (!asks) return;
        scrubAt.current = at;
        setScrub({ at, state: "waiting" });
      },
      pick: (at) => {
        setPicked(at);
        const bud = at === null ? null : scrub?.at === at ? scrub : buds.find((each) => each.at === at);
        if (bud?.state === "ready") heardRef.current(bud.result);
      },
      reroll: () => {
        if (path) start(path.from, path.to);
      },
      close
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
    const kept3 = result.compare?.artifact_id;
    if (typeof kept3 !== "string" || !kept3) return null;
    for (const [key, each] of Object.entries(rounds)) {
      const round = each.find((other) => other.kept === kept3);
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
    const kept3 = result.compare?.artifact_id;
    return { id, gaps: gapsOf(result), kept: typeof kept3 === "string" && kept3 ? kept3 : null, name: result.compare?.name ? displayReferenceName(result.compare) : referenceName(result, rounds) ?? fileName(result.compare?.metrics, "") };
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
    /** Whether this window asked for the engine's earlier results (`readDesignsOnce`): once, however many results this
     *  window made before (a grow renders before the Make panel ever opens). */
    designsRead = false;
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
      this.designsRead = false;
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
    /** The engine's recent results, newest first (`GET /designs`), for the Make panel's history, after the ones this
     *  window made already (a grow's buds, a make before the read answered), each once. None when the backend has no
     *  sound design (an older plugin) or can't read them. */
    async readDesigns() {
      const generation = this.generation;
      try {
        const answer = await this.fetch("/designs");
        if (generation !== this.generation) return;
        const history = (Array.isArray(answer?.designs) ? answer.designs : []).map(resultOf).filter((result) => result !== null);
        const made = this.state.designs ?? [];
        const have = new Set(made.map((result) => result.artifact_id));
        this.set({ designs: [...made, ...history.filter((result) => !have.has(result.artifact_id))] });
      } catch {
        if (generation === this.generation && this.state.designs === null) this.set({ designs: [] });
      }
    }
    /** `readDesigns`, the first time this window asks (the Make panel opening); later opens read nothing more. */
    readDesignsOnce() {
      if (this.designsRead) return Promise.resolve();
      this.designsRead = true;
      return this.readDesigns();
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
      const kept3 = ids.slice(0, 200);
      const seeds = Array.isArray(said.near) ? said.near : [said.near];
      const near = seeds.filter((seed) => typeof seed === "string" && seed !== "").slice(0, 16);
      this.set({ found: { revision, ids: kept3, at: Date.now(), near, matches: foundMatches(said.matches, new Set(kept3)) } });
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
      const kept3 = Object.entries(this.state.jobs).filter(([, job]) => job.ended_at === null || now - job.ended_at < JOB_KEPT_MS);
      if (kept3.length !== Object.keys(this.state.jobs).length) this.set({ jobs: Object.fromEntries(kept3) });
      const stopped = this.state.status?.stale === true && this.state.status.running === false;
      if (this.state.status?.stale && !stopped) return Promise.resolve();
      return Promise.all(kept3.filter(([, job]) => job.ended_at === null).map(([id]) => this.refreshJob(id, stopped))).then(() => void 0);
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
      const loading2 = this.loading.get(id);
      if (loading2) return loading2;
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
      for (const [oldest, kept3] of this.cache) {
        if (this.cached <= this.limit) break;
        this.cache.delete(oldest);
        this.cached -= kept3.length;
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
    return /* @__PURE__ */ jsx("style", { children: `.${FOCUSABLE}:focus-visible{outline:2px solid ${T.focus};outline-offset:2px;border-radius:3px}@keyframes kumi-ants{to{stroke-dashoffset:-18}}.kumi-ants{animation:kumi-ants .7s linear infinite}@keyframes kumi-pop{0%{transform:scale(.55) translateY(6px);opacity:0}70%{transform:scale(1.06);opacity:1}100%{transform:none;opacity:1}}.kumi-pop{animation:kumi-pop .34s cubic-bezier(.2,.8,.3,1) both}@keyframes kumi-spin{to{transform:rotate(360deg)}}.kumi-spin{display:inline-block;animation:kumi-spin .8s linear infinite}@keyframes kumi-vine{from{stroke-dashoffset:1}to{stroke-dashoffset:0}}.kumi-vine{stroke-dasharray:1;animation:kumi-vine .9s cubic-bezier(.3,.6,.2,1) both}@keyframes kumi-breathe{50%{opacity:.35}}.kumi-breathe{animation:kumi-breathe 1.1s ease-in-out infinite}@keyframes kumi-open{0%{transform:scale(0)}70%{transform:scale(1.25)}100%{transform:scale(1)}}.kumi-open{transform-box:fill-box;transform-origin:center;animation:kumi-open .42s cubic-bezier(.2,.8,.3,1) both}@media (prefers-reduced-motion:reduce){.kumi-ants,.kumi-pop,.kumi-vine,.kumi-open{animation:none}.kumi-breathe{animation-duration:3s}.kumi-spin{animation-duration:2.4s}}` });
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
      const track2 = nameOf(item.track);
      const dawUndo = item.state === "applied" && (item.daw_undo === "next" || item.daw_undo === "buried") ? item.daw_undo : null;
      return /* @__PURE__ */ jsxs("li", { style: { ...S.row, gap: 7, padding: "4px 0", borderTop: `1px solid ${T.border}`, opacity: item.state === "undone" ? 0.7 : 1 }, children: [
        /* @__PURE__ */ jsx("span", { "aria-label": state.word, title: state.word, style: { width: 14, textAlign: "center", color: TONE[state.tone], flex: "none" }, children: state.mark }),
        /* @__PURE__ */ jsxs("span", { style: { display: "flex", flexDirection: "column", minWidth: 0, flex: 1 }, children: [
          /* @__PURE__ */ jsx("span", { style: { ...S.text, ...S.ellipsis, textDecoration: item.state === "undone" ? "line-through" : void 0 }, title: clean(item.title ?? "", 300), children: clean(item.title ?? item.label ?? item.id, 120) }),
          /* @__PURE__ */ jsxs("span", { style: { ...S.row, gap: 5 }, children: [
            track2 && /* @__PURE__ */ jsx(Dot, { color: colorOf(item.track) ?? T.faint, size: 6 }),
            /* @__PURE__ */ jsx("span", { style: { ...S.faint, ...S.ellipsis }, children: [track2 && clean(track2, 24), item.family && clean(item.family, 16), item.label && clean(item.label, 12), ago(item.at, now, t)].filter(Boolean).join(" · ") })
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

  // src/shared/controls.tsx
  var clamp = (span, value) => Math.min(span.max, Math.max(span.min, value));
  var snap = (span, value) => {
    const steps = Math.round((clamp(span, value) - span.min) / span.step);
    const decimals = Math.max(0, -Math.floor(Math.log10(span.step)));
    return Number((span.min + steps * span.step).toFixed(decimals + 1));
  };
  var fraction = (span, value) => (clamp(span, value) - span.min) / (span.max - span.min);
  function Slider({
    label,
    span,
    value,
    onChange,
    format,
    rest,
    restAt,
    ends,
    range = false,
    title
  }) {
    const track2 = useRef(null);
    const [dragging, setDragging] = useState(null);
    const middle = span.neutral ?? (range ? span.min : (span.min + span.max) / 2);
    const resting = restAt ?? middle;
    const values = value === void 0 ? range ? [resting, resting] : [resting] : Array.isArray(value) ? value : [value];
    const set = value !== void 0;
    const from = (clientX) => {
      const box = track2.current?.getBoundingClientRect();
      if (!box || box.width === 0) return values[0];
      return snap(span, span.min + (clientX - box.left) / box.width * (span.max - span.min));
    };
    const commit = (thumb, next) => {
      if (!range) return onChange(next);
      const pair = thumb === 0 ? [Math.min(next, values[1]), values[1]] : [values[0], Math.max(next, values[0])];
      onChange(pair);
    };
    const press = (event) => {
      if (event.button !== 0) return;
      const next = from(event.clientX);
      const thumb = range ? Math.abs(next - values[0]) < Math.abs(next - values[1]) || values[0] === values[1] && next < values[0] ? 0 : 1 : 0;
      event.currentTarget.setPointerCapture?.(event.pointerId);
      setDragging(thumb);
      commit(thumb, next);
    };
    const keys = (thumb) => (event) => {
      const big = event.shiftKey ? 10 : 1;
      const at = values[thumb];
      const moves = {
        ArrowRight: at + span.step * big,
        ArrowUp: at + span.step * big,
        ArrowLeft: at - span.step * big,
        ArrowDown: at - span.step * big,
        PageUp: at + span.step * 10,
        PageDown: at - span.step * 10,
        Home: span.min,
        End: span.max
      };
      if (event.key === "Delete" || event.key === "Backspace") {
        event.preventDefault();
        onChange(void 0);
      } else if (event.key in moves) {
        event.preventDefault();
        commit(thumb, snap(span, moves[event.key]));
      }
    };
    const [a, b] = range ? [fraction(span, values[0]), fraction(span, values[1])] : [fraction(span, Math.min(middle, values[0])), fraction(span, Math.max(middle, values[0]))];
    const fillFrom = range || span.neutral !== void 0 ? a : 0;
    const fillTo = range || span.neutral !== void 0 ? b : fraction(span, values[0]);
    const shown = set ? format(value) : rest;
    return /* @__PURE__ */ jsxs("div", { style: { display: "flex", flexDirection: "column", gap: 3, minWidth: 0 }, title, children: [
      /* @__PURE__ */ jsxs("span", { style: { ...S.row, gap: 6, fontSize: 11.5, color: set ? T.secondary : T.muted }, children: [
        /* @__PURE__ */ jsx("span", { style: { ...S.ellipsis, flex: 1 }, children: label }),
        /* @__PURE__ */ jsx(
          "button",
          {
            "aria-label": set ? `${label}: ${shown}` : `${label}: ${rest || "—"}`,
            className: FOCUSABLE,
            disabled: !set,
            onClick: () => onChange(void 0),
            type: "button",
            style: {
              font: "inherit",
              fontSize: 10.5,
              fontVariantNumeric: "tabular-nums",
              padding: "0 6px",
              borderRadius: 8,
              border: `1px solid ${set ? `color-mix(in srgb, ${T.accent} 55%, transparent)` : "transparent"}`,
              background: set ? `color-mix(in srgb, ${T.accent} 12%, transparent)` : "transparent",
              color: set ? T.accent : T.faint,
              cursor: set ? "pointer" : "default",
              whiteSpace: "nowrap",
              transition: "background-color 140ms, border-color 140ms, color 140ms"
            },
            children: shown || "·"
          }
        )
      ] }),
      /* @__PURE__ */ jsxs(
        "div",
        {
          onDoubleClick: () => onChange(void 0),
          onPointerCancel: () => setDragging(null),
          onPointerDown: press,
          onPointerMove: (event) => {
            if (dragging !== null) commit(dragging, from(event.clientX));
          },
          onPointerUp: () => setDragging(null),
          ref: track2,
          style: { position: "relative", height: 16, cursor: "pointer", touchAction: "none" },
          children: [
            /* @__PURE__ */ jsx("span", { "aria-hidden": true, style: { position: "absolute", left: 0, right: 0, top: 6, height: 4, borderRadius: 2, background: T.border, opacity: set ? 0.9 : 0.55 } }),
            span.neutral !== void 0 && span.neutral > span.min && /* @__PURE__ */ jsx("span", { "aria-hidden": true, style: { position: "absolute", left: `${fraction(span, span.neutral) * 100}%`, top: 3, width: 1, height: 10, background: T.faint, transform: "translateX(-0.5px)" } }),
            set && /* @__PURE__ */ jsx(
              "span",
              {
                "aria-hidden": true,
                style: {
                  position: "absolute",
                  left: `${fillFrom * 100}%`,
                  width: `${Math.max(0, fillTo - fillFrom) * 100}%`,
                  top: 6,
                  height: 4,
                  borderRadius: 2,
                  background: T.accent,
                  boxShadow: `0 0 8px -2px ${T.accent}`,
                  transition: dragging === null ? "left 120ms, width 120ms" : "none"
                }
              }
            ),
            values.map((at, thumb) => /* @__PURE__ */ jsx(
              "div",
              {
                "aria-label": range ? `${label} ${thumb === 0 ? "↓" : "↑"}` : label,
                "aria-valuemax": span.max,
                "aria-valuemin": span.min,
                "aria-valuenow": at,
                "aria-valuetext": set ? shown : rest,
                className: FOCUSABLE,
                onKeyDown: keys(thumb),
                role: "slider",
                tabIndex: 0,
                style: {
                  position: "absolute",
                  left: `${fraction(span, at) * 100}%`,
                  top: 2,
                  width: 12,
                  height: 12,
                  borderRadius: "50%",
                  transform: `translateX(-6px) scale(${dragging === thumb ? 1.25 : 1})`,
                  background: T.raised,
                  border: `2px solid ${set ? T.accent : T.faint}`,
                  opacity: set ? 1 : 0.7,
                  boxShadow: set ? `0 2px 6px -2px ${T.accent}` : "none",
                  transition: dragging === null ? "left 120ms, transform 120ms, border-color 140ms" : "transform 120ms"
                }
              },
              thumb
            ))
          ]
        }
      ),
      ends && /* @__PURE__ */ jsxs("span", { "aria-hidden": true, style: { ...S.row, justifyContent: "space-between", fontSize: 10, color: T.faint, marginTop: -2 }, children: [
        /* @__PURE__ */ jsx("span", { children: ends[0] }),
        /* @__PURE__ */ jsx("span", { children: ends[1] })
      ] })
    ] });
  }
  function Pills({
    label,
    options,
    value,
    onChange,
    rest
  }) {
    const all = rest === null ? options : [{ value: void 0, label: rest }, ...options];
    return /* @__PURE__ */ jsxs("div", { style: { display: "flex", flexDirection: "column", gap: 4, minWidth: 0 }, children: [
      /* @__PURE__ */ jsx("span", { style: { fontSize: 11.5, color: value === void 0 ? T.muted : T.secondary }, children: label }),
      /* @__PURE__ */ jsx("div", { "aria-label": label, role: "radiogroup", style: { ...S.row, flexWrap: "wrap", gap: 4 }, children: all.map((option) => {
        const chosen = option.value === value;
        return /* @__PURE__ */ jsxs(
          "button",
          {
            "aria-checked": chosen,
            className: FOCUSABLE,
            onClick: () => onChange(option.value),
            role: "radio",
            title: option.title,
            type: "button",
            style: {
              ...S.row,
              gap: 4,
              font: "inherit",
              fontSize: 11.5,
              fontWeight: chosen ? 600 : 500,
              padding: "2px 9px",
              borderRadius: 12,
              border: `1px solid ${chosen ? T.accent : T.border}`,
              background: chosen ? `color-mix(in srgb, ${T.accent} 16%, transparent)` : "transparent",
              color: chosen ? T.accent : option.value === void 0 ? T.muted : T.secondary,
              cursor: "pointer",
              transition: "border-color 140ms, background-color 140ms, color 140ms"
            },
            children: [
              option.glyph,
              option.label
            ]
          },
          String(option.value ?? "")
        );
      }) })
    ] });
  }
  function Choice({ label, options, value, onChange, rest }) {
    return /* @__PURE__ */ jsxs("label", { style: { display: "flex", flexDirection: "column", gap: 4, minWidth: 0, fontSize: 11.5, color: value === void 0 ? T.muted : T.secondary }, children: [
      label,
      /* @__PURE__ */ jsxs(
        "select",
        {
          className: FOCUSABLE,
          onChange: (event) => onChange(event.target.value === "" ? void 0 : event.target.value),
          style: { font: "inherit", fontSize: 12, padding: "2px 6px", borderRadius: 6, border: `1px solid ${value === void 0 ? T.border : T.accent}`, background: "transparent", color: value === void 0 ? T.secondary : T.accent },
          value: value ?? "",
          children: [
            /* @__PURE__ */ jsx("option", { value: "", children: rest }),
            options.map((option) => /* @__PURE__ */ jsx("option", { value: option.value, children: option.label }, option.value))
          ]
        }
      )
    ] });
  }
  function Toggles({ label, options, value, onChange, rest }) {
    const chosen = value ?? [];
    return /* @__PURE__ */ jsxs("div", { style: { display: "flex", flexDirection: "column", gap: 4, minWidth: 0 }, children: [
      /* @__PURE__ */ jsxs("span", { style: { ...S.row, gap: 6, fontSize: 11.5, color: chosen.length ? T.secondary : T.muted }, children: [
        label,
        chosen.length === 0 && /* @__PURE__ */ jsx("span", { style: { color: T.faint, fontSize: 10.5 }, children: rest })
      ] }),
      /* @__PURE__ */ jsx("div", { "aria-label": label, role: "group", style: { ...S.row, flexWrap: "wrap", gap: 4 }, children: options.map((option) => {
        const on = chosen.includes(option.value);
        return /* @__PURE__ */ jsxs(
          "button",
          {
            "aria-pressed": on,
            className: FOCUSABLE,
            onClick: () => {
              const next = on ? chosen.filter((other) => other !== option.value) : [...chosen, option.value];
              onChange(next.length ? next : void 0);
            },
            title: option.title,
            type: "button",
            style: {
              font: "inherit",
              fontSize: 11.5,
              padding: "2px 9px",
              borderRadius: 6,
              border: `1px solid ${on ? T.accent : T.border}`,
              background: on ? `color-mix(in srgb, ${T.accent} 16%, transparent)` : "transparent",
              color: on ? T.accent : T.secondary,
              cursor: "pointer",
              transition: "border-color 140ms, background-color 140ms, color 140ms"
            },
            children: [
              on ? "✓ " : "",
              option.label
            ]
          },
          option.value
        );
      }) })
    ] });
  }
  function Group({ title, count, summary, open, onToggle, actions, children, hint }) {
    return /* @__PURE__ */ jsxs("section", { style: { display: "flex", flexDirection: "column", gap: 8, padding: "8px 10px", borderRadius: 10, border: `1px solid ${open ? `color-mix(in srgb, ${T.border} 100%, transparent)` : T.border}`, background: open ? `color-mix(in srgb, ${T.card} 60%, transparent)` : "transparent", transition: "background-color 160ms" }, children: [
      /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 6 }, children: [
        /* @__PURE__ */ jsxs(
          "button",
          {
            "aria-expanded": open,
            className: FOCUSABLE,
            onClick: onToggle,
            type: "button",
            style: { ...S.row, gap: 6, flex: 1, font: "inherit", fontSize: 12, fontWeight: 650, padding: 0, border: "none", background: "transparent", color: T.text, cursor: "pointer", textAlign: "left" },
            children: [
              /* @__PURE__ */ jsx("span", { "aria-hidden": true, style: { display: "inline-block", color: T.muted, transform: open ? "rotate(90deg)" : "none", transition: "transform 160ms" }, children: "▸" }),
              title,
              count && /* @__PURE__ */ jsx("span", { style: { fontSize: 10.5, fontWeight: 500, padding: "0 6px", borderRadius: 8, background: `color-mix(in srgb, ${T.accent} 14%, transparent)`, color: T.accent }, children: count }),
              !open && summary && /* @__PURE__ */ jsx("span", { style: { ...S.ellipsis, flex: 1, fontSize: 11, fontWeight: 400, color: T.muted }, children: summary })
            ]
          }
        ),
        actions
      ] }),
      open && hint && /* @__PURE__ */ jsx("span", { style: { ...S.faint, fontSize: 11, marginTop: -4 }, children: hint }),
      open && /* @__PURE__ */ jsx("div", { style: { display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(150px, 1fr))", gap: "10px 16px", alignItems: "start" }, children })
    ] });
  }

  // src/shared/MakeOptions.tsx
  var percent = (value) => Math.round(value * 100);
  var signed2 = (value) => value > 0 ? `+${value}` : value < 0 ? `−${Math.abs(value)}` : "0";
  var METERS = [
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
  ];
  function CardOptions({ card, options, sources, onChange }) {
    const t = useT();
    const auto = t("make.auto");
    const set = (key, value) => onChange((current) => ({ ...current, [key]: value }));
    const sliders = card.options.flatMap((name) => {
      if (name === "attack_from" || name === "bpm" || name === "bars") return [];
      const labels = {
        seconds: (value) => seconds(value),
        depth: (value) => `${percent(value)}%`,
        stretch: (value) => `${value}×`
      };
      return [
        /* @__PURE__ */ jsx(
          Slider,
          {
            label: t(`make.options.${name}`),
            onChange: (value) => set(name, value),
            rest: auto,
            span: RANGES[name],
            value: options[name],
            format: (value) => labels[name](value),
            title: t("make.autoTitle")
          },
          name
        )
      ];
    });
    const resonate = options.resonate ?? {};
    const modulate = options.modulate ?? {};
    return /* @__PURE__ */ jsxs("div", { style: { display: "flex", flexDirection: "column", gap: 10 }, children: [
      card.options.includes("attack_from") && /* @__PURE__ */ jsx(
        Choice,
        {
          label: t("make.options.attack_from"),
          onChange: (value) => set("attack_from", value === void 0 ? void 0 : Number(value)),
          options: sources.slice(0, 64).map((source, index) => ({ value: String(index), label: clean(source.name ?? source.sound_id ?? source.artifact_id ?? "", 40) })),
          rest: auto,
          value: options.attack_from === void 0 ? void 0 : String(options.attack_from)
        }
      ),
      card.key === "resonate" && /* @__PURE__ */ jsxs(Fragment2, { children: [
        /* @__PURE__ */ jsx(
          Pills,
          {
            label: t("make.resonate.pairing"),
            onChange: (pairing) => set("resonate", { ...resonate, pairing }),
            options: PAIRINGS.map((pairing) => ({ value: pairing, label: t(`make.resonate.pairings.${pairing}`), title: t(`make.resonate.pairingHints.${pairing}`) })),
            rest: auto,
            value: resonate.pairing
          }
        ),
        /* @__PURE__ */ jsx(Pills, { label: t("make.resonate.ring"), onChange: (ring2) => set("resonate", { ...resonate, ring: ring2 }), options: RINGS.map((ring2) => ({ value: ring2, label: t(`make.resonate.rings.${ring2}`) })), rest: auto, value: resonate.ring })
      ] }),
      card.key === "modulate" && /* @__PURE__ */ jsxs(Fragment2, { children: [
        /* @__PURE__ */ jsx(
          Pills,
          {
            label: t("make.modulate.carrier"),
            onChange: (carrier) => set("modulate", { ...modulate, carrier }),
            options: CARRIERS.filter((carrier) => carrier !== "random").map((carrier) => ({ value: carrier, label: t(`make.modulate.carriers.${carrier}`) })),
            rest: t("make.modulate.carriers.random"),
            value: modulate.carrier === "random" ? void 0 : modulate.carrier
          }
        ),
        /* @__PURE__ */ jsx(
          Toggles,
          {
            label: t("make.modulate.kinds"),
            onChange: (kinds) => set("modulate", { ...modulate, kinds }),
            options: MOD_KINDS.map((kind) => ({ value: kind, label: t(`make.modulate.kindNames.${kind}`), title: t(`make.modulate.kindHints.${kind}`) })),
            rest: t("make.modulate.allKinds"),
            value: modulate.kinds
          }
        )
      ] }),
      (sliders.length > 0 || card.key === "resonate" || card.key === "modulate") && /* @__PURE__ */ jsxs("div", { style: { display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(150px, 1fr))", gap: "10px 16px", alignItems: "start" }, children: [
        sliders,
        card.key === "resonate" && /* @__PURE__ */ jsx(
          Slider,
          {
            ends: [t("make.resonate.short"), t("make.resonate.long")],
            format: (value) => `${percent(value)}%`,
            label: t("make.resonate.decay"),
            onChange: (decay) => set("resonate", { ...resonate, decay }),
            rest: auto,
            span: RESONATE_SPANS.decay,
            value: resonate.decay
          }
        ),
        card.key === "modulate" && /* @__PURE__ */ jsxs(Fragment2, { children: [
          /* @__PURE__ */ jsx(
            Slider,
            {
              format: (value) => String(value),
              label: t("make.modulate.stages"),
              onChange: (stages) => set("modulate", { ...modulate, stages }),
              rest: auto,
              span: MODULATE_SPANS.stages,
              title: t("make.modulate.stagesTitle"),
              value: modulate.stages
            }
          ),
          /* @__PURE__ */ jsx(
            Slider,
            {
              format: (value) => `${percent(value)}%`,
              label: t("make.modulate.retrigger"),
              onChange: (retrigger) => set("modulate", { ...modulate, retrigger }),
              rest: auto,
              span: MODULATE_SPANS.retrigger,
              title: t("make.modulate.retriggerTitle"),
              value: modulate.retrigger
            }
          )
        ] })
      ] })
    ] });
  }
  function LoopSection({ options, tempo, onChange }) {
    const t = useT();
    const auto = t("make.auto");
    const loop = options.loop ?? {};
    const style = loop.style === "melody" ? "melody" : "beat";
    const [open, setOpen] = useState({ rhythm: true });
    const setLoop = (patch) => onChange((current) => ({ ...current, loop: { ...current.loop ?? {}, ...patch } }));
    const control = (name) => {
      const label = t(`make.loop.controls.${name}`);
      if (name === "meter") {
        const chosen = loop.meter ? `${loop.meter[0]}/${loop.meter[1]}` : void 0;
        return /* @__PURE__ */ jsx(
          Pills,
          {
            label,
            onChange: (value) => setLoop({ meter: value === void 0 ? void 0 : value.split("/").map(Number) }),
            options: METERS.map(([beats, unit]) => ({ value: `${beats}/${unit}`, label: `${beats}/${unit}` })),
            rest: auto,
            value: chosen
          },
          name
        );
      }
      if (name === "grid") return /* @__PURE__ */ jsx(Pills, { label, onChange: (grid) => setLoop({ grid }), options: GRIDS.map((grid) => ({ value: grid, label: t(`make.loop.grids.${grid}`) })), rest: auto, value: loop.grid }, name);
      if (name === "key") return /* @__PURE__ */ jsx(Pills, { label, onChange: (key) => setLoop({ key }), options: KEYS.map((key) => ({ value: key, label: key })), rest: auto, value: loop.key }, name);
      if (name === "scale") return /* @__PURE__ */ jsx(Choice, { label, onChange: (scale) => setLoop({ scale }), options: SCALES.map((scale) => ({ value: scale, label: t(`make.loop.scales.${scale}`) })), rest: auto, value: loop.scale }, name);
      if (name === "tuning") return /* @__PURE__ */ jsx(Choice, { label, onChange: (tuning) => setLoop({ tuning }), options: TUNINGS.map((tuning) => ({ value: tuning, label: t(`make.loop.tunings.${tuning}`) })), rest: auto, value: loop.tuning }, name);
      if (name === "contour") return /* @__PURE__ */ jsx(Pills, { label, onChange: (contour) => setLoop({ contour }), options: CONTOURS.map((contour) => ({ value: contour, label: t(`make.loop.contours.${contour}`) })), rest: auto, value: loop.contour }, name);
      const span = LOOP_SPANS[name];
      const format = (value) => name === "register" ? t("make.loop.registerValue", signed2(value)) : name === "octaves" ? t("make.loop.octavesValue", value) : name === "lean" ? `${value < 0 ? t("make.loop.early") : value > 0 ? t("make.loop.late") : ""} ${percent(Math.abs(value))}%`.trim() : `${percent(value)}%`;
      return /* @__PURE__ */ jsx(
        Slider,
        {
          ends: name === "lean" ? [t("make.loop.early"), t("make.loop.late")] : void 0,
          format: (value) => format(value),
          label,
          onChange: (value) => setLoop({ [name]: value }),
          rest: auto,
          span,
          title: t(`make.loop.hints.${name}`),
          value: loop[name]
        },
        name
      );
    };
    const counted = (controls) => controls.filter((name) => loop[name] !== void 0).length;
    return /* @__PURE__ */ jsxs("div", { style: { display: "flex", flexDirection: "column", gap: 10 }, children: [
      /* @__PURE__ */ jsxs("div", { style: { display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(150px, 1fr))", gap: "10px 16px", alignItems: "end" }, children: [
        /* @__PURE__ */ jsx(
          Pills,
          {
            label: t("make.loop.style"),
            onChange: (value) => setLoop({ style: value ?? "beat" }),
            options: [
              { value: "beat", label: t("make.loop.styles.beat"), title: t("make.loop.styleHints.beat") },
              { value: "melody", label: t("make.loop.styles.melody"), title: t("make.loop.styleHints.melody") }
            ],
            rest: null,
            value: style
          }
        ),
        /* @__PURE__ */ jsx(
          Slider,
          {
            format: (value) => t("make.optionValues.bpm", num(value, 0)),
            label: t("make.options.bpm"),
            onChange: (value) => onChange((current) => ({ ...current, bpm: value })),
            rest: t("make.loop.tempoFrom", num(tempo, 0)),
            restAt: tempo,
            span: RANGES.bpm,
            title: t("make.loop.tempoTitle"),
            value: options.bpm
          }
        ),
        /* @__PURE__ */ jsx(
          Slider,
          {
            format: (value) => t("make.optionValues.bars", value),
            label: t("make.options.bars"),
            onChange: (value) => onChange((current) => ({ ...current, bars: value })),
            rest: auto,
            span: RANGES.bars,
            value: options.bars
          }
        )
      ] }),
      LOOP_GROUPS.filter((group) => !group.style || group.style === style).map((group) => {
        const controls = group.controls.filter((name) => {
          const owner = loopStyleOf(name);
          return !owner || owner === style;
        });
        const count = counted(controls);
        return /* @__PURE__ */ jsx(
          Group,
          {
            actions: count > 0 ? /* @__PURE__ */ jsx(TextButton, { onClick: () => setLoop(Object.fromEntries(controls.map((name) => [name, void 0]))), title: t("make.loop.resetTitle"), children: t("make.loop.reset") }) : void 0,
            count: count > 0 ? t("make.steer.count", count) : void 0,
            hint: t(`make.loop.groupHints.${group.key}`),
            onToggle: () => setOpen((current) => ({ ...current, [group.key]: !current[group.key] })),
            open: open[group.key] === true,
            title: t(`make.loop.groups.${group.key}`),
            children: controls.map(control)
          },
          group.key
        );
      })
    ] });
  }
  function SteerSection({ steer, onChange, open, onToggle }) {
    const t = useT();
    const count = steerCount(steer);
    const bipolar = (key) => STEER_SOUND[key].min < 0;
    const valueWords = (key, value) => {
      if (Array.isArray(value)) {
        if (key === "pitch") return t("make.steer.values.semitones", signed2(value[0]), signed2(value[1]));
        if (key === "delay") return t("make.steer.values.ms", value[0], value[1]);
        return t("make.steer.values.percentRange", percent(value[0]), percent(value[1]));
      }
      if (key === "level") return t("make.steer.values.down", Math.round(value * 18));
      if (key === "pan") return t("make.steer.values.pan", percent(value));
      if (key in STEER_SOUND && bipolar(key)) {
        const end = value < 0 ? t(`make.steer.ends.${key}.low`) : t(`make.steer.ends.${key}.high`);
        return `${end} ${percent(Math.abs(value))}%`;
      }
      return `${percent(value)}%`;
    };
    const slider = (key) => {
      const span = key in STEER_SOUND ? STEER_SOUND[key] : STEER_LAYER[key];
      const range = STEER_RANGES.includes(key);
      return /* @__PURE__ */ jsx(
        Slider,
        {
          ends: key in STEER_SOUND && bipolar(key) ? [t(`make.steer.ends.${key}.low`), t(`make.steer.ends.${key}.high`)] : void 0,
          format: (value) => valueWords(key, value),
          label: t(`make.steer.controls.${key}`),
          onChange: (value) => {
            const next = { ...steer };
            next[key] = value;
            onChange(next);
          },
          range,
          rest: "",
          span,
          title: t(`make.steer.hints.${key}`),
          value: range ? steer[key] : steer[key]
        },
        key
      );
    };
    const steered = steerOf(steer) ?? {};
    const summary = Object.keys(steered).map((key) => `${t(`make.steer.controls.${key}`)} ${valueWords(key, steered[key])}`).join(" · ");
    return /* @__PURE__ */ jsxs(
      Group,
      {
        actions: /* @__PURE__ */ jsxs("span", { style: { ...S.row, gap: 4 }, children: [
          /* @__PURE__ */ jsxs(TextButton, { onClick: () => onChange(rollSteer()), title: t("make.steer.rollTitle"), children: [
            /* @__PURE__ */ jsxs("svg", { "aria-hidden": true, height: "10", style: { marginRight: 4, verticalAlign: "-1px" }, viewBox: "0 0 12 12", width: "10", children: [
              /* @__PURE__ */ jsx("rect", { fill: "none", height: "10", rx: "2.5", stroke: "currentColor", strokeWidth: "1.2", width: "10", x: "1", y: "1" }),
              /* @__PURE__ */ jsx("circle", { cx: "4", cy: "4", fill: "currentColor", r: "1" }),
              /* @__PURE__ */ jsx("circle", { cx: "6", cy: "6", fill: "currentColor", r: "1" }),
              /* @__PURE__ */ jsx("circle", { cx: "8", cy: "8", fill: "currentColor", r: "1" })
            ] }),
            t("make.steer.roll")
          ] }),
          count > 0 && /* @__PURE__ */ jsx(TextButton, { onClick: () => onChange({}), title: t("make.steer.resetTitle"), children: t("make.steer.reset") })
        ] }),
        count: count > 0 ? t("make.steer.count", count) : void 0,
        hint: t("make.steer.hint"),
        onToggle,
        open,
        summary,
        title: t("make.steer.title"),
        children: [
          /* @__PURE__ */ jsx("span", { style: { gridColumn: "1 / -1", ...S.heading, fontSize: 10.5 }, children: t("make.steer.sound") }),
          Object.keys(STEER_SOUND).map(slider),
          /* @__PURE__ */ jsx("span", { style: { gridColumn: "1 / -1", ...S.heading, fontSize: 10.5, marginTop: 4 }, title: t("make.steer.layersHint"), children: t("make.steer.layers") }),
          Object.keys(STEER_LAYER).map(slider),
          /* @__PURE__ */ jsx("span", { style: { gridColumn: "1 / -1", fontSize: 10.5, color: T.faint }, children: t("make.steer.layersHint") })
        ]
      }
    );
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
  function Glyph({ card, size = 20 }) {
    return /* @__PURE__ */ jsx("svg", { "aria-hidden": true, height: size, style: { flex: "none" }, viewBox: "0 0 28 20", width: size * 28 / 20, children: GLYPHS[card] });
  }
  var TAB_GLYPH = { cross: "blend", fuse: "smear", loop: "loop", stack: "stack" };
  var SOURCE_CHIPS = 48;
  function Flying({ flight, onDone }) {
    const ghost = useRef(null);
    const done = useRef(onDone);
    done.current = onDone;
    const [origin, setOrigin] = useState(null);
    useLayoutEffect(() => {
      const box = ghost.current?.getBoundingClientRect();
      setOrigin(box ? { left: box.left, top: box.top } : null);
    }, []);
    useEffect(() => {
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
    return /* @__PURE__ */ jsx(
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
    return /* @__PURE__ */ jsxs("div", { ref: box, style: { position: "relative", height: 56, color: T.accent }, children: [
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
  function Stages({ stages }) {
    const t = useT();
    return /* @__PURE__ */ jsx("span", { "aria-label": t("make.stages"), role: "list", style: { ...S.row, flexWrap: "wrap", gap: 4 }, children: stages.map((stage, index) => /* @__PURE__ */ jsxs("span", { role: "listitem", style: { ...S.row, gap: 4 }, children: [
      index > 0 && /* @__PURE__ */ jsx("span", { "aria-hidden": true, style: { color: T.faint, fontSize: 11 }, children: "→" }),
      /* @__PURE__ */ jsx("span", { style: { fontSize: 11, padding: "1px 7px", borderRadius: 8, background: `color-mix(in srgb, ${T.accent} 10%, transparent)`, border: `1px solid color-mix(in srgb, ${T.accent} 30%, transparent)`, color: T.secondary }, children: clean(stage, 80) })
    ] }, index)) });
  }
  function MakePanel({ sources, colorOf: colorOf2, tempo, results, onMake, onClose, playing, onPlay, onStop, onKeep, kept: kept3, use, placement = "bottom", dock, onSources }) {
    const inline = placement === "inline";
    const t = useT();
    const [cardKey, setCardKey] = useState("blend");
    const [lastMode, setLastMode] = useState({ cross: "blend", fuse: "texture", loop: "loop", stack: "stack" });
    const [options, setOptions] = useState({});
    const [steers, setSteers] = useState({});
    const [steerOpen, setSteerOpen] = useState(false);
    const [chosen, setChosen] = useState([]);
    const [trail, setTrail] = useState([]);
    const [busy, setBusy] = useState(null);
    const [error, setError] = useState(null);
    const requests = useRef(/* @__PURE__ */ new Map());
    const [shown, setShown] = useState({});
    const [fresh2, setFresh] = useState(() => /* @__PURE__ */ new Set());
    const [arrived, setArrived] = useState(null);
    const cards = useRef(/* @__PURE__ */ new Map());
    const list = useRef(null);
    const reveal = (element, where) => {
      const box = list.current;
      if (!box || !element) return;
      const [outer, inner] = [box.getBoundingClientRect(), element.getBoundingClientRect()];
      let top = box.scrollTop;
      if (where === "start") top = 0;
      else if (inner.top < outer.top) top += inner.top - outer.top - 8;
      else if (inner.bottom > outer.bottom) top += inner.bottom - outer.bottom + 8;
      else return;
      box.scrollTo({ top, behavior: window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth" });
    };
    useEffect(() => {
      if (!arrived) return;
      reveal(cards.current.get(arrived), "nearest");
      setArrived(null);
    }, [arrived, results]);
    const [flights, setFlights] = useState([]);
    const flightKey = useRef(0);
    const fly = (id) => {
      const canvas = cards.current.get(id)?.querySelector("canvas");
      const to = dock?.current?.getBoundingClientRect();
      if (!canvas || !to || to.width === 0 || window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
      const key = ++flightKey.current;
      setFlights((current) => [...current, { key, from: canvas.getBoundingClientRect(), to, image: canvas.toDataURL() }]);
    };
    const [open, setOpen] = useState(false);
    useEffect(() => {
      const frame2 = requestAnimationFrame(() => setOpen(true));
      return () => cancelAnimationFrame(frame2);
    }, []);
    const card = CARDS.find((entry) => entry.key === cardKey) ?? CARDS[0];
    const note = cardNote(sources.length);
    const refused = note !== null;
    const dawTempo = typeof tempo === "number" && tempo >= 40 && tempo <= 240 ? Math.round(tempo) : DEFAULT_BPM;
    const steer = steers[card.key] ?? {};
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
    const byId = useMemo(() => new Map((results ?? []).map((result) => [result.artifact_id, result])), [results]);
    const chosenResults = chosen.flatMap((id) => byId.has(id) ? [byId.get(id)] : []);
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
    const makeFrom = (next) => {
      if (!onSources || next.length === 0) return;
      setTrail((current) => [...current, sources].slice(-8));
      setChosen([]);
      onSources(next);
      reveal(list.current, "start");
    };
    const tint = `color-mix(in srgb, ${T.accent} 16%, transparent)`;
    const modes = CARDS.filter((entry) => entry.tab === card.tab);
    const chooser = /* @__PURE__ */ jsxs("div", { style: { display: "flex", flexDirection: "column", gap: 7 }, children: [
      /* @__PURE__ */ jsx("div", { "aria-label": t("make.way"), role: "radiogroup", style: { display: "grid", gridTemplateColumns: `repeat(${TABS.length}, 1fr)`, border: `1px solid ${T.border}`, borderRadius: 8, overflow: "hidden" }, children: TABS.map((tab, index) => {
        const picked = card.tab === tab;
        return /* @__PURE__ */ jsxs(
          "button",
          {
            "aria-checked": picked,
            className: FOCUSABLE,
            disabled: refused,
            onClick: () => setCardKey(lastMode[tab]),
            role: "radio",
            title: t(`make.opHints.${tab}`),
            type: "button",
            style: {
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              gap: 6,
              minWidth: 0,
              font: "inherit",
              fontSize: 12.5,
              fontWeight: picked ? 650 : 500,
              padding: "6px 4px",
              border: "none",
              borderLeft: index > 0 ? `1px solid ${T.border}` : "none",
              background: picked ? tint : "transparent",
              color: refused ? T.faint : picked ? T.accent : T.secondary,
              cursor: refused ? "default" : "pointer",
              transition: "background-color 140ms, color 140ms"
            },
            children: [
              /* @__PURE__ */ jsx(Glyph, { card: TAB_GLYPH[tab], size: 14 }),
              /* @__PURE__ */ jsx("span", { style: S.ellipsis, children: t(`make.ops.${tab}`) })
            ]
          },
          tab
        );
      }) }),
      modes.length > 1 && /* @__PURE__ */ jsx("div", { "aria-label": t(`make.ops.${card.tab}`), role: "radiogroup", style: { ...S.row, flexWrap: "wrap", gap: 6 }, children: modes.map((entry) => {
        const picked = entry.key === card.key;
        return /* @__PURE__ */ jsxs(
          "button",
          {
            "aria-checked": picked,
            className: FOCUSABLE,
            disabled: refused,
            onClick: () => {
              setCardKey(entry.key);
              setLastMode((current) => ({ ...current, [entry.tab]: entry.key }));
            },
            role: "radio",
            title: t(`make.cards.${entry.key}.hint`),
            type: "button",
            style: {
              ...S.row,
              gap: 5,
              font: "inherit",
              fontSize: 12,
              fontWeight: picked ? 600 : 500,
              padding: "3px 10px 3px 7px",
              borderRadius: 14,
              border: `1px solid ${picked ? T.accent : T.border}`,
              background: picked ? tint : "transparent",
              boxShadow: picked ? `0 4px 14px -8px ${T.accent}` : "none",
              color: refused ? T.faint : picked ? T.accent : T.secondary,
              cursor: refused ? "default" : "pointer",
              transition: "border-color 140ms, background-color 140ms, color 140ms, box-shadow 200ms"
            },
            children: [
              /* @__PURE__ */ jsx(Glyph, { card: entry.key, size: 13 }),
              t(`make.cards.${entry.key}.title`)
            ]
          },
          entry.key
        );
      }) }),
      /* @__PURE__ */ jsx("span", { style: { ...S.muted, fontSize: 11.5 }, children: t(`make.cards.${card.key}.hint`) })
    ] });
    return /* @__PURE__ */ jsxs(
      "section",
      {
        "aria-label": t("make.title"),
        style: {
          ...placement === "side" ? { position: "absolute", right: 8, top: 52, bottom: 8, width: "min(430px, 46%)" } : placement === "bottom" ? { position: "absolute", left: 8, right: 8, bottom: 8, maxHeight: "82%" } : {},
          display: "flex",
          flexDirection: "column",
          borderRadius: 12,
          border: `1px solid ${T.border}`,
          background: T.raised,
          boxShadow: "0 18px 48px -18px rgba(0,0,0,.55)",
          color: T.text,
          transform: open ? "none" : placement === "side" ? "translateX(24px)" : "translateY(24px)",
          opacity: open ? 1 : 0,
          transition: "transform 260ms cubic-bezier(.2,.8,.2,1), opacity 200ms",
          zIndex: inline ? void 0 : 2
        },
        children: [
          /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 8, padding: "10px 12px 6px", borderBottom: `1px solid ${T.border}` }, children: [
            /* @__PURE__ */ jsx("span", { style: { fontSize: 13.5, fontWeight: 650 }, children: t("make.title") }),
            /* @__PURE__ */ jsx("span", { style: S.faint, children: t("make.from", sources.length) }),
            /* @__PURE__ */ jsx("span", { style: { flex: 1 } }),
            trail.length > 0 && onSources && /* @__PURE__ */ jsxs(
              TextButton,
              {
                onClick: () => {
                  const back = trail[trail.length - 1];
                  setTrail((current) => current.slice(0, -1));
                  setChosen([]);
                  onSources(back);
                },
                title: t("make.backTitle", trail[trail.length - 1].length),
                children: [
                  /* @__PURE__ */ jsx("span", { "aria-hidden": true, children: "‹ " }),
                  t("make.back")
                ]
              }
            ),
            /* @__PURE__ */ jsx(TextButton, { label: t("make.close"), onClick: onClose, title: t("make.close"), children: "×" })
          ] }),
          /* @__PURE__ */ jsxs("div", { ref: list, style: { position: "relative", overflowY: "auto", padding: "8px 12px 12px", display: "flex", flexDirection: "column", gap: 10 }, children: [
            /* @__PURE__ */ jsxs("div", { "aria-label": t("make.sources"), role: "list", style: { ...S.row, flex: "none", flexWrap: "nowrap", gap: 4, overflowX: "auto", paddingBottom: 2, scrollbarWidth: "thin" }, children: [
              sources.slice(0, SOURCE_CHIPS).map((source, index) => /* @__PURE__ */ jsxs("span", { role: "listitem", style: { ...S.row, gap: 4, flex: "none", fontSize: 11, padding: "1px 7px 1px 5px", borderRadius: 10, border: `1px solid ${source.artifact_id ? `color-mix(in srgb, ${T.accent} 45%, transparent)` : T.border}`, color: T.secondary, maxWidth: 180 }, children: [
                source.artifact_id ? /* @__PURE__ */ jsx("span", { "aria-label": t("make.made"), role: "img", style: { color: T.accent, fontSize: 10, flex: "none" }, children: "✦" }) : /* @__PURE__ */ jsx("span", { style: { width: 7, height: 7, borderRadius: "50%", background: colorOf2(source), flex: "none" } }),
                /* @__PURE__ */ jsx("span", { style: S.ellipsis, children: clean(source.name ?? source.sound_id ?? source.artifact_id ?? "", 40) })
              ] }, `${index}-${source.sound_id ?? source.artifact_id}`)),
              sources.length > SOURCE_CHIPS && /* @__PURE__ */ jsx("span", { style: { ...S.faint, flex: "none" }, children: t("make.moreSources", sources.length - SOURCE_CHIPS) })
            ] }),
            chooser,
            card.key === "loop" ? /* @__PURE__ */ jsx(LoopSection, { onChange: setOptions, options, tempo: dawTempo }) : /* @__PURE__ */ jsx(CardOptions, { card, onChange: setOptions, options, sources }),
            /* @__PURE__ */ jsx(SteerSection, { onChange: (next) => setSteers((current) => ({ ...current, [card.key]: next })), onToggle: () => setSteerOpen(!steerOpen), open: steerOpen, steer }),
            /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 8, flexWrap: "wrap" }, children: [
              /* @__PURE__ */ jsx(
                "button",
                {
                  className: FOCUSABLE,
                  disabled: refused || busy !== null,
                  onClick: () => void make(requestFor(card, sources, { ...options, ...card.key === "loop" ? { bpm: options.bpm ?? dawTempo } : {}, steer }), "make"),
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
              note && /* @__PURE__ */ jsx("span", { style: S.faint, children: t(note.key, note.least) }),
              /* @__PURE__ */ jsx("span", { "aria-live": "polite", style: error ? { ...S.faint, color: T.warn, flex: "1 1 14rem" } : S.hidden, children: error ?? "" })
            ] }),
            takes.length > 0 && /* @__PURE__ */ jsx("div", { "aria-label": t("make.results"), role: "list", style: { display: "flex", flexDirection: "column", gap: 8 }, children: takes.map((take) => {
              const at = Math.min(take.results.length - 1, shown[take.key] ?? take.results.length - 1);
              const result = take.results[at];
              const isPlaying = playing === result.artifact_id;
              const inPalette = kept3.has(result.artifact_id);
              const isChosen = chosen.includes(result.artifact_id);
              const stages = result.mode === "modulate" ? stagesOf(result.recipe) : [];
              return /* @__PURE__ */ jsxs(
                "div",
                {
                  className: fresh2.has(result.artifact_id) ? "kumi-pop" : void 0,
                  ref: (element) => {
                    if (element) cards.current.set(result.artifact_id, element);
                    else cards.current.delete(result.artifact_id);
                  },
                  role: "listitem",
                  style: {
                    display: "flex",
                    flexDirection: "column",
                    gap: 5,
                    padding: "8px 10px",
                    borderRadius: 10,
                    border: `1px solid ${isChosen || isPlaying ? T.accent : T.border}`,
                    background: isChosen ? `color-mix(in srgb, ${T.accent} 10%, transparent)` : isPlaying ? `color-mix(in srgb, ${T.accent} 6%, transparent)` : "transparent",
                    boxShadow: isChosen ? `0 0 0 1px ${T.accent}` : "none",
                    transition: "border-color 160ms, background-color 160ms, box-shadow 160ms"
                  },
                  children: [
                    /* @__PURE__ */ jsxs("div", { style: { display: "flex", alignItems: "flex-start", gap: 8 }, children: [
                      /* @__PURE__ */ jsx("span", { style: { ...S.text, flex: 1, fontSize: 12, lineHeight: 1.35, ...stages.length ? {} : { display: "-webkit-box", WebkitLineClamp: 2, WebkitBoxOrient: "vertical", overflow: "hidden" } }, title: result.recipe, children: stages.length ? clean(result.recipe.split(":", 1)[0], 80) : clean(result.recipe, 220) || resultName(result) }),
                      onSources && /* @__PURE__ */ jsx(
                        "button",
                        {
                          "aria-label": t("make.choose"),
                          "aria-pressed": isChosen,
                          className: FOCUSABLE,
                          onClick: () => setChosen((current) => current.includes(result.artifact_id) ? current.filter((id) => id !== result.artifact_id) : [...current, result.artifact_id]),
                          title: t("make.chooseTitle"),
                          type: "button",
                          style: {
                            flex: "none",
                            width: 20,
                            height: 20,
                            display: "flex",
                            alignItems: "center",
                            justifyContent: "center",
                            padding: 0,
                            borderRadius: "50%",
                            border: `1.5px solid ${isChosen ? T.accent : T.faint}`,
                            background: isChosen ? T.accent : "transparent",
                            color: isChosen ? T.raised : "transparent",
                            fontSize: 11,
                            fontWeight: 700,
                            cursor: "pointer",
                            transform: isChosen ? "scale(1.08)" : "none",
                            transition: "background-color 140ms, border-color 140ms, transform 160ms"
                          },
                          children: "✓"
                        }
                      )
                    ] }),
                    stages.length > 0 && /* @__PURE__ */ jsx(Stages, { stages }),
                    /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 8 }, children: [
                      /* @__PURE__ */ jsx(
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
                      /* @__PURE__ */ jsx("div", { "aria-hidden": true, onClick: () => isPlaying ? onStop() : onPlay(result.artifact_id), style: { flex: 1, minWidth: 0, cursor: "pointer" }, children: /* @__PURE__ */ jsx(Wave, { duration: result.duration, peaks: result.peaks, playing: isPlaying }) })
                    ] }),
                    /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 6, flexWrap: "wrap" }, children: [
                      /* @__PURE__ */ jsxs("span", { style: S.faint, children: [
                        t(`make.ops.${result.mode === "loop" ? "loop" : result.op}`),
                        result.mode && result.mode !== "loop" ? ` · ${t(`make.cards.${result.mode}.title`)}` : "",
                        " · ",
                        seconds(result.duration)
                      ] }),
                      /* @__PURE__ */ jsxs("span", { style: { ...S.faint, fontSize: 10.5, fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace" }, title: t("make.seedTitle"), children: [
                        "#",
                        result.seed
                      ] }),
                      /* @__PURE__ */ jsx("span", { style: { flex: 1 } }),
                      take.results.length > 1 && /* @__PURE__ */ jsxs("span", { role: "group", "aria-label": t("make.variations"), style: { ...S.row, gap: 2 }, children: [
                        /* @__PURE__ */ jsx(TextButton, { disabled: at === 0, label: t("make.previous"), onClick: () => setShown((current) => ({ ...current, [take.key]: at - 1 })), children: "‹" }),
                        /* @__PURE__ */ jsx("span", { style: { ...S.faint, fontVariantNumeric: "tabular-nums" }, children: t("make.variation", at + 1, take.results.length) }),
                        /* @__PURE__ */ jsx(TextButton, { disabled: at === take.results.length - 1, label: t("make.next"), onClick: () => setShown((current) => ({ ...current, [take.key]: at + 1 })), children: "›" })
                      ] })
                    ] }),
                    /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 6, flexWrap: "wrap" }, children: [
                      /* @__PURE__ */ jsx(TextButton, { disabled: busy !== null, onClick: () => void make(reroll(take.request), take.key), title: t("make.rerollTitle"), children: busy === take.key ? t("make.rerolling") : /* @__PURE__ */ jsxs(Fragment2, { children: [
                        /* @__PURE__ */ jsx("span", { "aria-hidden": true, children: "↻ " }),
                        t("make.reroll")
                      ] }) }),
                      /* @__PURE__ */ jsxs(
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
                            /* @__PURE__ */ jsx("span", { "aria-hidden": true, children: inPalette ? "✓ " : "+ " }),
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
            takes.length === 0 && results !== null && /* @__PURE__ */ jsx("span", { style: S.faint, children: t("make.none") })
          ] }),
          onSources && chosenResults.length > 0 && /* @__PURE__ */ jsxs(
            "div",
            {
              "aria-label": t("make.chosen", chosenResults.length),
              className: "kumi-pop",
              role: "group",
              style: { ...S.row, flexWrap: "wrap", gap: 6, padding: "8px 12px", borderTop: `1px solid ${T.border}`, background: `color-mix(in srgb, ${T.accent} 7%, ${T.raised})`, borderRadius: "0 0 12px 12px" },
              children: [
                /* @__PURE__ */ jsx("span", { style: { ...S.text, fontWeight: 600, flex: "1 1 6rem" }, children: t("make.chosen", chosenResults.length) }),
                /* @__PURE__ */ jsx(TextButton, { onClick: () => makeFrom(chosenResults.map(sourceOfResult)), strong: true, title: t("make.fromTheseTitle"), children: t("make.fromThese") }),
                /* @__PURE__ */ jsx(
                  TextButton,
                  {
                    onClick: () => makeFrom([...sources, ...chosenResults.filter((result) => !sources.some((source) => source.artifact_id === result.artifact_id)).map(sourceOfResult)]),
                    title: t("make.addToSoundsTitle"),
                    children: t("make.addToSounds")
                  }
                ),
                /* @__PURE__ */ jsx(TextButton, { onClick: () => setChosen([]), title: t("make.clearChosenTitle"), children: t("make.clearChosen") })
              ]
            }
          ),
          flights.map((flight) => /* @__PURE__ */ jsx(Flying, { flight, onDone: () => setFlights((current) => current.filter((other) => other.key !== flight.key)) }, flight.key))
        ]
      }
    );
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
  function itemReference(item, revision) {
    if (item.sound_id) return soundRequest(item.sound_id, item.name, revision);
    return item.artifact_id ? artifactReference(item.artifact_id, item.name) : null;
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

  // src/shared/peaks.ts
  var DECODE_MOST = 12 * 1024 * 1024;
  var kept = /* @__PURE__ */ new Map();
  var loading = /* @__PURE__ */ new Map();
  var askers = /* @__PURE__ */ new Map();
  var queue = Promise.resolve();
  var SKIPPED = /* @__PURE__ */ Symbol("skipped");
  function peaksFor(id, bars, audio, wanted = () => true) {
    const key = `${bars}:${id}`;
    const done = kept.get(key);
    if (done !== void 0) return Promise.resolve(done);
    const running = loading.get(key);
    if (running) {
      askers.get(key)?.push(wanted);
      return running;
    }
    askers.set(key, [wanted]);
    const run = async () => {
      if (!(askers.get(key) ?? []).some((ask) => ask())) return SKIPPED;
      const url = await audio(id);
      const comma = url.indexOf(",");
      if (comma < 0 || url.length > DECODE_MOST || typeof OfflineAudioContext !== "function") return null;
      const binary = atob(url.slice(comma + 1));
      const bytes = new Uint8Array(binary.length);
      for (let at = 0; at < binary.length; at++) bytes[at] = binary.charCodeAt(at);
      const buffer = await new OfflineAudioContext(1, 1, 44100).decodeAudioData(bytes.buffer);
      const mixed2 = new Float32Array(buffer.length);
      for (let channel = 0; channel < buffer.numberOfChannels; channel++) {
        const data = buffer.getChannelData(channel);
        for (let at = 0; at < data.length; at++) mixed2[at] += data[at];
      }
      return peaksOf(mixed2, bars);
    };
    const promise = queue.then(run, run).catch(() => null).then((peaks) => {
      loading.delete(key);
      askers.delete(key);
      if (peaks === SKIPPED) return null;
      kept.set(key, peaks);
      return peaks;
    });
    queue = promise;
    loading.set(key, promise);
    return promise;
  }
  function peaksKept(id, bars) {
    return kept.get(`${bars}:${id}`) ?? null;
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
  var ring = (count, radius, turn = -Math.PI / 2) => Array.from({ length: count }, (_, index) => [Math.cos(turn + index * 2 * Math.PI / count) * radius, Math.sin(turn + index * 2 * Math.PI / count) * radius]);
  var GLYPH_SHAPES = [
    null,
    [
      [0, -1.3],
      [1.1, 0],
      [0, 1.3],
      [-1.1, 0]
    ],
    [
      [0, -1.35],
      [1.2, 0.75],
      [-1.2, 0.75]
    ],
    [
      [-0.92, -0.92],
      [0.92, -0.92],
      [0.92, 0.92],
      [-0.92, 0.92]
    ],
    ring(6, 1.08),
    ring(8, 1, -Math.PI / 2).map(([x, y], index) => index % 2 ? [x * 0.5, y * 0.5] : [x * 1.4, y * 1.4]),
    [
      [0, 1.35],
      [1.2, -0.75],
      [-1.2, -0.75]
    ],
    [
      [-0.4, -1.15],
      [0.4, -1.15],
      [0.4, -0.4],
      [1.15, -0.4],
      [1.15, 0.4],
      [0.4, 0.4],
      [0.4, 1.15],
      [-0.4, 1.15],
      [-0.4, 0.4],
      [-1.15, 0.4],
      [-1.15, -0.4],
      [-0.4, -0.4]
    ]
  ];
  function glyphOf(group) {
    return (group % GLYPH_SHAPES.length + GLYPH_SHAPES.length) % GLYPH_SHAPES.length;
  }
  function traceGlyph(path, glyph, x, y, r) {
    const shape = GLYPH_SHAPES[glyph];
    if (!shape) {
      path.moveTo(x + r, y);
      path.arc(x, y, r, 0, Math.PI * 2);
      return;
    }
    shape.forEach(([px, py], index) => index === 0 ? path.moveTo(x + px * r, y + py * r) : path.lineTo(x + px * r, y + py * r));
    path.closePath();
  }
  var cellOf = (x, y) => Math.min(GRID - 1, Math.floor(y * GRID)) * GRID + Math.min(GRID - 1, Math.floor(x * GRID));
  var MOST_ARRANGEMENTS = 16;
  var MOST_LABELS = 400;
  var unitOf = (value) => typeof value === "number" && Number.isFinite(value) ? Math.min(1, Math.max(0, value)) : NaN;
  function buildModel(map) {
    const arrangements = (Array.isArray(map?.arrangements) ? map.arrangements : []).slice(0, MOST_ARRANGEMENTS).map((raw) => ({
      id: typeof raw?.id === "string" ? clean(raw.id, 24) : "",
      labels: (Array.isArray(raw?.labels) ? raw.labels : []).slice(0, MOST_LABELS).filter((label) => Array.isArray(label) && typeof label[0] === "string" && Number.isFinite(label[1]) && Number.isFinite(label[2])).map(([text, x, y]) => ({ text: clean(text, 40), x: unitOf(x), y: unitOf(y) }))
    }));
    const placed = arrangements.map(() => []);
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
      placed.forEach((values, k) => values.push(unitOf(raw[4 + 2 * k]), unitOf(raw[5 + 2 * k])));
    }
    const groups = Object.entries(map?.groups ?? {}).map(([key, group]) => ({
      key: Number(key),
      name: clean(group?.name ?? `group ${key}`, 32),
      color: groupColor(group?.color, Number(key)),
      count: typeof group?.count === "number" ? group.count : points.filter((point) => point.group === Number(key)).length
    })).filter((group) => Number.isInteger(group.key)).sort((a, b) => a.key - b.key);
    return { revision: map?.revision ?? null, points, groups, sounds: map?.sounds ?? {}, cells, arrangements, places: placed.map((values) => Float32Array.from(values)) };
  }
  function arrangedModel(model, k) {
    const place = model.places[k];
    if (!place) return model;
    const cells = /* @__PURE__ */ new Map();
    const points = model.points.map((point, index) => {
      const [x, y] = [place[2 * index], place[2 * index + 1]];
      const at = Number.isNaN(x) || Number.isNaN(y) ? point : { ...point, x, y };
      const cell = cellOf(at.x, at.y);
      const list = cells.get(cell);
      if (list) list.push(index);
      else cells.set(cell, [index]);
      return at;
    });
    return { ...model, points, cells };
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
  function mapSide(width, height, top = 0) {
    return Math.max(1, Math.min(width, height - top) - 32);
  }
  var roomBeside = (width, right) => width - right >= 240 ? width - right : width;
  function fitView(width, height, top = 0, right = 0) {
    const size = mapSide(width, height, top);
    const room = roomBeside(width, right);
    return { scale: 1, dx: size <= room ? (room - size) / 2 : (width - size) / 2, dy: top + (height - top - size) / 2 };
  }
  function floatBeside(box, w, h, width, height, top, bottom) {
    const GAP = 12;
    const EDGE = 8;
    const floor = height - bottom - h;
    const docked = { left: Math.max(EDGE, (width - w) / 2), top: Math.max(top, floor) };
    if (!box) return docked;
    const [x0, y0, x1, y1] = box;
    const beside = Math.min(Math.max(top, y0 - 6), Math.max(top, floor));
    const across = Math.min(Math.max(EDGE, (x0 + x1) / 2 - w / 2), Math.max(EDGE, width - w - EDGE));
    if (x1 + GAP + w <= width - EDGE) return { left: x1 + GAP, top: beside };
    if (x0 - GAP - w >= EDGE) return { left: x0 - GAP - w, top: beside };
    if (y0 - GAP - h >= top) return { left: across, top: y0 - GAP - h };
    if (y1 + GAP <= floor) return { left: across, top: y1 + GAP };
    return docked;
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
    else if (typeof legacy === "number" && legacy !== 0 && delta !== 0) memory.pans = Math.abs(legacy + 3 * delta) <= 1.5;
    else memory.pans = true;
    return memory.pans;
  }
  function pick2(model, view, size, px, py, radius = 8, visible) {
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
  function frame(model, ids, width, height, top = 0, right = 0, margin = 48) {
    const chosen = model.points.filter((point) => ids.has(point.id));
    if (!chosen.length) return null;
    const xs = chosen.map((point) => point.x);
    const ys = chosen.map((point) => point.y);
    const [x0, x1, y0, y1] = [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)];
    const size = mapSide(width, height, top);
    const room = roomBeside(width, right);
    const span = Math.max(x1 - x0, y1 - y0, 0.04);
    const scale = Math.min(40, Math.max(1, (Math.min(room, height - top) - 2 * margin) / (span * size)));
    const cx = (x0 + x1) / 2;
    const cy = (y0 + y1) / 2;
    return { scale, dx: room / 2 - cx * size * scale, dy: top + (height - top) / 2 - cy * size * scale };
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

  // src/shared/atlas/color.ts
  function rgbOf(color) {
    const text = color.trim();
    const hex = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(text);
    if (hex) {
      const digits = hex[1].length === 3 ? [...hex[1]].map((digit) => digit + digit).join("") : hex[1];
      const value = parseInt(digits, 16);
      return [value >> 16, value >> 8 & 255, value & 255];
    }
    const rgb = /^rgba?\(\s*(\d{1,3})[\s,]+(\d{1,3})[\s,]+(\d{1,3})\s*(?:[,/]\s*[\d.]+%?\s*)?\)$/i.exec(text);
    if (!rgb) return null;
    const channels = [Number(rgb[1]), Number(rgb[2]), Number(rgb[3])];
    return channels.every((channel) => channel <= 255) ? channels : null;
  }
  function rgba(color, alpha) {
    return `rgba(${color[0]}, ${color[1]}, ${color[2]}, ${alpha})`;
  }
  function lighten(color, amount) {
    const toward = (channel) => Math.round(channel + (255 - channel) * amount);
    return `rgb(${toward(color[0])}, ${toward(color[1])}, ${toward(color[2])})`;
  }
  function isDark(color) {
    return 0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2] < 128;
  }

  // src/shared/atlas/pulse.ts
  var WAVE_BARS = 160;
  function levelAt(levels, progress) {
    if (!levels || levels.length === 0 || !(progress >= 0) || progress > 1) return 0;
    const at = Math.min(levels.length - 1, progress * levels.length);
    const before = Math.floor(at);
    const after = Math.min(levels.length - 1, before + 1);
    return levels[before] + (levels[after] - levels[before]) * (at - before);
  }
  function follow(shown, level, elapsed) {
    return level >= shown ? level : Math.max(level, shown - elapsed / 220);
  }
  function progressOf(elapsed, duration) {
    if (!(duration > 0) || elapsed < 0) return null;
    const progress = elapsed / 1e3 / duration;
    return progress <= 1 ? progress : null;
  }

  // src/shared/atlas/overlay.ts
  var FADE_MS = 180;
  var RIPPLE_MS = 1100;
  var Overlay = class {
    constructor(canvas) {
      this.canvas = canvas;
      this.context = canvas.getContext("2d");
    }
    canvas;
    context;
    width = 0;
    height = 0;
    ratio = 1;
    frame = 0;
    pulse = null;
    /** The pulse that just stopped, fading where it was, and when it stopped. */
    fading = null;
    /** The core's colour: the canvas's own CSS colour (the theme's text colour), read when a sound starts. */
    core = "#fff";
    still = false;
    resize(width, height, ratio) {
      if (width === this.width && height === this.height && ratio === this.ratio) return;
      [this.width, this.height, this.ratio] = [width, height, ratio];
      this.canvas.width = Math.round(width * ratio);
      this.canvas.height = Math.round(height * ratio);
      this.draw(performance.now());
    }
    /** A sound starts playing (its pulse begins), or the sound stops (null: its pulse fades). */
    play(pulse) {
      const now = performance.now();
      if (this.pulse && !pulse && !this.still) this.fading = { pulse: this.pulse, shown: this.pulse.shown, since: now };
      this.pulse = pulse ? { ...pulse, since: now, shown: 0, at: now } : null;
      if (pulse) this.core = getComputedStyle(this.canvas).color || this.core;
      if (pulse) this.fading = null;
      this.wake();
    }
    /** The playing sound's waveform arrived. */
    levels(levels) {
      if (this.pulse) this.pulse.levels = levels;
    }
    /** The map moved under a still pulse (a pan, a zoom, a resize): draw it where the sound is now. */
    moved() {
      if (this.frame === 0 && (this.pulse || this.fading)) this.draw(performance.now());
    }
    dispose() {
      cancelAnimationFrame(this.frame);
      this.frame = 0;
      this.pulse = null;
      this.fading = null;
    }
    wake() {
      if (this.still) {
        this.draw(performance.now());
        return;
      }
      if (this.frame === 0) this.frame = requestAnimationFrame(this.tick);
    }
    tick = (now) => {
      this.frame = 0;
      this.draw(now);
      if (this.pulse || this.fading) this.frame = requestAnimationFrame(this.tick);
    };
    draw(now) {
      const context = this.context;
      if (!context || this.width === 0) return;
      context.setTransform(this.ratio, 0, 0, this.ratio, 0, 0);
      context.clearRect(0, 0, this.width, this.height);
      if (this.fading) {
        const left = 1 - (now - this.fading.since) / FADE_MS;
        if (left <= 0) this.fading = null;
        else this.ring(context, this.fading.pulse, this.fading.shown, left, null);
      }
      const pulse = this.pulse;
      if (!pulse) return;
      const progress = progressOf(now - pulse.since, pulse.duration);
      if (pulse.levels && progress !== null && !this.still) {
        pulse.shown = follow(pulse.shown, levelAt(pulse.levels, progress), now - pulse.at);
        pulse.at = now;
        this.ring(context, pulse, pulse.shown, 1, null);
      } else {
        this.ring(context, pulse, 0.35, 1, this.still ? -1 : (now - pulse.since) / RIPPLE_MS);
      }
    }
    /** One pulse: a glow and three rings reaching as far as `level`, and the sound's own shape bright at the middle.
     *  With `ripple` (a phase), two rings spread from it instead (the plain pulse); a negative one holds them still. */
    ring(context, pulse, level, strength, ripple) {
      const place = pulse.place();
      if (!place) return;
      const [x, y] = place;
      if (x < -60 || y < -60 || x > this.width + 60 || y > this.height + 60) return;
      const channels = rgbOf(pulse.color);
      context.save();
      context.globalCompositeOperation = "lighter";
      if (channels) {
        const reach = 26 + 30 * level;
        const glow = context.createRadialGradient(x, y, 0, x, y, reach);
        glow.addColorStop(0, rgba(channels, 1));
        glow.addColorStop(0.35, rgba(channels, 0.45));
        glow.addColorStop(1, rgba(channels, 0));
        context.globalAlpha = (0.5 + 0.4 * level) * strength;
        context.fillStyle = glow;
        context.fillRect(x - reach, y - reach, reach * 2, reach * 2);
      }
      context.strokeStyle = channels ? lighten(channels, 0.3) : pulse.color;
      if (ripple === null) {
        context.lineWidth = 1.75;
        for (let ring2 = 0; ring2 < 3; ring2++) {
          context.globalAlpha = (0.9 - ring2 * 0.25) * strength;
          context.beginPath();
          context.arc(x, y, 9 + ring2 * (7 + 9 * level), 0, Math.PI * 2);
          context.stroke();
        }
      } else {
        for (const offset of ripple < 0 ? [0.25] : [0, 0.5]) {
          const phase = ripple < 0 ? offset : ((ripple + offset) % 1 + 1) % 1;
          context.globalAlpha = 0.7 * (1 - phase) ** 1.5 * strength;
          context.lineWidth = 2 - phase;
          context.beginPath();
          context.arc(x, y, 8 + phase * 26, 0, Math.PI * 2);
          context.stroke();
        }
      }
      context.restore();
      const core = new Path2D();
      traceGlyph(core, pulse.glyph, x, y, 4);
      context.globalAlpha = strength;
      context.fillStyle = this.core;
      context.fill(core);
      context.globalAlpha = 1;
    }
  };

  // src/shared/atlas/AtlasOverlay.tsx
  function AtlasOverlay({
    width,
    height,
    playing,
    place,
    audio,
    still,
    moved: moved2
  }) {
    const canvas = useRef(null);
    const overlay = useRef(null);
    const placed = useRef(place);
    placed.current = place;
    const fetched = useRef(audio);
    fetched.current = audio;
    useEffect(() => {
      if (!canvas.current) return;
      const made = new Overlay(canvas.current);
      overlay.current = made;
      return () => {
        made.dispose();
        overlay.current = null;
      };
    }, []);
    useEffect(() => {
      if (overlay.current) overlay.current.still = still;
    }, [still]);
    useEffect(() => overlay.current?.resize(width, height, window.devicePixelRatio || 1), [width, height]);
    useEffect(() => overlay.current?.moved(), [moved2]);
    const [id, color, glyph, duration] = [playing?.id ?? null, playing?.color ?? "", playing?.glyph ?? 0, playing?.duration ?? 0];
    useEffect(() => {
      const drawn = overlay.current;
      if (!drawn) return;
      if (id === null) {
        drawn.play(null);
        return;
      }
      let live = true;
      drawn.play({ place: () => placed.current(), color, glyph, duration, levels: peaksKept(id, WAVE_BARS) });
      if (fetched.current) void peaksFor(id, WAVE_BARS, fetched.current, () => live).then((levels) => live && drawn.levels(levels));
      return () => {
        live = false;
      };
    }, [id, color, glyph, duration]);
    return /* @__PURE__ */ jsx("canvas", { "aria-hidden": true, ref: canvas, style: { position: "absolute", inset: 0, width: "100%", height: "100%", pointerEvents: "none", color: T.text } });
  }

  // src/shared/atlas/nebula.ts
  var NEBULA_SIDE = 128;
  var NEBULA_PAD = 0.12;
  var SPREAD = 2;
  var SKIRT = 5;
  var CORE = 0.55;
  var ALONE = CORE / (2 * SPREAD + 1) ** 2;
  var CROWD = 8;
  var BRIGHTEST = 0.85;
  function blur(from, into, side, radius, step2) {
    const across = step2 === 1 ? side : 1;
    const scale = 1 / (2 * radius + 1);
    for (let line = 0; line < side; line++) {
      const start = line * across;
      let sum = 0;
      for (let k = 0; k <= radius && k < side; k++) sum += from[start + k * step2];
      for (let k = 0; k < side; k++) {
        into[start + k * step2] = sum * scale;
        const enters = k + radius + 1;
        const leaves = k - radius;
        if (enters < side) sum += from[start + enters * step2];
        if (leaves >= 0) sum -= from[start + leaves * step2];
      }
    }
  }
  function nebulaField(points, keep, colorOf2, side = NEBULA_SIDE) {
    const cells = side * side;
    const sums = [new Float32Array(cells), new Float32Array(cells), new Float32Array(cells), new Float32Array(cells)];
    const [red, green, blue, weight] = sums;
    const span = side / (1 + 2 * NEBULA_PAD);
    const colors = /* @__PURE__ */ new Map();
    for (let index = 0; index < points.length; index++) {
      if (!keep(index)) continue;
      const point = points[index];
      let color = colors.get(point.group);
      if (!color) {
        color = colorOf2(point.group);
        colors.set(point.group, color);
      }
      const fx = (point.x + NEBULA_PAD) * span - 0.5;
      const fy = (point.y + NEBULA_PAD) * span - 0.5;
      const x0 = Math.floor(fx);
      const y0 = Math.floor(fy);
      if (x0 < 0 || y0 < 0 || x0 + 1 >= side || y0 + 1 >= side) continue;
      const right = fx - x0;
      const down = fy - y0;
      for (let corner = 0; corner < 4; corner++) {
        const share = (corner & 1 ? right : 1 - right) * (corner & 2 ? down : 1 - down);
        const cell = (y0 + (corner >> 1)) * side + x0 + (corner & 1);
        red[cell] += color[0] * share;
        green[cell] += color[1] * share;
        blue[cell] += color[2] * share;
        weight[cell] += share;
      }
    }
    const spare = new Float32Array(cells);
    const core = new Float32Array(cells);
    for (const sum of sums) {
      for (let pass = 0; pass < 2; pass++) {
        blur(sum, spare, side, SPREAD, 1);
        blur(spare, sum, side, SPREAD, side);
      }
      core.set(sum);
      for (let pass = 0; pass < 2; pass++) {
        blur(sum, spare, side, SKIRT, 1);
        blur(spare, sum, side, SKIRT, side);
      }
      for (let cell = 0; cell < cells; cell++) sum[cell] = CORE * core[cell] + (1 - CORE) * sum[cell];
    }
    let peak = 0;
    for (let cell = 0; cell < cells; cell++) if (weight[cell] > peak) peak = weight[cell];
    const pixels = new Uint8ClampedArray(cells * 4);
    if (peak <= 0) return pixels;
    const gain = -Math.log(1 - BRIGHTEST) / Math.max(peak, ALONE * CROWD);
    for (let cell = 0; cell < cells; cell++) {
      const here = weight[cell];
      if (here <= 1e-9) continue;
      pixels[4 * cell] = red[cell] / here;
      pixels[4 * cell + 1] = green[cell] / here;
      pixels[4 * cell + 2] = blue[cell] / here;
      pixels[4 * cell + 3] = 255 * (1 - Math.exp(-here * gain));
    }
    return pixels;
  }
  function nebulaAlpha(zoom) {
    const alpha = 1 / Math.max(1, zoom) ** 1.35;
    return alpha < 0.03 ? 0 : alpha;
  }
  function haloOf(radius, zoom, crowd, unit) {
    const reach = radius * 3.3;
    const overlap = crowd * 64 * 64 / (unit * unit) * 0.77 * reach * reach;
    return { radius: reach, alpha: Math.min(0.34 / Math.sqrt(Math.max(1, zoom)), overlap > 0 ? Math.max(0.16, 0.9 / overlap) : 1) };
  }

  // src/shared/atlas/glow.ts
  var GREY = [128, 128, 128];
  var MOST_SPRITES = 240;
  function picture(width, height) {
    if (typeof OffscreenCanvas !== "function") return null;
    const canvas = new OffscreenCanvas(width, height);
    const context = canvas.getContext("2d");
    return context ? { canvas, context } : null;
  }
  var Glow = class {
    current = null;
    last = null;
    sprites = /* @__PURE__ */ new Map();
    crowds = /* @__PURE__ */ new WeakMap();
    shapes = /* @__PURE__ */ new Map();
    ground = "";
    dark = true;
    /** Whether the canvas's ground is a dark one, from the theme colour the canvas resolved (a colour it can't read
     *  counts as dark, as both hosts' first themes are). */
    onDark(context, ground) {
      if (ground === this.ground) return this.dark;
      this.ground = ground;
      const kept3 = context.fillStyle;
      context.fillStyle = "#000";
      context.fillStyle = ground;
      const read = typeof context.fillStyle === "string" ? rgbOf(context.fillStyle) : null;
      context.fillStyle = kept3;
      this.dark = read ? isDark(read) : true;
      this.shapes.clear();
      return this.dark;
    }
    /** The colour a sound's shape is filled in: its family's, a little brighter on a dark ground so it stays crisp over
     *  its own glow. */
    shape(color) {
      let shape = this.shapes.get(color);
      if (shape === void 0) {
        const channels = this.dark ? rgbOf(color) : null;
        shape = channels ? lighten(channels, 0.2) : color;
        this.shapes.set(color, shape);
      }
      return shape;
    }
    /** The nebula of `layout` for the sounds `mask` keeps, made the first time it's asked for and kept until either
     *  changes; with it, the one the map showed before a new layout (for the move between them to fade out). Two
     *  pictures in all, drawn over in turn. */
    nebula(layout, mask, colorOf2) {
      if (this.current?.layout !== layout || this.current.mask !== mask) {
        const same = this.current?.layout === layout;
        const sheet = (same ? this.current?.sheet : this.last?.sheet) ?? picture(NEBULA_SIDE, NEBULA_SIDE);
        if (!sheet) return { now: null, before: null };
        if (!same) this.last = this.current;
        const pixels = nebulaField(
          layout.points,
          (index) => mask[index] === 1,
          (group) => rgbOf(colorOf2(group)) ?? GREY
        );
        sheet.context.putImageData(new ImageData(pixels, NEBULA_SIDE, NEBULA_SIDE), 0, 0);
        this.current = { layout, mask, sheet };
      }
      return { now: this.current.sheet.canvas, before: this.last && this.last.layout !== layout ? this.last.sheet.canvas : null };
    }
    /** Draw a nebula under the map at `view`, `unit` pixels to the map's square, at `alpha`. */
    draw(context, nebula, view, unit, alpha) {
      if (alpha <= 0) return;
      const kept3 = context.globalAlpha;
      context.globalAlpha = alpha;
      context.imageSmoothingEnabled = true;
      context.drawImage(nebula, view.dx - NEBULA_PAD * unit, view.dy - NEBULA_PAD * unit, (1 + 2 * NEBULA_PAD) * unit, (1 + 2 * NEBULA_PAD) * unit);
      context.globalAlpha = kept3;
    }
    /** How many sounds the layout's most crowded cell holds (of its 64 × 64 grid). */
    crowd(layout) {
      let crowd = this.crowds.get(layout);
      if (crowd === void 0) {
        crowd = 0;
        for (const cell of layout.cells.values()) crowd = Math.max(crowd, cell.length);
        this.crowds.set(layout, crowd);
      }
      return crowd;
    }
    /** Halos of `radius` pixels at `alpha`, on a canvas drawn at `ratio` device pixels to one: each `at` adds one, `done`
     *  hands the canvas back as it was. Null when there's nothing to draw them with, or nothing to see. */
    halos(context, radius, alpha, ratio) {
      if (alpha < 0.02 || radius < 1 || typeof OffscreenCanvas !== "function") return null;
      const size = Math.round(radius * 2) / 2;
      const side = Math.max(2, Math.ceil(size * 2 * ratio));
      const [keptAlpha, keptBlend] = [context.globalAlpha, context.globalCompositeOperation];
      context.globalCompositeOperation = "lighter";
      let strong = -1;
      let color = "";
      let sprite = null;
      return {
        at: (next, x, y, strength) => {
          if (strength <= 0.02) return;
          if (next !== color) {
            color = next;
            sprite = this.sprite(next, side);
          }
          if (!sprite) return;
          const stepped = Math.round(Math.min(1, strength) * 10) / 10;
          if (stepped !== strong) {
            strong = stepped;
            context.globalAlpha = alpha * stepped;
          }
          context.drawImage(sprite, x - size, y - size, size * 2, size * 2);
        },
        done: () => {
          context.globalAlpha = keptAlpha;
          context.globalCompositeOperation = keptBlend;
        }
      };
    }
    /** A soft round sprite in one colour, `side` device pixels across: bright in the middle, gone at its edge. */
    sprite(color, side) {
      const key = `${color}|${side}`;
      const kept3 = this.sprites.get(key);
      if (kept3) return kept3;
      const channels = rgbOf(color);
      const made = channels ? picture(side, side) : null;
      if (!made || !channels) return null;
      const middle = side / 2;
      const fade = made.context.createRadialGradient(middle, middle, 0, middle, middle, middle);
      fade.addColorStop(0, rgba(channels, 1));
      fade.addColorStop(0.35, rgba(channels, 0.45));
      fade.addColorStop(1, rgba(channels, 0));
      made.context.fillStyle = fade;
      made.context.fillRect(0, 0, side, side);
      if (this.sprites.size >= MOST_SPRITES) this.sprites.clear();
      this.sprites.set(key, made.canvas);
      return made.canvas;
    }
  };
  var glows = /* @__PURE__ */ new WeakMap();
  function glowOf(canvas) {
    let glow = glows.get(canvas);
    if (!glow) {
      glow = new Glow();
      glows.set(canvas, glow);
    }
    return glow;
  }

  // src/shared/mapUi.tsx
  var glass = {
    background: `color-mix(in srgb, ${T.raised} 90%, transparent)`,
    backdropFilter: "blur(12px) saturate(1.15)",
    WebkitBackdropFilter: "blur(12px) saturate(1.15)",
    border: `1px solid color-mix(in srgb, ${T.border} 85%, transparent)`,
    boxShadow: "0 10px 30px -12px rgba(0,0,0,.55), 0 1px 0 rgba(255,255,255,.03) inset",
    borderRadius: 12
  };
  var PATHS = {
    pick: /* @__PURE__ */ jsx("path", { d: "M4 2.5 L4 13 L6.8 10.4 L8.7 14.5 L10.4 13.7 L8.6 9.7 L12.4 9.5 Z", fill: "currentColor", strokeLinejoin: "round" }),
    hand: /* @__PURE__ */ jsx(
      "path",
      {
        d: "M5.2 8.5 V3.8 a1 1 0 0 1 2 0 V7.6 V2.9 a1 1 0 0 1 2 0 V7.6 V3.6 a1 1 0 0 1 2 0 V8 V5.4 a1 1 0 0 1 2 0 V9.6 c0 2.6 -1.8 4.6 -4.4 4.6 c-1.7 0 -2.7 -.7 -3.6 -1.9 L2.6 9.6 a1 1 0 0 1 1.5 -1.3 Z",
        fill: "none",
        stroke: "currentColor",
        strokeLinejoin: "round",
        strokeWidth: "1.2"
      }
    ),
    tiles: /* @__PURE__ */ jsxs(Fragment2, { children: [
      /* @__PURE__ */ jsx("rect", { fill: "currentColor", height: "4.6", rx: "1.2", width: "4.6", x: "2", y: "2" }),
      /* @__PURE__ */ jsx("rect", { fill: "currentColor", fillOpacity: ".7", height: "4.6", rx: "1.2", width: "4.6", x: "9.4", y: "2" }),
      /* @__PURE__ */ jsx("rect", { fill: "currentColor", fillOpacity: ".7", height: "4.6", rx: "1.2", width: "4.6", x: "2", y: "9.4" }),
      /* @__PURE__ */ jsx("rect", { fill: "currentColor", fillOpacity: ".45", height: "4.6", rx: "1.2", width: "4.6", x: "9.4", y: "9.4" })
    ] }),
    hover: /* @__PURE__ */ jsxs(Fragment2, { children: [
      /* @__PURE__ */ jsx("path", { d: "M3 9.5 V8 a5 5 0 0 1 10 0 V9.5", fill: "none", stroke: "currentColor", strokeLinecap: "round", strokeWidth: "1.3" }),
      /* @__PURE__ */ jsx("rect", { fill: "currentColor", height: "4.6", rx: "1.2", width: "2.8", x: "2.2", y: "9" }),
      /* @__PURE__ */ jsx("rect", { fill: "currentColor", height: "4.6", rx: "1.2", width: "2.8", x: "11", y: "9" })
    ] }),
    fit: /* @__PURE__ */ jsx("path", { d: "M2.5 6 V2.5 H6 M10 2.5 H13.5 V6 M13.5 10 V13.5 H10 M6 13.5 H2.5 V10", fill: "none", stroke: "currentColor", strokeLinecap: "round", strokeLinejoin: "round", strokeWidth: "1.4" }),
    grow: /* @__PURE__ */ jsxs(Fragment2, { children: [
      /* @__PURE__ */ jsx("path", { d: "M2.5 13.5 C5 13 6 10.5 7.5 8.5 S11 4 13.5 2.5", fill: "none", stroke: "currentColor", strokeLinecap: "round", strokeWidth: "1.3" }),
      /* @__PURE__ */ jsx("circle", { cx: "2.6", cy: "13.4", fill: "currentColor", r: "1.6" }),
      /* @__PURE__ */ jsx("circle", { cx: "13.4", cy: "2.6", fill: "currentColor", r: "1.6" }),
      /* @__PURE__ */ jsx("path", { d: "M6.6 9.6 c-1.6 .2 -2.4 -.6 -2.6 -2 c1.4 -.1 2.4 .5 2.6 2 Z M9.4 5.8 c.3 -1.6 1.3 -2.2 2.6 -2 c-.1 1.4 -1 2.2 -2.6 2 Z", fill: "currentColor", fillOpacity: ".8" })
    ] }),
    arrange: /* @__PURE__ */ jsxs(Fragment2, { children: [
      /* @__PURE__ */ jsx("circle", { cx: "5", cy: "5", fill: "currentColor", r: "2" }),
      /* @__PURE__ */ jsx("path", { d: "M8.5 8.5 L13.5 13.5 M13.5 9.6 V13.5 H9.6", fill: "none", stroke: "currentColor", strokeLinecap: "round", strokeLinejoin: "round", strokeWidth: "1.4" })
    ] }),
    layers: /* @__PURE__ */ jsxs(Fragment2, { children: [
      /* @__PURE__ */ jsx("path", { d: "M8 2 L14 5.2 L8 8.4 L2 5.2 Z", fill: "currentColor", fillOpacity: ".9" }),
      /* @__PURE__ */ jsx("path", { d: "M2.4 8 L8 11 L13.6 8 M2.4 10.8 L8 13.8 L13.6 10.8", fill: "none", stroke: "currentColor", strokeLinejoin: "round", strokeWidth: "1.2" })
    ] }),
    close: /* @__PURE__ */ jsx("path", { d: "M4 4 L12 12 M12 4 L4 12", fill: "none", stroke: "currentColor", strokeLinecap: "round", strokeWidth: "1.6" }),
    play: /* @__PURE__ */ jsx("path", { d: "M5 3 L13 8 L5 13 Z", fill: "currentColor", strokeLinejoin: "round" }),
    stop: /* @__PURE__ */ jsx("rect", { fill: "currentColor", height: "9", rx: "1.5", width: "9", x: "3.5", y: "3.5" }),
    plus: /* @__PURE__ */ jsx("path", { d: "M8 3 V13 M3 8 H13", fill: "none", stroke: "currentColor", strokeLinecap: "round", strokeWidth: "1.6" }),
    make: /* @__PURE__ */ jsxs(Fragment2, { children: [
      /* @__PURE__ */ jsx("circle", { cx: "6", cy: "8", fill: "currentColor", fillOpacity: ".55", r: "4" }),
      /* @__PURE__ */ jsx("circle", { cx: "10", cy: "8", fill: "currentColor", fillOpacity: ".55", r: "4" })
    ] }),
    near: /* @__PURE__ */ jsxs(Fragment2, { children: [
      /* @__PURE__ */ jsx("circle", { cx: "8", cy: "8", fill: "currentColor", r: "1.8" }),
      /* @__PURE__ */ jsx("circle", { cx: "8", cy: "8", fill: "none", r: "4.4", stroke: "currentColor", strokeDasharray: "2 1.6", strokeWidth: "1.2" }),
      /* @__PURE__ */ jsx("circle", { cx: "13.2", cy: "4", fill: "currentColor", r: "1.1" }),
      /* @__PURE__ */ jsx("circle", { cx: "3.2", cy: "12.4", fill: "currentColor", r: "1.1" })
    ] }),
    reset: /* @__PURE__ */ jsx("path", { d: "M3.5 8 a4.5 4.5 0 1 0 1.4 -3.3 M3.5 2.8 V5.6 H6.3", fill: "none", stroke: "currentColor", strokeLinecap: "round", strokeLinejoin: "round", strokeWidth: "1.4" })
  };
  function Icon({ name, size = 16 }) {
    return /* @__PURE__ */ jsx("svg", { "aria-hidden": true, height: size, style: { flex: "none", display: "block" }, viewBox: "0 0 16 16", width: size, children: PATHS[name] });
  }
  function ToolButton({ icon, label, title, pressed, onClick, children, disabled, expanded }) {
    return /* @__PURE__ */ jsxs(
      "button",
      {
        "aria-expanded": expanded,
        "aria-haspopup": expanded === void 0 ? void 0 : "menu",
        "aria-label": children ? void 0 : label,
        "aria-pressed": expanded === void 0 ? pressed : void 0,
        className: FOCUSABLE,
        disabled,
        onClick,
        title: title ?? label,
        type: "button",
        style: {
          ...S.row,
          justifyContent: "center",
          gap: 5,
          minWidth: 30,
          height: 30,
          padding: children ? "0 10px 0 8px" : 0,
          font: "inherit",
          fontSize: 12,
          fontWeight: pressed ? 600 : 500,
          borderRadius: 15,
          border: "none",
          background: pressed ? `color-mix(in srgb, ${T.accent} 22%, transparent)` : "transparent",
          color: disabled ? T.faint : pressed ? T.accent : T.secondary,
          cursor: disabled ? "default" : "pointer",
          boxShadow: pressed ? `0 0 0 1px color-mix(in srgb, ${T.accent} 45%, transparent) inset` : "none",
          transition: "background-color 140ms, color 140ms, box-shadow 160ms, transform 120ms",
          flex: "none"
        },
        children: [
          /* @__PURE__ */ jsx(Icon, { name: icon }),
          children
        ]
      }
    );
  }
  function ToolGroup({ children, label }) {
    return /* @__PURE__ */ jsx("div", { "aria-label": label, role: "toolbar", style: { ...glass, ...S.row, position: "relative", zIndex: 2, gap: 2, padding: 3, borderRadius: 18, pointerEvents: "auto" }, children });
  }
  function Rule() {
    return /* @__PURE__ */ jsx("span", { "aria-hidden": true, style: { width: 1, height: 18, margin: "0 3px", background: T.border, flex: "none" } });
  }
  function LayoutMenu({ label, choices, current, onChoose, maxHeight }) {
    const [open, setOpen] = useState(false);
    const box = useRef(null);
    const items = useRef([]);
    useEffect(() => {
      if (!open) return;
      const away = (event) => {
        if (!box.current?.contains(event.target)) setOpen(false);
      };
      window.addEventListener("pointerdown", away, true);
      return () => window.removeEventListener("pointerdown", away, true);
    }, [open]);
    const shown = choices.find((choice) => choice.id === current) ?? choices[0];
    useEffect(() => {
      if (open) items.current[Math.max(0, choices.indexOf(shown))]?.focus();
    }, [open]);
    const keys = (event) => {
      if (!open) return;
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        setOpen(false);
        return;
      }
      if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
      event.preventDefault();
      const at = items.current.findIndex((item) => item === event.target);
      items.current[(at + (event.key === "ArrowDown" ? 1 : choices.length - 1)) % choices.length]?.focus();
    };
    return /* @__PURE__ */ jsxs("div", { onKeyDown: keys, ref: box, style: { position: "relative", display: "flex" }, children: [
      /* @__PURE__ */ jsx(ToolButton, { expanded: open, icon: "layers", label, onClick: () => setOpen(!open), pressed: open || shown !== choices[0], title: `${label}: ${shown?.name ?? ""}`, children: shown !== choices[0] ? shown?.name : void 0 }),
      open && /* @__PURE__ */ jsx("div", { "aria-label": label, className: "kumi-pop", role: "menu", style: { ...glass, position: "absolute", top: 38, right: -4, width: "max-content", maxWidth: 300, maxHeight, overflowY: "auto", padding: 4, display: "flex", flexDirection: "column", gap: 1, zIndex: 3 }, children: choices.map((choice, index) => /* @__PURE__ */ jsxs(
        "button",
        {
          "aria-checked": choice.id === shown?.id,
          className: FOCUSABLE,
          onClick: () => {
            onChoose(choice.id);
            setOpen(false);
          },
          ref: (element) => {
            items.current[index] = element;
          },
          role: "menuitemradio",
          type: "button",
          style: {
            display: "flex",
            flexDirection: "column",
            alignItems: "flex-start",
            gap: 1,
            font: "inherit",
            textAlign: "left",
            padding: "6px 10px",
            borderRadius: 8,
            border: "none",
            cursor: "pointer",
            color: T.text,
            background: choice.id === shown?.id ? `color-mix(in srgb, ${T.accent} 16%, transparent)` : "transparent"
          },
          children: [
            /* @__PURE__ */ jsx("span", { style: { fontSize: 12.5, fontWeight: 600, color: choice.id === shown?.id ? T.accent : T.text }, children: choice.name }),
            /* @__PURE__ */ jsx("span", { style: { fontSize: 11, color: T.muted }, children: choice.title })
          ]
        },
        choice.id
      )) })
    ] });
  }

  // src/shared/atlas/Glyph.tsx
  function Glyph2({ glyph, color, size = 9 }) {
    const shape = GLYPH_SHAPES[glyph];
    return /* @__PURE__ */ jsx("svg", { "aria-hidden": true, height: size, style: { flex: "none" }, viewBox: "-1.5 -1.5 3 3", width: size, children: shape ? /* @__PURE__ */ jsx("polygon", { fill: color, points: shape.map(([x, y]) => `${x},${y}`).join(" ") }) : /* @__PURE__ */ jsx("circle", { cx: "0", cy: "0", fill: color, r: "1" }) });
  }

  // src/shared/atlas/Wave.tsx
  function usePlayhead(playing, duration) {
    const [startedAt, setStartedAt] = useState(null);
    const [now, setNow] = useState(0);
    useEffect(() => {
      setStartedAt(playing ? performance.now() : null);
      setNow(performance.now());
    }, [playing]);
    const progress = startedAt === null ? null : progressOf(now - startedAt, duration);
    const running = progress !== null;
    useEffect(() => {
      if (!running) return;
      const frame2 = requestAnimationFrame(setNow);
      return () => cancelAnimationFrame(frame2);
    }, [running, now]);
    return progress;
  }
  function Wave2({ peaks, colors, width, height, progress = null }) {
    const canvas = useRef(null);
    const tint = colors.join();
    useEffect(() => {
      const element = canvas.current;
      const context = element?.getContext("2d");
      if (!element || !context || width <= 0) return;
      const ratio = window.devicePixelRatio || 1;
      element.width = Math.round(width * ratio);
      element.height = Math.round(height * ratio);
      context.setTransform(ratio, 0, 0, ratio, 0, 0);
      context.clearRect(0, 0, width, height);
      const own = getComputedStyle(element).color;
      if (colors.length > 1) {
        const across = context.createLinearGradient(0, 0, width, 0);
        colors.forEach((color, index) => across.addColorStop(index / (colors.length - 1), color));
        context.fillStyle = across;
      } else context.fillStyle = colors[0] ?? own;
      const bars = Math.max(1, Math.floor(width / 3));
      for (let bar = 0; bar < bars; bar++) {
        let peak = 0.06;
        if (peaks && peaks.length > 0) {
          const [from, to] = [Math.floor(bar * peaks.length / bars), Math.max(Math.floor(bar * peaks.length / bars) + 1, Math.floor((bar + 1) * peaks.length / bars))];
          for (let at = from; at < to && at < peaks.length; at++) peak = Math.max(peak, peaks[at]);
        }
        const tall = Math.max(1, peak * (height - 2));
        context.globalAlpha = !peaks ? 0.3 : progress !== null && bar / bars <= progress ? 1 : progress !== null ? 0.5 : 0.9;
        context.fillRect(bar * 3, (height - tall) / 2, 2, tall);
      }
      context.globalAlpha = 1;
    }, [peaks, tint, width, height, progress]);
    return /* @__PURE__ */ jsxs("span", { "aria-hidden": true, style: { position: "relative", display: "block", flex: "none", width, height }, children: [
      /* @__PURE__ */ jsx("canvas", { ref: canvas, style: { display: "block", width, height, color: T.accent } }),
      progress !== null && progress > 0 && progress < 1 && /* @__PURE__ */ jsx("span", { style: { position: "absolute", top: -1, bottom: -1, left: progress * width, width: 1.5, borderRadius: 1, background: T.text } })
    ] });
  }

  // src/shared/atlas/HoverPill.tsx
  var WIDEST = 300;
  var TALLEST = 44;
  var AWAY = 24;
  function HoverPill({
    id,
    sound,
    color,
    glyph,
    playing,
    audio,
    at,
    within
  }) {
    const [peaks, setPeaks] = useState(() => peaksKept(id, WAVE_BARS));
    const fetched = useRef(audio);
    fetched.current = audio;
    useEffect(() => {
      setPeaks(peaksKept(id, WAVE_BARS));
      if (!playing || !fetched.current) return;
      let live = true;
      void peaksFor(id, WAVE_BARS, fetched.current, () => live).then((found) => live && found && setPeaks(found));
      return () => {
        live = false;
      };
    }, [id, playing]);
    const duration = typeof sound.duration === "number" ? sound.duration : 0;
    const progress = usePlayhead(playing, duration);
    const under = [seconds(sound.duration), clean(sound.folder ?? "", 40)].filter(Boolean).join(" · ");
    const side = at[0] + AWAY + WIDEST > within[0] ? { right: Math.max(0, within[0] - at[0] + AWAY) } : { left: at[0] + AWAY };
    const top = Math.min(Math.max(0, at[1] - TALLEST / 2), Math.max(0, within[1] - TALLEST));
    return /* @__PURE__ */ jsxs("div", { style: { ...glass, borderRadius: 10, position: "absolute", ...side, top, maxWidth: WIDEST, pointerEvents: "none", padding: "5px 10px", ...S.row, gap: 10 }, children: [
      /* @__PURE__ */ jsxs("div", { style: { minWidth: 0, flex: "0 1 auto" }, children: [
        /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 6, fontSize: 11.5, color: T.text }, children: [
          /* @__PURE__ */ jsx(Glyph2, { color, glyph }),
          /* @__PURE__ */ jsxs("span", { style: { ...S.ellipsis, fontWeight: 600 }, children: [
            clean(sound.name, 48),
            " ",
            /* @__PURE__ */ jsxs("span", { style: { color: T.muted, fontWeight: 400 }, children: [
              "· ",
              clean(sound.class ?? "", 16)
            ] })
          ] })
        ] }),
        under && /* @__PURE__ */ jsx("div", { style: { ...S.ellipsis, fontSize: 10.5, color: T.faint, paddingLeft: 15, fontVariantNumeric: "tabular-nums" }, children: under })
      ] }),
      peaks && /* @__PURE__ */ jsx(Wave2, { colors: [color], height: 24, peaks, progress, width: 96 })
    ] });
  }

  // src/shared/atlas/names.ts
  var NAMES_ON_SCREEN = 600;
  var NAMES_MOST = 50;
  var kept2 = /* @__PURE__ */ new WeakMap();
  function regionsOf(points) {
    const known = kept2.get(points);
    if (known) return known;
    const families = /* @__PURE__ */ new Map();
    for (const point of points) {
      const family = families.get(point.group);
      if (family) family.push(point);
      else families.set(point.group, [point]);
    }
    const regions = [];
    for (const [group, family] of families) {
      const across = family.map((point) => point.x).sort((a, b) => a - b);
      const [from, middle, to] = [across[Math.floor(across.length * 0.25)], across[Math.floor(across.length * 0.5)], across[Math.min(across.length - 1, Math.ceil(across.length * 0.75))]];
      const tops = family.filter((point) => point.x >= from && point.x <= to).map((point) => point.y);
      tops.sort((a, b) => a - b);
      regions.push({ group, x: middle, y: tops[Math.floor(tops.length * 0.015)] });
    }
    kept2.set(points, regions);
    return regions;
  }
  function regionAlpha(zoom) {
    return Math.min(1, Math.max(0, (2.3 - zoom) / 0.9));
  }
  function fitting(boxes, most, padX = 0, padY = 0, taken = []) {
    const kept3 = [...taken];
    const fits = [];
    for (let index = 0; index < boxes.length && fits.length < most; index++) {
      const [left, top, width, height] = boxes[index];
      if (kept3.some(([l, t, w, h]) => left < l + w + padX && l < left + width + padX && top < t + h + padY && t < top + height + padY)) continue;
      kept3.push(boxes[index]);
      fits.push(index);
    }
    return fits;
  }
  function nameRank(index) {
    return Math.imul(index + 1, 2654435761) >>> 0;
  }
  function clearOf(boxes, places, margin = 3) {
    const ROW = 16;
    const rows = /* @__PURE__ */ new Map();
    for (const place of places) {
      const row = Math.floor(place[1] / ROW);
      const list = rows.get(row);
      if (list) list.push(place);
      else rows.set(row, [place]);
    }
    return boxes.map(([left, top, width, height]) => {
      for (let row = Math.floor((top - margin) / ROW); row <= Math.floor((top + height + margin) / ROW); row++) {
        for (const [x, y] of rows.get(row) ?? []) {
          if (x >= left - margin && x <= left + width + margin && y >= top - margin && y <= top + height + margin) return false;
        }
      }
      return true;
    });
  }

  // src/shared/atlas/labels.ts
  function track(context, spacing) {
    if ("letterSpacing" in context) context.letterSpacing = spacing;
  }
  function drawWords(context, words, alpha, look, room) {
    if (alpha <= 0 || words.length === 0) return [];
    context.save();
    context.textAlign = "center";
    context.textBaseline = "middle";
    context.lineJoin = "round";
    context.strokeStyle = look.ground;
    const name = `700 12px ${look.family}`;
    context.font = name;
    track(context, "0.2em");
    const boxes = words.map((word) => {
      const width = context.measureText(word.text).width;
      return [word.x - width / 2, word.y - 8, width, word.count ? 30 : 16];
    });
    const written = room ? fitting(boxes, words.length, 10, 2) : boxes.map((_, index) => index);
    for (const index of written) {
      const word = words[index];
      context.font = name;
      track(context, "0.2em");
      context.lineWidth = 5;
      context.globalAlpha = 0.85 * alpha;
      context.strokeText(word.text, word.x, word.y);
      context.globalAlpha = alpha;
      context.fillStyle = word.color;
      context.fillText(word.text, word.x, word.y);
      if (!word.count) continue;
      context.font = `500 10px ${look.family}`;
      track(context, "0.06em");
      context.lineWidth = 3;
      context.globalAlpha = 0.8 * alpha;
      context.strokeText(word.count, word.x, word.y + 14);
      context.globalAlpha = 0.85 * alpha;
      context.fillStyle = look.muted;
      context.fillText(word.count, word.x, word.y + 14);
    }
    context.restore();
    return written.map((index) => boxes[index]);
  }
  var widths = /* @__PURE__ */ new Map();
  function drawNames(context, names, gap, look, taken) {
    if (names.length === 0) return;
    context.save();
    const font = `500 10.5px ${look.family}`;
    context.font = font;
    context.textBaseline = "middle";
    context.lineJoin = "round";
    context.lineWidth = 3;
    context.strokeStyle = look.ground;
    if (widths.size > 6e3) widths.clear();
    const offered = [...names].sort((a, b) => a.rank - b.rank);
    const boxes = offered.map((word) => {
      const key = `${font}|${word.text}`;
      let width = widths.get(key);
      if (width === void 0) {
        width = context.measureText(word.text).width;
        widths.set(key, width);
      }
      return [word.x + gap, word.y - 7, width, 14];
    });
    const clear = clearOf(
      boxes,
      offered.map((word) => [word.x, word.y])
    );
    const open = boxes.flatMap((_, index) => clear[index] ? [index] : []);
    for (const pick3 of fitting(open.map((index) => boxes[index]), NAMES_MOST, 16, 5, taken)) {
      const index = open[pick3];
      const word = offered[index];
      context.globalAlpha = 0.75;
      context.strokeText(word.text, boxes[index][0], word.y);
      context.globalAlpha = 0.95;
      context.fillStyle = word.color;
      context.fillText(word.text, boxes[index][0], word.y);
    }
    context.restore();
  }

  // src/shared/GrowLayer.tsx
  var percent2 = (at) => Math.round(at * 100);
  var mixed = (from, to, at) => `color-mix(in srgb, ${to} ${percent2(at)}%, ${from})`;
  function GrowLayer({
    grow,
    from,
    to,
    fromMap,
    toMap,
    fromColor,
    toColor,
    width,
    height,
    top,
    bottom,
    playing,
    onPlay,
    onStop,
    actions
  }) {
    const t = useT();
    const press = useRef(null);
    if (!from) return null;
    if (!to || !grow.to) {
      return /* @__PURE__ */ jsx("svg", { "aria-hidden": true, height, style: { position: "absolute", inset: 0, overflow: "visible", pointerEvents: "none" }, width, children: /* @__PURE__ */ jsx("circle", { className: "kumi-breathe", cx: from[0], cy: from[1], fill: "none", r: 11, stroke: fromColor, strokeWidth: 2.5 }) });
    }
    const at = (bud) => vinePoint(from, to, bud);
    const [cx, cy] = [(from[0] + to[0]) / 2 - (to[1] - from[1]) * 0.22, (from[1] + to[1]) / 2 + (to[0] - from[0]) * 0.22];
    const curve = `M ${from[0]} ${from[1]} Q ${cx} ${cy} ${to[0]} ${to[1]}`;
    const gradient = `kumi-vine-${Math.round(from[0])}-${Math.round(to[0])}`;
    const handle = at(grow.handle);
    const pointOf = (event) => {
      const box = (event.currentTarget.ownerSVGElement ?? event.currentTarget).getBoundingClientRect();
      return [event.clientX - box.left, event.clientY - box.top];
    };
    const nudge = (event) => {
      const step2 = event.key === "ArrowRight" || event.key === "ArrowUp" ? 0.05 : event.key === "ArrowLeft" || event.key === "ArrowDown" ? -0.05 : 0;
      if (!step2) return;
      event.preventDefault();
      grow.scrubTo(Math.min(1, Math.max(0, Math.round((grow.handle + step2) * 20) / 20)), true);
    };
    const picked = grow.picked === null ? null : grow.scrub?.at === grow.picked ? grow.scrub : grow.buds.find((bud) => bud.at === grow.picked) ?? null;
    const pickedAt = picked ? at(picked.at) : null;
    const failed = grow.buds.some((bud) => bud.state !== "failed") ? void 0 : grow.buds.find((bud) => bud.state === "failed");
    const another = /* @__PURE__ */ jsx(TextButton, { onClick: grow.reroll, title: t("grow.rerollTitle"), children: t("grow.reroll") });
    const xs = [from[0], to[0], cx];
    const ys = [from[1], to[1], cy];
    const anchor = pickedAt ? [pickedAt[0] - 14, pickedAt[1] - 14, pickedAt[0] + 14, pickedAt[1] + 14] : [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)];
    const card = floatBeside(anchor, 300, picked?.state === "ready" ? 132 : failed || picked?.state === "failed" ? 96 : 70, width, height, top, bottom);
    const budView = (bud, scrubbed = false) => {
      const [x, y] = at(bud.at);
      const color = mixed(fromColor, toColor, bud.at);
      const sounding = bud.state === "ready" && playing === bud.result.artifact_id;
      const isPicked = grow.picked === bud.at;
      return /* @__PURE__ */ jsxs(
        "g",
        {
          "aria-label": t("grow.bud", percent2(bud.at)),
          className: FOCUSABLE,
          onClick: () => grow.pick(bud.at),
          onKeyDown: (event) => {
            if (event.key === "Enter" || event.key === " ") {
              event.preventDefault();
              grow.pick(bud.at);
            }
          },
          role: "button",
          style: { cursor: "pointer", pointerEvents: "auto" },
          tabIndex: 0,
          children: [
            /* @__PURE__ */ jsx("circle", { cx: x, cy: y, fill: "transparent", r: 12 }),
            isPicked && /* @__PURE__ */ jsx("circle", { cx: x, cy: y, fill: "none", r: 11, stroke: T.text, strokeWidth: 1.25 }),
            sounding && /* @__PURE__ */ jsx("circle", { cx: x, cy: y, fill: "none", r: 14, stroke: color, strokeOpacity: 0.5, strokeWidth: 2 }),
            bud.state === "ready" ? /* @__PURE__ */ jsx("circle", { className: "kumi-open", cx: x, cy: y, fill: color, r: 6.5, stroke: T.raised, strokeWidth: 1.5 }) : bud.state === "failed" ? /* @__PURE__ */ jsx("path", { d: `M ${x - 4} ${y - 4} L ${x + 4} ${y + 4} M ${x + 4} ${y - 4} L ${x - 4} ${y + 4}`, stroke: T.bad, strokeLinecap: "round", strokeWidth: 2 }) : /* @__PURE__ */ jsx("circle", { className: bud.state === "growing" ? "kumi-breathe" : void 0, cx: x, cy: y, fill: T.raised, r: 5, stroke: color, strokeDasharray: bud.state === "waiting" ? "2 2" : void 0, strokeWidth: 1.75 })
          ]
        },
        `${scrubbed ? "s" : "b"}${bud.at}`
      );
    };
    return /* @__PURE__ */ jsxs(Fragment2, { children: [
      /* @__PURE__ */ jsxs("svg", { "aria-label": t("grow.title", clean(grow.from?.name ?? "", 60), clean(grow.to.name, 60)), height, role: "group", style: { position: "absolute", inset: 0, overflow: "visible", pointerEvents: "none" }, width, children: [
        /* @__PURE__ */ jsx("defs", { children: /* @__PURE__ */ jsxs("linearGradient", { gradientUnits: "userSpaceOnUse", id: gradient, x1: from[0], x2: to[0], y1: from[1], y2: to[1], children: [
          /* @__PURE__ */ jsx("stop", { offset: "0", stopColor: fromColor }),
          /* @__PURE__ */ jsx("stop", { offset: "1", stopColor: toColor })
        ] }) }),
        /* @__PURE__ */ jsx("path", { d: curve, fill: "none", opacity: 0.22, stroke: `url(#${gradient})`, strokeLinecap: "round", strokeWidth: 10 }),
        /* @__PURE__ */ jsx("path", { className: "kumi-vine", d: curve, fill: "none", pathLength: 1, stroke: `url(#${gradient})`, strokeLinecap: "round", strokeWidth: 3 }),
        /* @__PURE__ */ jsx("circle", { cx: from[0], cy: from[1], fill: "none", r: 9, stroke: fromColor, strokeWidth: 2 }),
        /* @__PURE__ */ jsx("circle", { cx: to[0], cy: to[1], fill: "none", r: 9, stroke: toColor, strokeDasharray: "3 2", strokeWidth: 2 }),
        grow.buds.map((bud) => budView(bud)),
        grow.scrub && !grow.buds.some((bud) => bud.at === grow.scrub?.at) && budView(grow.scrub, true),
        /* @__PURE__ */ jsxs(
          "g",
          {
            "aria-label": t("grow.handle"),
            "aria-valuemax": 100,
            "aria-valuemin": 0,
            "aria-valuenow": percent2(grow.handle),
            "aria-valuetext": t("grow.bud", percent2(grow.handle)),
            className: FOCUSABLE,
            onKeyDown: nudge,
            onPointerDown: (event) => {
              event.stopPropagation();
              event.currentTarget.setPointerCapture(event.pointerId);
              const [x, y] = pointOf(event);
              press.current = { x, y, moved: false };
            },
            onPointerMove: (event) => {
              const held = press.current;
              if (!held) return;
              const at2 = pointOf(event);
              if (!held.moved && Math.hypot(at2[0] - held.x, at2[1] - held.y) > 3) held.moved = true;
              if (held.moved) grow.scrubTo(nearestOnVine(from, to, at2), false);
            },
            onPointerUp: (event) => {
              const held = press.current;
              press.current = null;
              if (!held) return;
              const at2 = pointOf(event);
              if (held.moved) {
                grow.scrubTo(nearestOnVine(from, to, at2), true);
                return;
              }
              const under = [...grow.buds, ...grow.scrub ? [grow.scrub] : []].find((bud) => {
                const [x, y] = vinePoint(from, to, bud.at);
                return Math.hypot(x - at2[0], y - at2[1]) <= 12;
              });
              if (under) grow.pick(under.at);
              else grow.scrubTo(nearestOnVine(from, to, at2), true);
            },
            onPointerCancel: () => {
              press.current = null;
            },
            role: "slider",
            style: { cursor: "grab", pointerEvents: "auto" },
            tabIndex: 0,
            children: [
              /* @__PURE__ */ jsx("circle", { cx: handle[0], cy: handle[1], fill: "transparent", r: 16 }),
              /* @__PURE__ */ jsx("circle", { cx: handle[0], cy: handle[1], fill: T.raised, r: 8.5, stroke: T.accent, strokeWidth: 2.5 }),
              /* @__PURE__ */ jsx("circle", { cx: handle[0], cy: handle[1], fill: T.accent, r: 3 }),
              /* @__PURE__ */ jsx("text", { fill: T.text, fontSize: 10.5, fontWeight: 600, paintOrder: "stroke", stroke: T.raised, strokeWidth: 3, textAnchor: "middle", x: handle[0], y: handle[1] - 15, children: `${percent2(grow.handle)}%` })
            ]
          }
        )
      ] }),
      /* @__PURE__ */ jsxs(
        "div",
        {
          className: "kumi-pop",
          role: "group",
          "aria-label": t("grow.title", clean(grow.from?.name ?? "", 60), clean(grow.to.name, 60)),
          style: { ...glass, position: "absolute", zIndex: 1, left: card.left, top: card.top, width: 300, padding: "8px 10px", display: "flex", flexDirection: "column", gap: 6 },
          children: [
            /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 6 }, children: [
              /* @__PURE__ */ jsx(Icon, { name: "grow", size: 14 }),
              /* @__PURE__ */ jsx("span", { style: { ...S.ellipsis, flex: 1, fontSize: 12, fontWeight: 650 }, children: t("grow.title", clean(grow.from?.name ?? "", 40), clean(grow.to.name, 40)) }),
              /* @__PURE__ */ jsx("button", { "aria-label": t("grow.close"), className: FOCUSABLE, onClick: grow.close, title: t("grow.close"), type: "button", style: { display: "flex", padding: 3, border: "none", borderRadius: 10, background: "transparent", color: T.faint, cursor: "pointer" }, children: /* @__PURE__ */ jsx(Icon, { name: "close", size: 11 }) })
            ] }),
            !picked ? failed ? (
              // Nothing grew and nothing is on its way (a sound's file is gone, the map moved on): why, at once.
              /* @__PURE__ */ jsxs(Fragment2, { children: [
                /* @__PURE__ */ jsx("span", { style: { fontSize: 11, color: T.bad }, children: t("grow.failed", clean(failed.error, 160)) }),
                /* @__PURE__ */ jsx("div", { style: S.row, children: another })
              ] })
            ) : /* @__PURE__ */ jsx("span", { style: { ...S.faint, fontSize: 11 }, children: t("grow.hint") }) : picked.state === "ready" ? /* @__PURE__ */ jsxs(Fragment2, { children: [
              /* @__PURE__ */ jsxs("span", { style: { fontSize: 11, color: T.secondary }, title: picked.result.recipe, children: [
                /* @__PURE__ */ jsx("span", { style: { ...S.ellipsis, display: "block" }, children: clean(picked.result.recipe, 160) }),
                /* @__PURE__ */ jsx("span", { style: { color: T.faint }, children: seconds(picked.result.duration) })
              ] }),
              /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 6, flexWrap: "wrap" }, children: [
                /* @__PURE__ */ jsx(TextButton, { onClick: () => playing === picked.result.artifact_id ? onStop() : onPlay(picked.result), children: playing === picked.result.artifact_id ? t("map.stop") : t("map.play") }),
                actions?.(picked.result),
                fromMap && toMap && /* @__PURE__ */ jsx(
                  TextButton,
                  {
                    onClick: () => {
                      const place = vinePoint(fromMap, toMap, picked.at);
                      const end = { source: sourceOfResult(picked.result), id: null, place, name: t("grow.bud", percent2(picked.at)) };
                      if (grow.to) grow.between(end, grow.to);
                    },
                    title: t("grow.againTitle"),
                    children: t("grow.again")
                  }
                ),
                another
              ] })
            ] }) : picked.state === "failed" ? /* @__PURE__ */ jsxs(Fragment2, { children: [
              /* @__PURE__ */ jsx("span", { style: { fontSize: 11, color: T.bad }, children: t("grow.failed", clean(picked.error, 160)) }),
              /* @__PURE__ */ jsx("div", { style: S.row, children: another })
            ] }) : /* @__PURE__ */ jsx("span", { style: { ...S.faint, fontSize: 11 }, children: t("grow.growing", percent2(picked.at)) })
          ]
        }
      )
    ] });
  }

  // src/shared/Player.tsx
  var BARS = 160;
  function Waveform({ id, color, playing, duration, audio, onClick }) {
    const canvas = useRef(null);
    const [peaks, setPeaks] = useState(() => peaksKept(id, BARS));
    const [width, setWidth] = useState(0);
    const [startedAt, setStartedAt] = useState(null);
    const [now, setNow] = useState(0);
    useEffect(() => {
      let live = true;
      setPeaks(peaksKept(id, BARS));
      void peaksFor(id, BARS, audio, () => live).then((found) => live && setPeaks(found));
      return () => {
        live = false;
      };
    }, [id, audio]);
    useEffect(() => {
      const element = canvas.current?.parentElement;
      if (!element) return;
      const observer = new ResizeObserver((entries) => setWidth(Math.floor(entries[0]?.contentRect.width ?? 0)));
      observer.observe(element);
      return () => observer.disconnect();
    }, []);
    useEffect(() => setStartedAt(playing ? performance.now() : null), [playing, id]);
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
      const height = 34;
      element.width = width * ratio;
      element.height = height * ratio;
      context.setTransform(ratio, 0, 0, ratio, 0, 0);
      context.clearRect(0, 0, width, height);
      context.fillStyle = getComputedStyle(element).color;
      const bars = peaks ?? new Array(BARS).fill(0.06);
      const step2 = width / bars.length;
      context.globalAlpha = peaks ? 0.95 : 0.3;
      bars.forEach((peak, bar) => {
        const tall = Math.max(1, peak * (height - 2));
        context.fillRect(bar * step2, (height - tall) / 2, Math.max(1, step2 - 0.5), tall);
      });
    }, [peaks, width, color]);
    const progress = startedAt !== null && duration > 0 ? Math.min(1, (now - startedAt) / 1e3 / duration) : null;
    return /* @__PURE__ */ jsxs("div", { "aria-hidden": true, onClick, style: { position: "relative", flex: "1 1 160px", minWidth: 120, height: 34, cursor: "pointer" }, children: [
      /* @__PURE__ */ jsx("canvas", { ref: canvas, style: { width: "100%", height: 34, display: "block", color } }),
      progress !== null && progress > 0 && progress < 1 && /* @__PURE__ */ jsx("span", { style: { position: "absolute", top: -2, bottom: -2, left: `${progress * 100}%`, width: 2, borderRadius: 1, background: T.text, boxShadow: `0 0 8px ${color}` } })
    ] });
  }
  function Player({
    id,
    sound,
    color,
    playing,
    onPlay,
    onStop,
    onClose,
    actions,
    audio
  }) {
    const t = useT();
    const [shown, setShown] = useState(false);
    useEffect(() => {
      const frame2 = requestAnimationFrame(() => setShown(true));
      return () => cancelAnimationFrame(frame2);
    }, []);
    const name = clean(sound.name ?? id, 80);
    return /* @__PURE__ */ jsxs(
      "section",
      {
        "aria-label": t("map.player", name),
        style: {
          ...glass,
          position: "relative",
          display: "flex",
          flexDirection: "column",
          gap: 6,
          padding: "8px 34px 8px 8px",
          transform: shown ? "none" : "translateY(16px)",
          opacity: shown ? 1 : 0,
          transition: "transform 240ms cubic-bezier(.2,.8,.2,1), opacity 180ms",
          pointerEvents: "auto"
        },
        children: [
          /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 10, flexWrap: "wrap" }, children: [
            /* @__PURE__ */ jsx(
              "button",
              {
                "aria-label": playing ? t("map.stop") : t("map.play"),
                className: FOCUSABLE,
                onClick: playing ? onStop : onPlay,
                title: t("map.playTitle"),
                type: "button",
                style: {
                  flex: "none",
                  width: 34,
                  height: 34,
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "center",
                  padding: 0,
                  borderRadius: "50%",
                  border: `1px solid ${color}`,
                  background: playing ? `color-mix(in srgb, ${color} 34%, transparent)` : `color-mix(in srgb, ${color} 14%, transparent)`,
                  color,
                  cursor: "pointer",
                  boxShadow: playing ? `0 0 14px -2px ${color}` : "none",
                  transition: "background-color 140ms, box-shadow 200ms"
                },
                children: /* @__PURE__ */ jsx(Icon, { name: playing ? "stop" : "play", size: 13 })
              }
            ),
            /* @__PURE__ */ jsxs("div", { style: { display: "flex", flexDirection: "column", minWidth: 0, flex: "0 1 220px" }, children: [
              /* @__PURE__ */ jsx("span", { style: { ...S.text, ...S.ellipsis, fontWeight: 650 }, children: name }),
              /* @__PURE__ */ jsxs("span", { style: { ...S.row, gap: 4, marginTop: 2 }, children: [
                sound.class && /* @__PURE__ */ jsx(Chip, { children: clean(sound.class, 16) }),
                sound.kind && /* @__PURE__ */ jsx(Chip, { children: clean(sound.kind, 16) }),
                typeof sound.duration === "number" && /* @__PURE__ */ jsx(Chip, { children: seconds(sound.duration) })
              ] })
            ] }),
            /* @__PURE__ */ jsx(Waveform, { audio, color, duration: typeof sound.duration === "number" ? sound.duration : 2, id, onClick: playing ? onStop : onPlay, playing })
          ] }),
          /* @__PURE__ */ jsx(
            "button",
            {
              "aria-label": t("map.letGo"),
              className: FOCUSABLE,
              onClick: onClose,
              title: t("map.letGo"),
              type: "button",
              style: { position: "absolute", top: 6, right: 6, width: 24, height: 24, display: "flex", alignItems: "center", justifyContent: "center", padding: 0, borderRadius: 12, border: "none", background: "transparent", color: T.faint, cursor: "pointer" },
              children: /* @__PURE__ */ jsx(Icon, { name: "close", size: 12 })
            }
          ),
          /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 6, flexWrap: "wrap", marginRight: -26 }, children: [
            sound.folder && /* @__PURE__ */ jsx("span", { style: { ...S.faint, ...S.ellipsis, flex: "1 1 8rem", fontSize: 11 }, children: clean(sound.folder, 80) }),
            /* @__PURE__ */ jsx("span", { style: { ...S.row, gap: 6, flexWrap: "wrap", justifyContent: "flex-end", marginLeft: "auto" }, children: actions })
          ] })
        ]
      }
    );
  }

  // src/shared/MapCanvas.tsx
  var MATCHES_LISTED = 6;
  var FADE_MS2 = 450;
  var INTRO_MS = 900;
  var MORPH_MS = 760;
  var FLY_MS = 620;
  var FLY_STAGGER = 14;
  var FLY_MOST = 48;
  var STAGGER = 0.55;
  var ARRANGE_MS = 1e3;
  var LAYOUTS = ["character", "texture", "keys", "tempo", "families", "length", "folders"];
  var AXES = {
    character: ["left", "right", "top", "bottom"],
    texture: ["left", "right", "top", "bottom"],
    tempo: ["left", "right", "bottom"]
  };
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
      strong: read(CANVAS.strong, "#ddd"),
      ground: read(CANVAS.ground, "#151515")
    };
  }
  var ROUNDS = 8;
  var maps = 0;
  function MapCanvas({ map, found, playing, selected, onPick, onSelect, actions, empty, leaving, hiddenActions, marked, picked, onPicked, onHover, tools, pickedActions, tiles = false, onTiles, flying, wheel = "auto", audio, onStop, coverRight = 0, arrangement, onArrangement, grow, growActions, onPlayMade }) {
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
    const [handTool, setHandTool] = useState(false);
    const [grabbing, setGrabbing] = useState(false);
    const over = useRef(false);
    const [spaceHeld, setSpaceHeld] = useState(false);
    const space = useRef(false);
    useEffect(() => {
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
      const blur2 = () => hold(false);
      window.addEventListener("keydown", down, true);
      window.addEventListener("keyup", up, true);
      window.addEventListener("blur", blur2);
      return () => {
        window.removeEventListener("keydown", down, true);
        window.removeEventListener("keyup", up, true);
        window.removeEventListener("blur", blur2);
      };
    }, []);
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
      moved2.current = true;
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
    const topRef = useRef(null);
    const legendRef = useRef(null);
    const [topInset, setTopInset] = useState(0);
    const mapSize = mapSide(size.width, size.height, topInset);
    const fit = () => fitView(size.width, size.height, topInset, coverRight);
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
      if (leaving?.size) animate(FADE_MS2);
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
    useEffect(() => {
      if (performance.now() >= until.current) return;
      const frame2 = requestAnimationFrame(setClock);
      return () => cancelAnimationFrame(frame2);
    }, [clock]);
    const [ownArrangement, setOwnArrangement] = useState("");
    const arrangementId = arrangement ?? ownArrangement;
    const arrangementIndex = LAYOUTS.includes(arrangementId) ? model.arrangements.findIndex((each) => each.id === arrangementId) : -1;
    const arranged = useMemo(() => arrangedModel(model, arrangementIndex), [model, arrangementIndex]);
    const chooseArrangement = (id) => {
      if (arrangement === void 0) setOwnArrangement(id);
      onArrangement?.(id);
    };
    const layoutChoices = useMemo(() => {
      const named = LAYOUTS.filter((id) => model.arrangements.some((each) => each.id === id));
      return named.length === 0 ? [] : [{ id: "", name: t("arrange.similar.name"), title: t("arrange.similar.title") }, ...named.map((id) => ({ id, name: t(`arrange.${id}.name`), title: t(`arrange.${id}.title`) }))];
    }, [model, t]);
    const drawn = useRef(null);
    const shift = useRef(null);
    const shiftedTo = useRef(arranged);
    if (shiftedTo.current !== arranged) {
      shiftedTo.current = arranged;
      const last = drawn.current;
      shift.current = !still && last && last.model === model && last.places.length === model.points.length * 2 ? { from: last.places.slice(), at: performance.now() } : null;
    }
    useEffect(() => {
      if (shift.current) animate(ARRANGE_MS);
    }, [arranged]);
    const layout = useMemo(() => tiles || morph.from > 0 ? tileLayout(arranged) : null, [arranged, tiles, morph.from > 0]);
    const tiled = useMemo(() => layout ? tileModel(arranged, layout) : null, [arranged, layout]);
    const active = tiles && tiled ? tiled : arranged;
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
    const nearName = useMemo(() => {
      const seeds = !searched && foundIds ? (found?.near ?? []).filter((id) => model.sounds[id]) : [];
      if (seeds.length === 0) return "";
      const first = clean(model.sounds[seeds[0]].name ?? "", 60);
      return seeds.length === 1 ? t("map.near", first) : t("map.nearMany", first, seeds.length - 1);
    }, [searched, foundIds, found, model, t]);
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
      const top = topRef.current;
      if (!top) return;
      const observer = new ResizeObserver(() => {
        const legend = legendRef.current;
        setTopInset(legend ? Math.ceil(8 + legend.offsetTop + legend.offsetHeight + 8) : 0);
      });
      observer.observe(top);
      return () => observer.disconnect();
    }, [hasPoints]);
    const moved2 = useRef(false);
    useEffect(() => {
      if (size.width > 0 && size.height > 0) setView((current) => current && moved2.current ? current : fit());
    }, [size.width, size.height]);
    useEffect(() => {
      moved2.current = false;
      setView(size.width > 0 ? fit() : null);
    }, [model.revision]);
    useEffect(() => {
      if (!moved2.current && size.width > 0) setView(fit());
    }, [topInset, coverRight]);
    const framedFound = useRef(null);
    useEffect(() => {
      if (!foundIds || size.width === 0 || topInset === 0 || framedFound.current === foundIds) return;
      framedFound.current = foundIds;
      const framed = frame(active, foundIds, size.width, size.height, topInset, coverRight);
      if (!framed) return;
      moved2.current = true;
      setView(framed);
    }, [foundIds, model, size.width, size.height, topInset]);
    const wheelMode = useRef(wheel);
    wheelMode.current = wheel;
    useEffect(() => {
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
      const dots = arranged.points;
      const moving = shift.current;
      const shifting = moving && !still ? clamp01((now - moving.at) / ARRANGE_MS) : 1;
      const from = moving && shifting < 1 && moving.from.length === count * 2 ? moving.from : null;
      const places = drawn.current?.model === model && drawn.current.places.length === count * 2 ? drawn.current.places : new Float32Array(count * 2);
      drawn.current = { model, places };
      const placeOf = (index) => {
        const dot = dots[index];
        const blend = blendOf(index);
        let x = tilePoints ? dot.x + (tilePoints[index].x - dot.x) * blend : dot.x;
        let y = tilePoints ? dot.y + (tilePoints[index].y - dot.y) * blend : dot.y;
        if (from) {
          const along = easeInOut(clamp01(shifting * (1 + STAGGER) - spread[index] * STAGGER));
          x = from[2 * index] + (x - from[2 * index]) * along;
          y = from[2 * index + 1] + (y - from[2 * index + 1]) * along;
        }
        places[2 * index] = x;
        places[2 * index + 1] = y;
        return [x * unit + view.dx, y * unit + view.dy, blend];
      };
      const introFor = (index) => {
        const since = now - introAt.current - spread[index] * (INTRO_MS * 0.45);
        return since >= INTRO_MS * 0.55 ? 1 : since <= 0 ? 0 : easeOutBack(since / (INTRO_MS * 0.55));
      };
      const glyphs = new Map(model.groups.map((group) => [group.key, glyphOf(group.key)]));
      const shape = (path, x, y, blend, grow2 = 1, glyph = 0) => {
        if (blend <= 0) {
          const r = radius * grow2;
          if (r <= 0.05) return;
          traceGlyph(path, glyph, x, y, r);
          return;
        }
        const side = (2 * radius + (tileSide - 2 * radius) * blend) * grow2;
        if (side <= 0.1) return;
        const corner = side / 2 - (side / 2 - side * 0.18) * blend;
        if (typeof path.roundRect === "function") path.roundRect(x - side / 2, y - side / 2, side, side, corner);
        else path.rect(x - side / 2, y - side / 2, side, side);
      };
      const glyphAt = (index) => glyphs.get(points[index].group) ?? 0;
      const reach = (blend) => radius + (Math.max(radius, tileSide * 0.62) - radius) * blend;
      const labelled = [];
      let onScreen = 0;
      const glow = glowOf(canvas);
      const dark = glow.onDark(context, colors.ground);
      const calm = nebulaAlpha(view.scale) * (1 - morphed) * clamp01((now - introAt.current) / INTRO_MS) * (dark ? 0.85 : 0.4);
      if (calm > 0) {
        const nebula = glow.nebula(arranged, mask, (group) => groupColor2.get(group) ?? colors.faint);
        if (from && shifting < 0.3) {
          if (nebula.before) glow.draw(context, nebula.before, view, unit, calm * (1 - shifting / 0.3));
        } else if (nebula.now) glow.draw(context, nebula.now, view, unit, calm * (from ? clamp01((shifting - 0.7) / 0.3) : 1));
      }
      const halo = haloOf(radius, view.scale, glow.crowd(arranged), unit);
      for (const pass of [0, 1]) {
        const rounds = Array.from({ length: ROUNDS }, () => /* @__PURE__ */ new Map());
        const halos = pass === 1 && morphed < 1 ? glow.halos(context, halo.radius, halo.alpha, ratio) : null;
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
          const grown = introFor(index);
          halos?.at(color, x, y, grown * (1 - blend));
          shape(path, x, y, blend, grown, glyphAt(index));
          if (pass === 1 && ++onScreen <= NAMES_ON_SCREEN) labelled.push([x, y, index, color]);
        }
        halos?.done();
        context.globalAlpha = pass ? 0.92 : 0.16;
        for (const paths of rounds) {
          for (const [color, path] of paths) {
            context.fillStyle = glow.shape(color);
            context.fill(path);
          }
        }
      }
      for (const [id, at2] of leftAt.current) {
        const progress = (now - at2) / FADE_MS2;
        const index = indexOf.get(id);
        if (progress >= 1 || index === void 0) continue;
        const eased = 1 - (1 - progress) ** 2;
        const [x, y, blend] = placeOf(index);
        const path = new Path2D();
        shape(path, x, y, blend, 1 - eased + 0.02, glyphAt(index));
        context.globalAlpha = 0.92 * (1 - eased);
        context.fillStyle = groupColor2.get(points[index].group) ?? colors.faint;
        context.fill(path);
      }
      const lettering = { family: getComputedStyle(canvas).fontFamily || "sans-serif", ground: colors.ground, muted: colors.text };
      const layoutShown = arrangementIndex >= 0 ? model.arrangements[arrangementIndex] : null;
      const wordsIn = clamp01((shifting - 0.55) / 0.45);
      let words = [];
      if (layoutShown && layoutShown.labels.length > 0) {
        const families = new Map(model.groups.map((group) => [group.name, group]));
        for (const label of layoutShown.labels) {
          const [x, y] = [label.x * unit + view.dx, label.y * unit + view.dy];
          if (x < left - 80 || y < top || x > right + 80 || y > bottom) continue;
          const family = families.get(label.text);
          words.push(family ? { text: family.name.toLocaleUpperCase(), count: num(family.count, 0), color: glow.shape(family.color), x, y } : { text: label.text, color: colors.strong, x, y });
        }
      } else if (regionAlpha(view.scale) > 0) {
        const families = new Map(model.groups.map((group) => [group.key, group]));
        words = regionsOf(tilePoints && morphed > 0.5 ? tilePoints : dots).flatMap((region) => {
          const family = families.get(region.group);
          const [x, y] = [region.x * unit + view.dx, region.y * unit + view.dy - 20];
          if (!family || hidden.has(family.key) || x < left || y < top || x > right || y > bottom) return [];
          return [{ text: family.name.toLocaleUpperCase(), count: num(family.count, 0), color: glow.shape(family.color), x, y, size: family.count }];
        }).sort((a, b) => b.size - a.size);
      }
      const regional = !layoutShown || layoutShown.labels.length === 0;
      const written = drawWords(context, words, wordsIn * (regional ? regionAlpha(view.scale) * Math.abs(2 * morphed - 1) : 0.9), lettering, regional);
      if (onScreen <= NAMES_ON_SCREEN) {
        const names = labelled.flatMap(([x, y, index, color]) => {
          const text = mapLabel(model.sounds[points[index].id]?.name ?? "");
          return text ? [{ text, color: glow.shape(color), x, y, rank: nameRank(index) }] : [];
        });
        drawNames(context, names, reach(morphed) + 4, lettering, written);
      }
      if (layoutShown && wordsIn > 0) {
        context.textBaseline = "middle";
        context.lineJoin = "round";
        context.strokeStyle = colors.ground;
        context.lineWidth = 3;
        const family = lettering.family;
        context.font = `11px ${family}`;
        context.fillStyle = colors.text;
        context.globalAlpha = 0.85 * wordsIn;
        const [x0, y0, x1, y1] = [view.dx, view.dy, view.dx + unit, view.dy + unit];
        const [minX, maxX, minY, maxY] = [8, size.width - coverRight - 8, topInset + 10, size.height - 12];
        const keepY = (y) => Math.min(Math.max(y, minY), maxY);
        const middleX = Math.min(Math.max((x0 + x1) / 2, minX + 60), maxX - 60);
        for (const side of AXES[layoutShown.id] ?? []) {
          const word = t(`arrange.${layoutShown.id}.${side}`);
          const [text, x, y, align] = side === "left" ? [`← ${word}`, Math.max(x0 + 6, minX), keepY((y0 + y1) / 2), "left"] : side === "right" ? [`${word} →`, Math.min(x1 - 6, maxX), keepY((y0 + y1) / 2), "right"] : side === "top" ? [`↑ ${word}`, middleX, keepY(y0 + 10), "center"] : [`↓ ${word}`, middleX, keepY(y1 - 10), "center"];
          context.textAlign = align;
          context.strokeText(text, x, y);
          context.fillText(text, x, y);
        }
        context.textAlign = "start";
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
      const ring2 = (id, color, extra, width) => {
        const index = indexOf.get(id);
        if (index === void 0) return;
        const [x, y, blend] = placeOf(index);
        context.strokeStyle = color;
        context.lineWidth = width;
        context.beginPath();
        context.arc(x, y, reach(blend) + extra, 0, Math.PI * 2);
        context.stroke();
      };
      foundIds?.forEach((id) => ring2(id, colors.accent, 3, 1.5));
      const hovered2 = hoverAt;
      if (hovered2 !== null && hovered2 < count && mask[hovered2] === 1) {
        const [x, y, blend] = placeOf(hovered2);
        const color = groupColor2.get(points[hovered2].group) ?? colors.faint;
        const path = new Path2D();
        shape(path, x, y, blend, 1.45, glyphAt(hovered2));
        context.save();
        context.shadowColor = color;
        context.shadowBlur = 14;
        context.fillStyle = glow.shape(color);
        context.fill(path);
        context.restore();
        const ring3 = new Path2D();
        shape(ring3, x, y, blend, 2.3, glyphAt(hovered2));
        context.strokeStyle = colors.strong;
        context.lineWidth = 1.25;
        context.stroke(ring3);
      }
      if (selected) ring2(selected, colors.strong, 4, 2);
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
    }, [model, arranged, arrangementIndex, indexOf, view, size, mask, foundIds, selected, playing, mapSize, painted, clock, marked, picked, tiled, layout, morph, spread, flying, still, hoverAt, t, topInset, coverRight]);
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
      const boxing = event.button === 0 && Boolean(onPicked) && !space.current && !handTool && !grow?.choosing;
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
      const index = pick2(active, view, mapSize, x, y, reachPx(10), visible);
      const adding = event.shiftKey || event.metaKey || event.ctrlKey;
      if (index < 0) {
        if (adding) return;
        if (pickedIds.length > 0) onPicked?.([], "replace");
        if (selected !== null) onSelect(null);
        return;
      }
      const id = model.points[index].id;
      if (grow?.choosing) {
        const end = endOf(id);
        if (end) grow.name(end);
        onPick(id);
      } else if (onPicked && (event.metaKey || event.ctrlKey)) onPicked([id], "toggle");
      else if (onPicked && event.shiftKey) onPicked([id], "add");
      else onPick(id);
    };
    const endOf = (id) => soundEnd(id, model.sounds[id]?.name, model.revision);
    const placeOfEnd = (end) => {
      if (!end) return null;
      if (end.id === null) return end.place;
      const index = indexOf.get(end.id);
      return index === void 0 ? null : [active.points[index].x, active.points[index].y];
    };
    const colorOfEnd = (end) => {
      const index = end?.id ? indexOf.get(end.id) : void 0;
      return index === void 0 ? T.accent : model.groups.find((group) => group.key === model.points[index].group)?.color ?? T.accent;
    };
    const screenOf = (place) => place && view ? toScreen(view, mapSize, place[0], place[1]) : null;
    const onKey = (event) => {
      if (event.key === "Escape" && grow?.choosing && !event.shiftKey && !event.altKey && !event.ctrlKey && !event.metaKey) {
        event.preventDefault();
        grow.choose(false);
        return;
      }
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
        moved2.current = false;
        setView(fit());
      } else if ("zoom" in key) {
        const [x, y] = picked2 === void 0 ? middle : toScreen(view, mapSize, active.points[picked2].x, active.points[picked2].y);
        moved2.current = true;
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
    const picksBox = useMemo(() => {
      if (!view || pickedIds.length === 0) return null;
      let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
      for (const id of pickedIds) {
        const index = indexOf.get(id);
        if (index === void 0 || mask[index] === 2) continue;
        const [x, y] = toScreen(view, mapSize, active.points[index].x, active.points[index].y);
        [x0, y0, x1, y1] = [Math.min(x0, x), Math.min(y0, y), Math.max(x1, x), Math.max(y1, y)];
      }
      const inView = Number.isFinite(x0) && x1 >= 0 && y1 >= 0 && x0 <= size.width && y0 <= size.height;
      return inView ? [x0, y0, x1, y1] : null;
    }, [view, pickedIds, indexOf, mask, active, mapSize, size.width, size.height]);
    const cluster = useRef(null);
    const [clusterSize, setClusterSize] = useState([320, 40]);
    const hasPicks = pickedIds.length > 0;
    useEffect(() => {
      const element = cluster.current;
      if (!element) return;
      const observer = new ResizeObserver((entries) => {
        const rect = entries[0]?.contentRect;
        if (rect) setClusterSize([Math.ceil(rect.width) + 12, Math.ceil(rect.height) + 12]);
      });
      observer.observe(element);
      return () => observer.disconnect();
    }, [hasPicks]);
    if (!model.points.length) {
      return /* @__PURE__ */ jsx("div", { style: { ...S.muted, fontSize: 13, flex: 1, height: "100%", minHeight: 240, display: "flex", alignItems: "center", justifyContent: "center", padding: 24, textAlign: "center" }, children: /* @__PURE__ */ jsxs("div", { style: { maxWidth: 380 }, children: [
        empty,
        (map?.hidden ?? 0) > 0 && /* @__PURE__ */ jsxs("div", { role: "status", style: { ...S.row, flexWrap: "wrap", justifyContent: "center", gap: 6, marginTop: 8 }, children: [
          /* @__PURE__ */ jsx("span", { style: { ...S.faint, flex: "1 1 14rem" }, children: t("map.hidden", map?.hidden ?? 0) }),
          hiddenActions
        ] })
      ] }) });
    }
    const chosen = selected ? model.sounds[selected] : void 0;
    const chosenIndex = selected ? indexOf.get(selected) : void 0;
    const chosenColor = chosenIndex === void 0 ? T.accent : model.groups.find((group) => group.key === model.points[chosenIndex].group)?.color ?? T.accent;
    const hovered = hover ? model.points[hover.index] : void 0;
    const hoveredSound = hovered ? model.sounds[hovered.id] : void 0;
    const playingIndex = playing === null ? void 0 : indexOf.get(playing);
    const playingGroup = playingIndex === void 0 ? void 0 : model.points[playingIndex].group;
    const playingSound = playing === null || playingGroup === void 0 ? null : { id: playing, color: model.groups.find((group) => group.key === playingGroup)?.color ?? T.accent, glyph: glyphOf(playingGroup), duration: model.sounds[playing]?.duration ?? 0 };
    const playingPlace = () => {
      const [last, at2] = [drawn.current, viewRef.current];
      if (playingIndex === void 0 || !last || !at2 || last.places.length < 2 * playingIndex + 2) return null;
      const unit = mapSize * at2.scale;
      return [last.places[2 * playingIndex] * unit + at2.dx, last.places[2 * playingIndex + 1] * unit + at2.dy];
    };
    const countWords = shown === model.points.length - leavingCount ? t("map.count", num(shown, 0)) : t("map.countOf", num(shown, 0), num(model.points.length - leavingCount, 0));
    const hiddenCount = (map?.hidden ?? 0) + leavingCount;
    const clusterAt = floatBeside(picksBox, clusterSize[0], clusterSize[1], size.width - coverRight, size.height, topInset, chosen ? 120 : 10);
    return /* @__PURE__ */ jsxs("div", { style: { position: "relative", height: "100%", minHeight: 240, overflow: "hidden", color: T.text }, children: [
      /* @__PURE__ */ jsxs("div", { ref: topRef, style: { position: "absolute", top: 8, left: 8, right: 8, zIndex: 1, display: "flex", flexDirection: "column", alignItems: "flex-start", gap: 6, pointerEvents: "none" }, children: [
        /* @__PURE__ */ jsxs("div", { style: { ...S.row, alignSelf: "stretch", gap: 8, flexWrap: "wrap", alignItems: "flex-start" }, children: [
          /* @__PURE__ */ jsxs("div", { style: { ...glass, borderRadius: 18, position: "relative", display: "flex", alignItems: "center", flex: "1 1 180px", maxWidth: 420, minWidth: 150, height: 36, pointerEvents: "auto" }, children: [
            /* @__PURE__ */ jsx(
              "input",
              {
                "aria-label": t("map.search"),
                className: FOCUSABLE,
                onChange: (event) => setQuery(event.target.value),
                placeholder: t("map.searchHint"),
                style: { flex: 1, minWidth: 0, height: "100%", font: "inherit", fontSize: 12.5, padding: "0 12px", paddingRight: `${countWords.length * 0.55 + 1.6}em`, borderRadius: 18, border: "none", background: "transparent", color: T.text, outlineOffset: -2 },
                value: query
              }
            ),
            /* @__PURE__ */ jsx(
              "span",
              {
                style: { ...S.faint, fontSize: 10.5, lineHeight: "18px", padding: "0 7px", borderRadius: 9, background: T.chip, position: "absolute", right: 8, top: "50%", transform: "translateY(-50%)", whiteSpace: "nowrap", cursor: "default" },
                title: model.revision ? t("map.revision", model.revision) : void 0,
                children: countWords
              }
            )
          ] }),
          /* @__PURE__ */ jsx("span", { style: { flex: 1 } }),
          /* @__PURE__ */ jsxs(ToolGroup, { label: t("map.tools"), children: [
            onPicked && /* @__PURE__ */ jsx(ToolButton, { icon: "pick", label: t("map.pickTool"), onClick: () => setHandTool(false), pressed: !handTool, title: t("map.pickToolTitle") }),
            onPicked && /* @__PURE__ */ jsx(ToolButton, { icon: "hand", label: t("map.hand"), onClick: () => setHandTool(true), pressed: handTool, title: t("map.handTitle") }),
            grow && /* @__PURE__ */ jsx(ToolButton, { icon: "grow", label: t("grow.tool"), onClick: () => grow.choose(!grow.choosing), pressed: grow.choosing !== null, title: t("grow.toolTitle") }),
            onPicked && /* @__PURE__ */ jsx(Rule, {}),
            layoutChoices.length > 0 && /* @__PURE__ */ jsx(LayoutMenu, { choices: layoutChoices, current: arrangementIndex >= 0 ? arrangementId : "", label: t("arrange.label"), maxHeight: Math.max(120, size.height - 60), onChoose: chooseArrangement }),
            onTiles && /* @__PURE__ */ jsx(ToolButton, { icon: "tiles", label: t("map.tiles"), onClick: () => onTiles(!tiles), pressed: tiles, title: t("map.tilesTitle") }),
            tools,
            /* @__PURE__ */ jsx(Rule, {}),
            /* @__PURE__ */ jsx(
              ToolButton,
              {
                icon: "fit",
                label: t("map.fit"),
                onClick: () => {
                  moved2.current = false;
                  setView(fit());
                },
                title: t("map.fitTitle")
              }
            )
          ] })
        ] }),
        /* @__PURE__ */ jsx("div", { "aria-label": t("map.legend"), ref: legendRef, role: "group", style: { ...glass, borderRadius: 14, ...S.row, flexWrap: "wrap", gap: 4, padding: 4, pointerEvents: "auto", maxWidth: "100%" }, children: model.groups.filter((group) => group.count > 0).map((group) => {
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
              style: { ...S.row, gap: 5, font: "inherit", fontSize: 11, padding: "2px 8px 2px 6px", borderRadius: 10, border: "none", background: off ? "transparent" : `color-mix(in srgb, ${group.color} 12%, transparent)`, color: off ? T.faint : T.secondary, cursor: "pointer", opacity: off ? 0.55 : 1, transition: "background-color 140ms, opacity 140ms" },
              children: [
                /* @__PURE__ */ jsx("svg", { "aria-hidden": true, height: "9", style: { flex: "none", opacity: off ? 0.35 : 1 }, viewBox: "-1.5 -1.5 3 3", width: "9", children: /* @__PURE__ */ jsx(GlyphShape, { color: group.color, glyph: glyphOf(group.key) }) }),
                group.name,
                " ",
                /* @__PURE__ */ jsx("span", { style: { color: T.faint }, children: group.count })
              ]
            },
            group.key
          );
        }) }),
        grow?.choosing && // Grow names its sounds with clicks on the map: which one this click names, and a way out.
        /* @__PURE__ */ jsxs("div", { className: "kumi-pop", role: "status", style: { ...glass, borderRadius: 16, ...S.row, gap: 8, padding: "4px 6px 4px 12px", pointerEvents: "auto", maxWidth: "100%" }, children: [
          /* @__PURE__ */ jsx(Icon, { name: "grow", size: 13 }),
          /* @__PURE__ */ jsx("span", { style: { fontSize: 12, color: T.text }, children: grow.choosing === "from" ? t("grow.from") : t("grow.to", clean(grow.from?.name ?? "", 60)) }),
          /* @__PURE__ */ jsx(TextButton, { onClick: () => grow.choose(false), children: t("grow.cancel") })
        ] }),
        matches.length > 0 && /* @__PURE__ */ jsxs("div", { "aria-label": searched ? t("map.matches") : nearName || t("map.found"), role: "group", style: { ...glass, display: "flex", flexDirection: "column", gap: 1, padding: "6px 6px 6px", width: "min(340px, 100%)", pointerEvents: "auto" }, children: [
          /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 6, padding: "0 4px 4px" }, children: [
            /* @__PURE__ */ jsx("span", { style: { ...S.muted, ...S.ellipsis, flex: 1, fontSize: 11.5 }, children: searched ? t("map.matches") : nearName || t("map.found") }),
            onPicked && // Native's `a`: every sound the search keeps, or every sound Kumi found (a "More like these" among them).
            /* @__PURE__ */ jsx(TextButton, { onClick: () => onPicked(matches, "replace"), title: searched !== null ? t("dock.pickAllTitle") : t("dock.pickFoundTitle"), children: t("dock.pickAll", matches.length) })
          ] }),
          matches.slice(0, MATCHES_LISTED).map((id) => {
            const sound = model.sounds[id];
            const indexed = indexedValues(sound, t);
            const reasons = searched ? "" : foundLine(found?.matches?.[id], nearName !== "", t);
            return /* @__PURE__ */ jsxs(
              "button",
              {
                "aria-pressed": selected === id,
                className: FOCUSABLE,
                onClick: () => onPick(id),
                type: "button",
                style: { display: "flex", flexDirection: "column", alignItems: "stretch", gap: 0, font: "inherit", fontSize: 11.5, padding: "2px 6px", borderRadius: 6, border: "none", textAlign: "left", cursor: "pointer", color: T.text, background: selected === id ? T.card : "transparent" },
                children: [
                  /* @__PURE__ */ jsxs("span", { style: { ...S.row, gap: 6 }, children: [
                    /* @__PURE__ */ jsx("span", { style: { ...S.ellipsis, flex: 1 }, children: clean(sound?.name ?? id, 80) }),
                    indexed && /* @__PURE__ */ jsx("span", { style: { color: T.faint, whiteSpace: "nowrap" }, title: t("map.indexedTitle"), children: t("map.indexed", indexed) }),
                    sound?.class && /* @__PURE__ */ jsx("span", { style: { color: T.faint }, children: clean(sound.class, 16) })
                  ] }),
                  reasons && /* @__PURE__ */ jsx("span", { style: { ...S.ellipsis, color: T.faint, fontSize: 10.5 }, title: nearName ? t("map.closenessTitle") : void 0, children: reasons })
                ]
              },
              id
            );
          }),
          matches.length > MATCHES_LISTED && /* @__PURE__ */ jsx("span", { style: { ...S.faint, padding: "0 6px" }, children: t("map.more", matches.length - MATCHES_LISTED) })
        ] }),
        hiddenCount > 0 && // One quiet line for every sound that couldn't play here, never a notice per sound.
        /* @__PURE__ */ jsxs("div", { role: "status", style: { ...glass, borderRadius: 14, ...S.row, flexWrap: "wrap", gap: 6, padding: "3px 6px 3px 10px", pointerEvents: "auto", maxWidth: "100%" }, children: [
          /* @__PURE__ */ jsx("span", { style: { ...S.faint, fontSize: 11 }, children: t("map.hidden", hiddenCount) }),
          hiddenActions
        ] })
      ] }),
      /* @__PURE__ */ jsxs("div", { ref: boxRef, style: { position: "absolute", inset: 0 }, children: [
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
              const index = pick2(active, view, mapSize, x, y, reachPx(8), visible);
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
            onDoubleClick: () => {
              moved2.current = false;
              setView(fit());
            },
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
        /* @__PURE__ */ jsx(AtlasOverlay, { audio, height: size.height, moved: `${view?.scale},${view?.dx},${view?.dy},${clock}`, place: playingPlace, playing: playingSound, still, width: size.width }),
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
        hovered && hoveredSound && hover && /* @__PURE__ */ jsx(
          HoverPill,
          {
            at: [hover.x, hover.y],
            audio,
            color: model.groups.find((group) => group.key === hovered.group)?.color ?? T.accent,
            glyph: glyphOf(hovered.group),
            id: hovered.id,
            playing: playing === hovered.id,
            sound: hoveredSound,
            within: [size.width - coverRight, size.height]
          },
          hovered.id
        ),
        grow && (grow.from || grow.to) && /* @__PURE__ */ jsx(
          GrowLayer,
          {
            actions: growActions,
            bottom: chosen ? 120 : 10,
            from: screenOf(placeOfEnd(grow.from)),
            fromColor: colorOfEnd(grow.from),
            fromMap: placeOfEnd(grow.from),
            grow,
            height: size.height,
            onPlay: (result) => onPlayMade?.(result),
            onStop: () => onStop?.(),
            playing,
            to: screenOf(placeOfEnd(grow.to)),
            toColor: colorOfEnd(grow.to),
            toMap: placeOfEnd(grow.to),
            top: topInset,
            width: size.width - coverRight
          }
        )
      ] }),
      hasPicks && // The picks' actions, beside the picks (`kumi-pop` as they appear): how many, then what they can do.
      /* @__PURE__ */ jsxs(
        "div",
        {
          "aria-label": t("dock.picked", pickedIds.length),
          className: "kumi-pop",
          ref: cluster,
          role: "group",
          style: {
            ...glass,
            position: "absolute",
            zIndex: 1,
            left: clusterAt.left,
            top: clusterAt.top,
            maxWidth: `calc(100% - 16px)`,
            ...S.row,
            flexWrap: "wrap",
            gap: 6,
            padding: "5px 6px 5px 10px",
            transition: still ? "none" : "left 220ms cubic-bezier(.2,.8,.2,1), top 220ms cubic-bezier(.2,.8,.2,1)"
          },
          children: [
            /* @__PURE__ */ jsxs("span", { style: { ...S.row, gap: 5, fontSize: 12, fontWeight: 650, color: T.text }, children: [
              /* @__PURE__ */ jsx(Icon, { name: "pick", size: 12 }),
              t("dock.picked", pickedIds.length)
            ] }),
            pickedActions?.(pickedIds)
          ]
        }
      ),
      selected && chosen && // The sound picked last, in the player at the map's foot (left of a side sheet, when one is open).
      /* @__PURE__ */ jsx("div", { style: { position: "absolute", left: 10, right: 10 + coverRight, bottom: 10, zIndex: 1, display: "flex", justifyContent: "center", pointerEvents: "none", transition: still ? "none" : "right 260ms cubic-bezier(.2,.8,.2,1)" }, children: /* @__PURE__ */ jsx("div", { style: { width: "min(640px, 100%)" }, children: /* @__PURE__ */ jsx(
        Player,
        {
          actions: actions?.(selected, chosen),
          audio: audio ?? (() => Promise.reject(new Error("no audio"))),
          color: chosenColor,
          id: selected,
          onClose: () => {
            if (playing === selected) onStop?.();
            onSelect(null);
          },
          onPlay: () => onPick(selected),
          onStop: () => onStop?.(),
          playing: playing === selected,
          sound: chosen
        },
        selected
      ) }) })
    ] });
  }
  function GlyphShape({ glyph, color }) {
    const shape = GLYPH_SHAPES[glyph];
    return shape ? /* @__PURE__ */ jsx("polygon", { fill: color, points: shape.map(([x, y]) => `${x},${y}`).join(" ") }) : /* @__PURE__ */ jsx("circle", { cx: "0", cy: "0", fill: color, r: "1" });
  }

  // src/shared/PaletteDock.tsx
  var CHIPS_SHOWN = 240;
  var OPEN_HEIGHT = 260;
  var LOWEST = 150;
  var HIGHEST = 620;
  function Waveform2({ id, color, audio, width, height, bars }) {
    const canvas = useRef(null);
    const [peaks, setPeaks] = useState(() => id ? peaksKept(id, bars) : null);
    const [drawnWidth, setDrawnWidth] = useState(typeof width === "number" ? width : 0);
    useEffect(() => {
      const element = canvas.current;
      if (!id || !element || peaks) return;
      let live = true;
      const observer = new IntersectionObserver((entries) => {
        if (!entries.some((entry) => entry.isIntersecting)) return;
        observer.disconnect();
        void peaksFor(id, bars, audio).then((found) => live && setPeaks(found));
      });
      observer.observe(element);
      return () => {
        live = false;
        observer.disconnect();
      };
    }, [id, audio, peaks, bars]);
    useEffect(() => {
      if (typeof width === "number") return;
      const element = canvas.current;
      if (!element) return;
      const observer = new ResizeObserver((entries) => setDrawnWidth(Math.floor(entries[0]?.contentRect.width ?? 0)));
      observer.observe(element);
      return () => observer.disconnect();
    }, [width]);
    useEffect(() => {
      const element = canvas.current;
      const context = element?.getContext("2d");
      if (!element || !context || drawnWidth === 0) return;
      const ratio = window.devicePixelRatio || 1;
      element.width = drawnWidth * ratio;
      element.height = height * ratio;
      context.setTransform(ratio, 0, 0, ratio, 0, 0);
      context.clearRect(0, 0, drawnWidth, height);
      context.fillStyle = getComputedStyle(element).color;
      const shown = peaks ?? new Array(bars).fill(0.08);
      const step2 = drawnWidth / shown.length;
      context.globalAlpha = peaks ? 0.95 : 0.35;
      shown.forEach((peak, bar) => {
        const tall = Math.max(1, peak * (height - 2));
        context.fillRect(bar * step2, (height - tall) / 2, Math.max(1, step2 - 0.6), tall);
      });
    }, [peaks, color, drawnWidth, height, bars]);
    return /* @__PURE__ */ jsx("canvas", { "aria-hidden": true, ref: canvas, style: { width, height, flex: "none", display: "block", color } });
  }
  function useItemKeys(index, onMove, onRemove) {
    return (event) => {
      if (event.altKey && (event.key === "ArrowLeft" || event.key === "ArrowRight")) {
        event.preventDefault();
        onMove(index + (event.key === "ArrowLeft" ? -1 : 1));
      } else if (event.key === "Delete") {
        event.preventDefault();
        onRemove();
      }
    };
  }
  function useDrop({ item, dragging, onDrag, onDropHere }, name) {
    const [over, setOver] = useState(false);
    return {
      over,
      handlers: {
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
        }
      }
    };
  }
  function Chip2(props) {
    const { item, index, color, playing, audio, onPlay, onStop, onRemove, onMove, dragging, arriving } = props;
    const t = useT();
    const id = playableId(item);
    const name = clean(item.name, 60) || item.key;
    const { over, handlers } = useDrop(props, name);
    const keys = useItemKeys(index, onMove, onRemove);
    return /* @__PURE__ */ jsxs(
      "div",
      {
        ...handlers,
        className: arriving === null ? void 0 : "kumi-pop",
        role: "listitem",
        style: {
          display: "flex",
          alignItems: "center",
          gap: 2,
          flex: "none",
          maxWidth: 220,
          padding: "2px 2px 2px 4px",
          borderRadius: 7,
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
                /* @__PURE__ */ jsx(Waveform2, { audio, bars: 28, color, height: 16, id, width: 44 }),
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
  function Card(props) {
    const { item, index, color, playing, audio, onPlay, onStop, onRemove, onMove, dragging, arriving, use } = props;
    const t = useT();
    const id = playableId(item);
    const name = clean(item.name, 80) || item.key;
    const { over, handlers } = useDrop(props, name);
    const keys = useItemKeys(index, onMove, onRemove);
    return /* @__PURE__ */ jsxs(
      "div",
      {
        ...handlers,
        className: arriving === null ? void 0 : "kumi-pop",
        role: "listitem",
        style: {
          display: "flex",
          flexDirection: "column",
          gap: 4,
          minWidth: 0,
          padding: 7,
          borderRadius: 10,
          border: `1px solid ${playing ? color : over ? T.accent : T.border}`,
          background: playing ? `color-mix(in srgb, ${color} 12%, transparent)` : `color-mix(in srgb, ${T.card} 55%, transparent)`,
          boxShadow: playing ? `0 0 0 1px ${color}, 0 6px 18px -8px ${color}` : "none",
          opacity: dragging === item.key ? 0.4 : id ? 1 : 0.55,
          transition: "border-color 120ms, box-shadow 180ms, background-color 160ms, opacity 120ms, transform 160ms",
          cursor: "grab",
          animationDelay: arriving === null ? void 0 : `${380 + Math.min(arriving, 24) * 22}ms`
        },
        children: [
          /* @__PURE__ */ jsx(
            "button",
            {
              "aria-label": playing ? t("dock.stop", name) : t("dock.play", name),
              "aria-pressed": playing,
              className: FOCUSABLE,
              disabled: !id,
              onClick: () => playing ? onStop() : onPlay(),
              onKeyDown: keys,
              title: id ? t("dock.reorderHint") : t("dock.offMap"),
              type: "button",
              style: { display: "block", width: "100%", padding: 0, border: "none", background: "transparent", cursor: id ? "pointer" : "default" },
              children: /* @__PURE__ */ jsx(Waveform2, { audio, bars: 64, color, height: 30, id, width: "100%" })
            }
          ),
          /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 5 }, children: [
            /* @__PURE__ */ jsx("span", { "aria-hidden": true, style: { width: 7, height: 7, borderRadius: "50%", background: color, flex: "none" } }),
            /* @__PURE__ */ jsx("span", { style: { ...S.ellipsis, flex: 1, fontSize: 11.5, color: T.text }, title: name, children: name }),
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
          ] }),
          /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 5, minHeight: 18 }, children: [
            /* @__PURE__ */ jsx("span", { style: { ...S.faint, ...S.ellipsis, flex: 1, fontSize: 10.5 }, children: item.source === "artifact" ? t("dock.made") : clean(item.class ?? "", 16) }),
            use
          ] })
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
  function PaletteDock({ palette, groups, playing, onPlay, onStop, onDo, said, audio, kit, use, make, landing, open, onOpen }) {
    const t = useT();
    const current = palette?.current ?? null;
    const [editing, setEditing] = useState(null);
    const [forgetting, setForgetting] = useState(false);
    const [dragging, setDragging] = useState(null);
    const [height, setHeight] = useState(OPEN_HEIGHT);
    const [order, setOrder] = useState(null);
    useEffect(() => setOrder(null), [palette]);
    const known = useRef(null);
    const arrived = useMemo(() => {
      const keys = new Set((current?.items ?? []).map((item) => item.key));
      const before = known.current;
      const fresh2 = before && before.id === current?.id ? (current?.items ?? []).filter((item) => !before.keys.has(item.key)).map((item) => item.key) : [];
      known.current = { id: current?.id ?? null, keys };
      return new Map(fresh2.map((key, order2) => [key, order2]));
    }, [current]);
    const strip = useRef(null);
    useEffect(() => {
      const element = strip.current;
      if (!element || arrived.size === 0) return;
      const smooth = window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth";
      element.scrollTo(open ? { top: element.scrollHeight, behavior: smooth } : { left: element.scrollWidth, behavior: smooth });
    }, [arrived, open]);
    const [now, setNow] = useState(() => Date.now());
    const shownFor = said?.failed ? 8e3 : 4e3;
    useEffect(() => {
      if (!said) return;
      setNow(Date.now());
      const timer = setTimeout(() => setNow(Date.now()), Math.max(0, said.at + shownFor - Date.now()));
      return () => clearTimeout(timer);
    }, [said, shownFor]);
    const saying = said && now - said.at < shownFor ? said : null;
    const grip = useRef(null);
    const gripDown = (event) => {
      if (!open || event.button !== 0) return;
      event.currentTarget.setPointerCapture?.(event.pointerId);
      grip.current = { y: event.clientY, height, moved: false };
    };
    const gripMove = (event) => {
      const held = grip.current;
      if (!held) return;
      const delta = held.y - event.clientY;
      if (Math.abs(delta) > 3) held.moved = true;
      if (held.moved) setHeight(Math.min(HIGHEST, Math.max(LOWEST, held.height + delta)));
    };
    const run = (action) => onDo(action).then((done) => {
      if (!done) setOrder(null);
    });
    const items = order ?? current?.items ?? [];
    const colorOf2 = (item) => item.source === "artifact" ? T.accent : item.group === null ? T.faint : groupColor(groups?.[String(item.group)]?.color, item.group);
    const move = (key, to) => {
      if (to < 0 || to >= items.length) return;
      setOrder(moved(items, key, to));
      void run({ action: "move", key, to });
    };
    const itemProps = (item, index) => ({
      audio,
      color: colorOf2(item),
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
      playing: playing !== null && playing === playableId(item),
      use: use?.(item)
    });
    const header = /* @__PURE__ */ jsxs("div", { style: { ...S.row, flexWrap: "wrap", gap: 6 }, children: [
      /* @__PURE__ */ jsx("span", { style: { ...S.heading, fontSize: 11, color: T.secondary }, children: t("dock.title") }),
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
            style: { font: "inherit", fontSize: 12, fontWeight: 600, maxWidth: "14rem", padding: "0 4px", borderRadius: 4, border: `1px solid ${T.border}`, background: "transparent", color: T.text },
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
      current && current.excluded > 0 && /* @__PURE__ */ jsx("span", { style: S.faint, children: t("dock.excluded", current.excluded) }),
      /* @__PURE__ */ jsx("span", { style: { flex: 1 } }),
      current && current.items.length > 1 && make && /* @__PURE__ */ jsx(TextButton, { onClick: () => make(current), title: t("make.openTitle"), children: t("make.open") }),
      current && current.items.length > 0 && kit?.(current),
      current && !forgetting && /* @__PURE__ */ jsx(TextButton, { onClick: () => setForgetting(true), title: t("dock.forgetTitle", clean(current.name, 40)), children: t("dock.forget") }),
      current && items.length > 0 && /* @__PURE__ */ jsxs(
        "button",
        {
          "aria-expanded": open,
          className: FOCUSABLE,
          onClick: () => onOpen(!open),
          title: open ? t("dock.collapse") : t("dock.expand"),
          type: "button",
          style: {
            ...S.row,
            gap: 4,
            font: "inherit",
            fontSize: 11.5,
            fontWeight: 600,
            padding: "2px 10px",
            borderRadius: 12,
            border: `1px solid ${T.accent}`,
            background: `color-mix(in srgb, ${T.accent} ${open ? 10 : 18}%, transparent)`,
            color: T.accent,
            cursor: "pointer",
            flex: "none"
          },
          children: [
            open ? t("dock.fold") : t("dock.showAll", items.length),
            /* @__PURE__ */ jsx("span", { "aria-hidden": true, style: { display: "inline-block", transform: open ? "rotate(180deg)" : "none", transition: "transform 200ms" }, children: "▴" })
          ]
        }
      )
    ] });
    return /* @__PURE__ */ jsxs(
      "section",
      {
        "aria-label": t("dock.title"),
        ref: landing,
        style: {
          position: "relative",
          display: "flex",
          flexDirection: "column",
          gap: 6,
          padding: "0 10px 8px",
          borderTop: `1px solid ${T.border}`,
          background: `color-mix(in srgb, ${T.card} 40%, transparent)`,
          color: T.text,
          flex: "none",
          height: open && items.length > 0 ? height : void 0,
          transition: grip.current ? "none" : "height 260ms cubic-bezier(.2,.8,.2,1)"
        },
        children: [
          /* @__PURE__ */ jsx(
            "button",
            {
              "aria-expanded": open,
              "aria-label": open ? t("dock.collapse") : t("dock.expand"),
              className: FOCUSABLE,
              onClick: () => {
                if (grip.current?.moved) return;
                onOpen(!open);
              },
              onPointerCancel: () => grip.current = null,
              onPointerDown: gripDown,
              onPointerMove: gripMove,
              onPointerUp: () => setTimeout(() => grip.current = null, 0),
              title: open ? t("dock.gripTitle") : t("dock.expand"),
              type: "button",
              style: { alignSelf: "stretch", height: 12, padding: 0, border: "none", background: "transparent", cursor: open ? "ns-resize" : "pointer", display: "flex", alignItems: "center", justifyContent: "center", flex: "none" },
              children: /* @__PURE__ */ jsx("span", { "aria-hidden": true, style: { width: 36, height: 4, borderRadius: 2, background: T.border, transition: "background-color 140ms" } })
            }
          ),
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
          current && items.length > 0 && open && /* @__PURE__ */ jsxs("div", { "aria-label": t("dock.sounds"), ref: strip, role: "list", style: { flex: 1, minHeight: 0, overflowY: "auto", display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(150px, 1fr))", gap: 8, alignContent: "start", padding: "2px 1px" }, children: [
            items.slice(0, CHIPS_SHOWN).map((item, index) => /* @__PURE__ */ jsx(Card, { ...itemProps(item, index) }, item.key)),
            items.length > CHIPS_SHOWN && /* @__PURE__ */ jsx("span", { style: { ...S.faint, alignSelf: "center" }, children: t("dock.more", items.length - CHIPS_SHOWN) })
          ] }),
          current && items.length > 0 && !open && /* @__PURE__ */ jsxs("div", { "aria-label": t("dock.sounds"), ref: strip, role: "list", style: { display: "flex", flexWrap: "nowrap", gap: 5, overflowX: "auto", paddingBottom: 2, scrollbarWidth: "thin" }, children: [
            items.slice(0, CHIPS_SHOWN).map((item, index) => /* @__PURE__ */ jsx(Chip2, { ...itemProps(item, index) }, item.key)),
            items.length > CHIPS_SHOWN && /* @__PURE__ */ jsx("span", { style: { ...S.faint, alignSelf: "center", flex: "none" }, children: t("dock.more", items.length - CHIPS_SHOWN) })
          ] }),
          (!current || items.length === 0) && /* @__PURE__ */ jsxs("span", { style: { ...S.row, gap: 6, ...S.faint }, children: [
            /* @__PURE__ */ jsx(Icon, { name: "plus", size: 12 }),
            t("dock.empty")
          ] }),
          /* @__PURE__ */ jsx("span", { "aria-live": "polite", style: saying ? { ...S.faint, color: saying.failed ? T.warn : T.muted } : S.hidden, children: saying ? saying.failed ? t("dock.failed", saying.text) : saying.text : "" })
        ]
      }
    );
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
    const kept3 = useMemo(() => new Set((state.palette?.current?.items ?? []).flatMap((item) => item.artifact_id ? [item.artifact_id] : [])), [state.palette]);
    const groupOf = useMemo(() => new Map((state.map?.points ?? []).map(([id, , , group]) => [id, group])), [state.map]);
    useEffect(() => setMaking((current) => current?.some((source) => source.sound_id && source.map_revision !== revision) ? null : current), [revision]);
    const open = (sources) => {
      setMaking(sources);
      void store.readDesignsOnce();
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
      kept: kept3,
      makeFromPicks: (ids) => open(sourcesOf(ids.map((id) => ({ sound_id: id, name: state.map?.sounds[id]?.name ?? null })), revision)),
      makeFromPalette: (view) => open(sourcesOf(view.items, state.palette?.map_revision ?? null)),
      closeMake: () => setMaking(null),
      makeFrom: open,
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

  // src/shared/steps.ts
  var NEEDS_SETUP = { needs_runtime: true, outdated_runtime: true, damaged_runtime: true, needs_daw_component: true };
  var SETUP_DAWS = ["ableton", "reaper", "renoise"];
  var IN_DAW = { choose_control_surface: true, restart_daw: true, enable_tool: true };
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

  // src/dashboard/index.tsx
  var BASE = "/api/plugins/kumi";
  var POLL_MS = 5e3;
  var HOVER_PLAY = "kumi.mapHover";
  var TILES = "kumi.mapTiles";
  var PALETTE_OPEN = "kumi.paletteOpen";
  var LAYOUT = "kumi.mapLayout";
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
    const [paletteOpen, setPaletteOpen] = useState(() => globalThis.localStorage?.getItem(PALETTE_OPEN) !== "off");
    const [layout, setLayout] = useState(() => globalThis.localStorage?.getItem(LAYOUT) ?? "");
    const palette = usePaletteMap(store, state, audition, t, { hover, left: (id) => setSelected((current) => current === id ? null : current) });
    const grow = useGrow((request) => store.design(request), (result) => void audition.play(result.artifact_id).catch(() => {
    }), state.map?.revision ?? null);
    const dock = useRef(null);
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
    const setupMessage = setupWords(state);
    const steps = nextSteps(state, t);
    return /* @__PURE__ */ jsxs("div", { style: { display: "flex", flexDirection: "column", gap: 12, color: T.text, minHeight: "calc(100vh - 160px)" }, children: [
      /* @__PURE__ */ jsxs("div", { style: { ...S.row, gap: 10, padding: "4px 2px" }, children: [
        /* @__PURE__ */ jsx(Dot, { color: tone, size: 9 }),
        /* @__PURE__ */ jsx("span", { style: { fontSize: 15, fontWeight: 600 }, children: words.text }),
        state.status?.runtime?.version && /* @__PURE__ */ jsxs("span", { style: S.faint, children: [
          "Kumi ",
          state.status.runtime.version
        ] }),
        setupMessage && /* @__PURE__ */ jsx("span", { style: S.muted, children: setupMessage }),
        steps.map((step2) => /* @__PURE__ */ jsx(CopyButton, { label: step2.label, text: step2.request }, step2.request))
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
        /* @__PURE__ */ jsxs("div", { style: { border: `1px solid ${T.border}`, borderRadius: 6, background: T.card, overflow: "hidden", minHeight: 780, display: "flex", flexDirection: "column" }, children: [
          /* @__PURE__ */ jsx(
            MapCanvas,
            {
              audio: (id) => audition.url(id),
              onStop: () => audition.stop(),
              actions: (id, sound) => {
                const request = soundRequest(id, sound.name, state.map?.revision);
                return /* @__PURE__ */ jsxs(Fragment2, { children: [
                  /* @__PURE__ */ jsx(TextButton, { disabled: palette.marked.has(id), onClick: () => void palette.gather([id]), title: t("dock.addTitle"), children: palette.marked.has(id) ? t("dock.inPalette") : t("dock.add") }),
                  request && /* @__PURE__ */ jsx(CopyButton, { label: t("dashboard.copyReference"), text: request })
                ] });
              },
              empty: state.map === null ? t("map.reading") : ["needs_runtime", "outdated_runtime", "damaged_runtime"].includes(state.setup?.state ?? "") ? (
                // No runtime yet: setup comes first (the dashboard's header says how).
                t("map.setupFirst")
              ) : /* @__PURE__ */ jsxs(Fragment2, { children: [
                t("dashboard.mapEmpty"),
                /* @__PURE__ */ jsx("div", { style: { marginTop: 10 }, children: /* @__PURE__ */ jsx(CopyButton, { label: t("map.scanCopy"), text: SCAN_REQUEST }) })
              ] }),
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
              grow,
              onPlayMade: (result) => void audition.play(result.artifact_id).catch(() => {
              }),
              growActions: (result) => {
                const request = useRequest(result);
                const kept3 = palette.kept.has(result.artifact_id);
                return /* @__PURE__ */ jsxs(Fragment2, { children: [
                  /* @__PURE__ */ jsx(TextButton, { disabled: kept3, onClick: () => void palette.keep(result), title: t("make.keepTitle"), children: kept3 ? t("make.kept") : t("make.keep") }),
                  request && /* @__PURE__ */ jsx(CopyButton, { label: t("make.copyUse"), text: request }),
                  /* @__PURE__ */ jsx(TextButton, { onClick: () => palette.makeFrom([sourceOfResult(result), ...grow.to ? [grow.to.source] : []]), title: t("make.openTitle"), children: t("make.open") })
                ] });
              },
              onSelect: setSelected,
              picked: palette.picked,
              pickedActions: (ids) => /* @__PURE__ */ jsxs(Fragment2, { children: [
                /* @__PURE__ */ jsx(TextButton, { onClick: () => void palette.gather(ids), strong: true, title: t("dock.addTitle"), children: t("dock.addPicked", ids.length) }),
                ids.length > 1 && /* @__PURE__ */ jsx(TextButton, { onClick: () => palette.makeFromPicks(ids), title: t("make.openTitle"), children: t("make.open") }),
                ids.length === 2 && (() => {
                  const [from, to] = ids.map((id) => soundEnd(id, state.map?.sounds[id]?.name, state.map?.revision ?? null));
                  return from && to ? (
                    // The vine takes over from the picks: their actions would float over its card.
                    /* @__PURE__ */ jsx(
                      TextButton,
                      {
                        onClick: () => {
                          grow.between(from, to);
                          palette.clearPicks();
                        },
                        title: t("grow.betweenTitle"),
                        children: t("grow.between")
                      }
                    )
                  ) : null;
                })(),
                /* @__PURE__ */ jsx(TextButton, { onClick: palette.clearPicks, title: t("dock.clearPicksTitle"), children: t("dock.clearPicks", ids.length) })
              ] }),
              playing,
              selected,
              flying: palette.flying,
              onTiles: (on) => {
                globalThis.localStorage?.setItem(TILES, on ? "on" : "off");
                setTiles(on);
              },
              tiles,
              arrangement: layout,
              onArrangement: (id) => {
                globalThis.localStorage?.setItem(LAYOUT, id);
                setLayout(id);
              },
              tools: /* @__PURE__ */ jsx(
                ToolButton,
                {
                  icon: "hover",
                  label: t("dock.hover"),
                  onClick: () => {
                    globalThis.localStorage?.setItem(HOVER_PLAY, hover ? "off" : "on");
                    setHover(!hover);
                  },
                  pressed: hover,
                  title: t("dock.hoverTitle")
                }
              )
            }
          ),
          (state.map?.points?.length ?? 0) > 0 && /* @__PURE__ */ jsx(
            PaletteDock,
            {
              audio: (id) => audition.url(id),
              groups: state.map?.groups,
              kit: (current) => /* @__PURE__ */ jsx(CopyButton, { label: t("dock.kitCopy"), text: kitRequest(current.name) }),
              landing: dock,
              make: palette.makeFromPalette,
              onDo: (action) => store.paletteDo(action),
              onOpen: (open) => {
                globalThis.localStorage?.setItem(PALETTE_OPEN, open ? "on" : "off");
                setPaletteOpen(open);
              },
              onPlay: (id) => palette.play(id),
              onStop: () => audition.stop(),
              open: paletteOpen,
              palette: state.palette,
              playing,
              said: state.paletteSaid,
              use: (item) => {
                const request = itemReference(item, state.palette?.map_revision ?? null);
                return request ? /* @__PURE__ */ jsx(CopyButton, { label: t("dashboard.copyReference"), text: request }) : null;
              }
            }
          )
        ] }),
        palette.making && /* @__PURE__ */ jsx("div", { style: { gridColumn: 2, minWidth: 0 }, children: /* @__PURE__ */ jsx(
          MakePanel,
          {
            colorOf: palette.colorOf,
            dock,
            placement: "inline",
            kept: palette.kept,
            onClose: palette.closeMake,
            onKeep: palette.keep,
            onMake: (request) => store.design(request),
            onSources: palette.makeFrom,
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
