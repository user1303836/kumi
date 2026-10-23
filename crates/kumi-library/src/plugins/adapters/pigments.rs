use std::sync::LazyLock;

use crate::plugins::adapter::{pattern, PluginAdapter, PluginKind, PluginParameterHint, PluginRecipe, PluginSection};

// Names: low confidence. Arturia spells names out with their module first ("Engine 1 …", "Filter 1 …"), in a form not
// confirmed here, so the patterns need only the module and the control, in that order.
pub static PIGMENTS: LazyLock<PluginAdapter> = LazyLock::new(|| {
    PluginAdapter {
    id: "pigments", name: "Pigments", vendor: "Arturia", kind: PluginKind::Instrument,
    matcher: pattern(r"(?i)\bPigments\b"),
    overview: "Arturia's polysynth. Two main engines, each Virtual Analog, Wavetable, Sample (with granular) or Harmonic (newer versions add types), plus a utility engine (noise and samples). The engines feed two filters (in series or parallel), then the amp and three FX buses (A, B and an aux send), up to three effects each. Modulation: envelopes (the first drives the amp), LFOs, function generators, random sources, combinators, macros and a sequencer/arpeggiator; drag a source onto a knob. Hundreds of parameters, so Live turns only the configured ones.",
    sections: vec![
        PluginSection { name: "Engines", about: "Engines 1 and 2. What a control does depends on the engine type, picked in the window.",
            parameters: vec![
                PluginParameterHint { role: "engine level", names: pattern(r"(?i)Eng(ine)? ?[1-3]\b.*(Vol(ume)?|Level)"), about: "The engine's output." },
                PluginParameterHint { role: "engine pitch", names: pattern(r"(?i)Eng(ine)? ?[12]\b.*\b(Coarse|Fine|Tune|Pitch|Oct(ave)?)\b"), about: "Semitones and cents." },
                PluginParameterHint { role: "wavetable position", names: pattern(r"(?i)Eng(ine)? ?[12]\b.*(Position|\bPos\b|Morph)"), about: "Wavetable engine: where in the table, the main timbre move. Sample engine: where in the sample." },
                PluginParameterHint { role: "unison", names: pattern(r"(?i)Eng(ine)? ?[12]\b.*(Unison|Detune|Spread)"), about: "Unison voices, detune and spread." },
                PluginParameterHint { role: "filter send", names: pattern(r"(?i)Eng(ine)? ?[12]\b.*(Filter|Filt) ?(Mix|Balance|Route|Routing|Send)"), about: "How much goes to Filter 1 against Filter 2." },
            ] },
        PluginSection { name: "Filters and amp", about: "Two filters; the type (multimode, SEM, ladders, comb, formant, phaser, surgeon…) is picked in the window.",
            parameters: vec![
                PluginParameterHint { role: "cutoff", names: pattern(r"(?i)F(ilter|ilt|lt)? ?[12]\b.*(Cutoff|Freq)"), about: "Hz." },
                PluginParameterHint { role: "resonance", names: pattern(r"(?i)F(ilter|ilt|lt)? ?[12]\b.*Res(o(nance)?)?\b"), about: "0–100%." },
                PluginParameterHint { role: "filter drive and level", names: pattern(r"(?i)F(ilter|ilt|lt)? ?[12]\b.*(Drive|Gain|Mix|Volume)"), about: "Drive into it, its level." },
                PluginParameterHint { role: "routing", names: pattern(r"(?i)Filter ?Routing|Filter ?(Series|Parallel)|F1 ?F2"), about: "Series, parallel, or a blend between." },
                PluginParameterHint { role: "amp envelope", names: pattern(r"(?i)(Env(elope)? ?1|VCA|Amp ?Env).*(Attack|Decay|Sustain|Release)"), about: "The amp: Attack, Decay, Release in ms or s; Sustain 0–100%." },
            ] },
        PluginSection { name: "Modulation and FX", about: "Sources move only what they're routed to.",
            parameters: vec![
                PluginParameterHint { role: "mod envelopes", names: pattern(r"(?i)Env(elope)? ?[23]\b.*(Attack|Decay|Sustain|Release)"), about: "Same stages, for filter, pitch or anything." },
                PluginParameterHint { role: "lfo", names: pattern(r"(?i)LFO ?[1-3]\b.*(Rate|Freq|Sync)"), about: "Hz free, or a note value synced." },
                PluginParameterHint { role: "macro", names: pattern(r"(?i)^(Macro ?[1-4]|M[1-4])\b"), about: "The four macros: whatever the preset routed them to." },
                PluginParameterHint { role: "fx mix", names: pattern(r"(?i)\b(FX|Bus|Aux)\b.*(Dry ?/? ?Wet|Mix|Send|Return|Level|Volume)"), about: "Each effect's dry/wet, the aux send, the buses' levels." },
                PluginParameterHint { role: "master", names: pattern(r"(?i)^(Master|Main|Output) ?(Vol(ume)?|Level|Gain)?$"), about: "Output level." },
            ] },
    ],
    recipes: vec![
        PluginRecipe { name: "Wavetable growl bass", how: "Engine 1 Wavetable on a vocal or growl table, LFO 1 synced 1/8 on Position (40–60%). Engine 2 a sine an octave down for weight. Filter 1 a formant or comb type, LFO 2 on its cutoff. Mono, glide 40 ms. Bus A: Distortion, then a compressor." },
        PluginRecipe { name: "Supersaw", how: "Engine 1 Virtual Analog saw, unison 7, detune a third of the way up, spread full. Filter 1 low-pass at 6–8 kHz. Amp Attack 5 ms, Release 400 ms. Chorus 25%, Reverb 20%." },
        PluginRecipe { name: "Pluck", how: "Filter 1 low-pass at 300 Hz, Resonance 20%; envelope 2 to its cutoff, large amount: Attack 0, Decay 250 ms, Sustain 0. Amp Decay 400 ms, Sustain 0, Release 300 ms. Delay 1/8 dotted, 20%." },
        PluginRecipe { name: "Granular texture", how: "Engine 1 Sample in granular mode on a long texture: grains 100–200 ms, high density, position moved by a slow LFO (0.1 Hz), some random spray. Amp Attack 1 s, Release 3 s. Reverb 40%." },
        PluginRecipe { name: "Evolving pad", how: "A function generator over 2–4 bars, looping, on Wavetable Position and Filter 1 cutoff; engines 1 and 2 a few cents apart. Amp Attack 0.8 s, Release 2.5 s. Chorus 30%, Reverb 35%." },
    ],
    beyond: "Not parameters: each engine's type and its wavetable or sample (the engine's browser), filter types, modulation routing (drag a source onto a knob, or the modulation view), LFO and function shapes, sequencer and arpeggiator patterns, which effects sit on each bus and their order. Do these in Pigments' window (Kumi can open it) or ask the producer. A wavetable Kumi makes is imported from the Wavetable engine's browser; presets are saved from Pigments' own browser.",
    folders: None,
    wavetable: None,
}
});
