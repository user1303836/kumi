use std::sync::LazyLock;

use crate::plugins::adapter::{
    pattern, OsPaths, PluginAdapter, PluginFolders, PluginKind, PluginParameterHint, PluginRecipe, PluginSection, PluginWavetable,
    WavetableFormat,
};

// Names: Vital is open source and gives hosts each parameter's display name ("Filter 1 Cutoff", "Envelope 1 Attack",
// "LFO 1 Frequency", "Modulation 1 Amount"); oscillators read "Oscillator 1 …" or "Osc 1 …", and the patterns take both.
// Folders: Vital's defaults (Music/Vital on a Mac, Documents/Vital on Windows); its settings can move them.
pub static VITAL: LazyLock<PluginAdapter> = LazyLock::new(|| {
    PluginAdapter {
    id: "vital", name: "Vital", vendor: "Vital Audio", kind: PluginKind::Instrument,
    matcher: pattern(r"(?i)^Vital(ium)?\b"),
    overview: "Matt Tytel's free wavetable synth. Three wavetable oscillators and a sampler, each sent to Filter 1, Filter 2, both, the effects, or straight out; each oscillator has a spectral morph and a distortion (warp) stage. Then the effects (Chorus, Compressor, Delay, Distortion, EQ, Filter, Flanger, Phaser, Reverb), in the order set in the Effects tab. Six envelopes (Env 1 is the amp), eight LFOs, four random sources and four macros modulate anything by drag and drop. Hundreds of parameters, so Live turns only the configured ones.",
    sections: vec![
        PluginSection { name: "Oscillators and sampler", about: "Osc 1–3 have the same controls. The table, the spectral morph mode and the distortion mode are picked in the window.",
            parameters: vec![
                PluginParameterHint { role: "level and pan", names: pattern(r"(?i)^Osc(illator)? ?[1-3] (Level|Pan)$"), about: "Output level and pan." },
                PluginParameterHint { role: "pitch", names: pattern(r"(?i)^Osc(illator)? ?[1-3] (Transpose|Tune)$"), about: "Transpose in semitones, Tune in cents." },
                PluginParameterHint { role: "wave frame", names: pattern(r"(?i)^Osc(illator)? ?[1-3] (Wave ?)?Frame$"), about: "Position in the table, 0–256: the main timbre move." },
                PluginParameterHint { role: "unison", names: pattern(r"(?i)^Osc(illator)? ?[1-3] (Unison (Voices|Detune)|Detune (Power|Range)|Stereo Spread|Stack Style)$"), about: "Voices 1–16 and their detune; Stereo Spread widens them; Stack Style stacks octaves or chords instead." },
                PluginParameterHint { role: "spectral morph", names: pattern(r"(?i)^Osc(illator)? ?[1-3] (Spectral|Frequency) Morph( Amount| Spread)?$"), about: "Amount of the morph (Vocode, Formant Scale, Harmonic Stretch, Smear, Low and High Pass, Phase Disperse, Shepard Tone, Skew…)." },
                PluginParameterHint { role: "distortion", names: pattern(r"(?i)^Osc(illator)? ?[1-3] Distortion (Amount|Spread|Phase)$"), about: "Amount of the oscillator's warp (Sync, Formant, Quantize, Bend, Squeeze, Pulse Width, FM or RM from another oscillator or the sampler)." },
                PluginParameterHint { role: "phase", names: pattern(r"(?i)^Osc(illator)? ?[1-3] (Phase|Phase Randomi[sz]ation|Random Phase)$"), about: "Start phase and its randomness: less random gives tighter, repeatable attacks." },
                PluginParameterHint { role: "sampler", names: pattern(r"(?i)^Sampler? (Level|Pan|Transpose|Tune)$"), about: "The sampler's level, pan and pitch (noise and one-shots go here)." },
            ] },
        PluginSection { name: "Filters", about: "Filter 1, Filter 2 and the effects' filter. Model (Analog, Dirty, Ladder, Digital, Diode, Formant, Comb, Phaser) and style are picked in the window.",
            parameters: vec![
                PluginParameterHint { role: "cutoff", names: pattern(r"(?i)^(FX )?Filter ?([12]|FX)? Cutoff$"), about: "Cutoff frequency." },
                PluginParameterHint { role: "resonance", names: pattern(r"(?i)^(FX )?Filter ?([12]|FX)? Resonance$"), about: "0–100%." },
                PluginParameterHint { role: "drive", names: pattern(r"(?i)^(FX )?Filter ?([12]|FX)? Drive$"), about: "Drive into the filter, dB." },
                PluginParameterHint { role: "blend", names: pattern(r"(?i)^(FX )?Filter ?([12]|FX)? (Pass )?Blend$"), about: "Morphs the response, low-pass through band-pass to high-pass, on most models." },
                PluginParameterHint { role: "mix and keytrack", names: pattern(r"(?i)^(FX )?Filter ?([12]|FX)? (Mix|Key ?Track)$"), about: "Mix: dry against filtered. Key Track: how far cutoff follows the note." },
                PluginParameterHint { role: "formant", names: pattern(r"(?i)^(FX )?Filter ?([12]|FX)? Formant "), about: "The Formant model's vowel (X, Y), transpose and resonance." },
            ] },
        PluginSection { name: "Modulation", about: "A source moves only what it's connected to. Each connection's amount is a parameter; what it links isn't.",
            parameters: vec![
                PluginParameterHint { role: "amp envelope", names: pattern(r"(?i)^Env(elope)? ?1 (Delay|Attack|Hold|Decay|Sustain|Release)$"), about: "Env 1, the amp: times in seconds, Sustain 0–100%." },
                PluginParameterHint { role: "mod envelopes", names: pattern(r"(?i)^Env(elope)? ?[2-6] (Delay|Attack|Hold|Decay|Sustain|Release)$"), about: "Env 2–6, same stages." },
                PluginParameterHint { role: "envelope curves", names: pattern(r"(?i)^Env(elope)? ?[1-6] (Attack|Decay|Release) Power$"), about: "Each stage's curve: snappier or softer." },
                PluginParameterHint { role: "lfo", names: pattern(r"(?i)^LFO ?[1-8] (Frequency|Tempo|Sync( Type)?|Phase|Fade( In)?( Time)?|Delay( Time)?|Smooth( Time)?)$"), about: "Frequency in Hz when free, Tempo when synced (the sync mode is a parameter too); Fade and Delay ease it in." },
                PluginParameterHint { role: "macro", names: pattern(r"(?i)^Macro( Control)? ?[1-4]$"), about: "0–100%: whatever the preset connected." },
                PluginParameterHint { role: "connection amount", names: pattern(r"(?i)^Modulation ?[0-9]+ (Amount|Power|Bipolar|Stereo|Bypass)$"), about: "Connection N: amount (-100% to 100%), curve, bipolar, stereo, bypass. Its source and destination show in the window's matrix." },
            ] },
        PluginSection { name: "Effects and global", about: "An effect works only while it's on.",
            parameters: vec![
                PluginParameterHint { role: "effect on", names: pattern(r"(?i)^(Chorus|Compressor|Delay|Distortion|EQ|Filter FX|FX Filter|Flanger|Phaser|Reverb) (Switch|On|Enabled?)$"), about: "Turns that effect on or off." },
                PluginParameterHint { role: "effect mix", names: pattern(r"(?i)^(Chorus|Compressor|Delay|Distortion|Flanger|Phaser|Reverb) (Mix|Dry ?Wet)$"), about: "Dry/wet, 0–100%." },
                PluginParameterHint { role: "distortion", names: pattern(r"(?i)^Distortion (Drive|Type)$"), about: "Drive (dB) and type: Soft Clip, Hard Clip, Linear Fold, Sine Fold, Bit Crush, Down Sample." },
                PluginParameterHint { role: "compressor", names: pattern(r"(?i)^Compressor (Attack|Release|(Low|Mid|Band|High) )"), about: "Three-band upward and downward compression, OTT-style: per-band gains, thresholds, ratios; attack, release." },
                PluginParameterHint { role: "delay", names: pattern(r"(?i)^Delay (Feedback|Frequency|Tempo|Sync|Style|Filter)"), about: "Feedback %, time (Hz, or Tempo when synced), Style (mono, stereo, ping-pong), a filter on the repeats." },
                PluginParameterHint { role: "reverb", names: pattern(r"(?i)^Reverb (Decay|Size|Delay|Pre|Low|High|Chorus)"), about: "Decay time, size, pre-delay, the tail's filtering and chorus." },
                PluginParameterHint { role: "volume and voices", names: pattern(r"(?i)^(Volume|Polyphony|Portamento (Time|Slope)|Legato)$"), about: "Output volume (dB); Polyphony 1–32 (1 is mono); glide time and curve; Legato glides only overlapping notes." },
            ] },
    ],
    recipes: vec![
        PluginRecipe { name: "Reese bass", how: "Osc 1 and 2 on the default saw, Osc 2 Tune +10 to +15 cents (or one oscillator, Unison Voices 2–3, low detune). Both to Filter 1: Analog low-pass, 24 dB style, cutoff 300–800 Hz, Drive 6–10 dB, LFO 1 at 1–2 bars moving cutoff a little. Polyphony 1, Portamento 50 ms. Distortion soft clip a few dB; Chorus 20%." },
        PluginRecipe { name: "Supersaw", how: "Osc 1 saw, Unison Voices 8–12, detune a third of the way up, Stereo Spread full; Osc 2 the same, Transpose +12, quieter. Env 1 Attack 10 ms, Release 0.4 s. Chorus 30%, Reverb 20%." },
        PluginRecipe { name: "Pluck", how: "Osc 1 saw or square. Filter 1 low-pass near 200 Hz, Resonance 20%. Env 2 to Filter 1 cutoff, large amount: Attack 0, Decay 0.2–0.4 s, Sustain 0. Env 1 Decay 0.4 s, Sustain 0, Release 0.2 s. Delay ping-pong 1/8 dotted, 20%." },
        PluginRecipe { name: "Neuro growl", how: "Osc 1 on a vocal or formant table, Wave Frame on LFO 1 (synced 1/8, a shaped curve). Osc 1 distortion FM from Osc 2 (a sine an octave down) at 30–60%. Filter 1 Formant model, X on LFO 2. Effects: Distortion (sine fold), Compressor at 100%, EQ low cut at 120 Hz; the sub on its own oscillator or track." },
        PluginRecipe { name: "Spectral pad", how: "Osc 1 and 2 on soft tables, Spectral Morph (Smear or Harmonic Stretch) on a slow LFO, Unison 6, Stereo Spread full. Env 1 Attack 0.6–1 s, Release 2–3 s. Low-pass near 3 kHz. Chorus 40%, Reverb 30% with a 4–6 s decay." },
        PluginRecipe { name: "Wobble", how: "LFO 1 Tempo 1/4–1/8 (or a triplet) on Filter 1 cutoff over most of its range, a little on Wave Frame. Analog or Dirty low-pass, Drive 10 dB, Resonance 25%. Polyphony 1, Legato on. Distortion a few dB." },
    ],
    beyond: "Not host parameters: which source moves what (drag a source's handle onto a knob in Vital's window; its amount then shows as a Modulation N parameter), wavetables (the pencil above an oscillator opens the editor; a WAV dropped on it loads, Serum-style tables included), LFO shapes (drawn in each LFO), the effects' order (Effects tab). Presets are JSON (.vital): author, comments, preset_style, macro1–macro4 (the macros' names), synth_version, and settings, which holds every parameter by its internal id (osc_1_level, osc_1_wave_frame, filter_1_cutoff, env_1_attack…) in Vital's own units, not 0–1 (cutoff is a note number: 60 is about 262 Hz). settings.modulations lists the connections as {source, destination} (lfo_1 → filter_1_cutoff), each amount in modulation_N_amount; settings.wavetables, lfos and sample hold the rest. For a routing or a table no parameter reaches, Kumi can edit a saved .vital (the producer saves the sound first) and write it under a new name into the user Presets folder, for the producer to load from Vital's browser.",
    folders: Some(PluginFolders {
        presets: Some(OsPaths { mac: Some("~/Music/Vital/User/Presets"), windows: Some("~/Documents/Vital/User/Presets") }),
        wavetables: Some(OsPaths { mac: Some("~/Music/Vital/User/Wavetables"), windows: Some("~/Documents/Vital/User/Wavetables") }),
    }),
    wavetable: Some(PluginWavetable { frame: 2048, max_frames: 256, format: WavetableFormat::Clm }),
}
});
