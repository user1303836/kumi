use std::sync::LazyLock;

use crate::plugins::adapter::{pattern, PluginAdapter, PluginKind, PluginParameterHint, PluginRecipe, PluginSection};

// Names: Ozone starts each parameter with its module (and the module's instance), in a form not confirmed here; the
// patterns need only the module's name and the control's, in that order.
pub static OZONE12: LazyLock<PluginAdapter> = LazyLock::new(|| {
    PluginAdapter {
    id: "ozone12", name: "Ozone 12", vendor: "iZotope", kind: PluginKind::Effect,
    matcher: pattern(r"(?i)\bOzone ?12\b"),
    overview: "iZotope's mastering suite: one plug-in holding a chain of modules, processed in order. The core: Equalizer, Dynamic EQ, Dynamics (a multiband compressor), Imager, Exciter, and the Maximizer last. Also Low End Focus, Clarity, Stabilizer, Master Rebalance, Match EQ, the Vintage modules, and in 12 Stem EQ, Bass Control and Unlimiter (several need Advanced). Parameter names start with their module; a module's parameters do nothing while it isn't in the chain. Advanced also installs each module as its own plug-in (Ozone 12 Maximizer and so on), with the same controls. Hundreds of parameters, so Live turns only the configured ones.",
    sections: vec![
        PluginSection { name: "Maximizer", about: "The final limiter: loudness and the peak ceiling.",
            parameters: vec![
                PluginParameterHint { role: "threshold", names: pattern(r"(?i)Maximi[sz]er.*Threshold"), about: "dB. Lower pushes harder into the limiter: louder, more limited (output is made up)." },
                PluginParameterHint { role: "ceiling", names: pattern(r"(?i)Maximi[sz]er.*(Ceiling|Margin|Output Level)"), about: "The highest peak out, dB: -1.0 for streaming." },
                PluginParameterHint { role: "character", names: pattern(r"(?i)Maximi[sz]er.*Character"), about: "0–10: low is fast and aggressive, high slower and smoother." },
                PluginParameterHint { role: "mode", names: pattern(r"(?i)Maximi[sz]er.*(Mode|IRC|Style)"), about: "The IRC algorithm: later ones stay cleaner when pushed; Low Latency for live use." },
                PluginParameterHint { role: "transients and width", names: pattern(r"(?i)Maximi[sz]er.*(Transient|Upward|Soft ?Clip|Stereo Indep|Independence)"), about: "Transient Emphasis keeps attacks, Upward Compress lifts quiet parts, Soft Clip shaves peaks before limiting, Stereo Independence lets left and right limit apart (wider, a less steady center)." },
                PluginParameterHint { role: "true peak", names: pattern(r"(?i)Maximi[sz]er.*True ?Peak"), about: "On: also catches peaks between samples." },
            ] },
        PluginSection { name: "Equalizer and Dynamic EQ", about: "Bands with frequency, gain, Q and shape. A Dynamic EQ band moves its gain only while its range is past the threshold.",
            parameters: vec![
                // TS: one look-behind with four alternatives; fancy_regex wants each look-behind a fixed width.
                PluginParameterHint { role: "eq gain", names: pattern(r"(?i)(?<!Dynamic )(?<!Match )(?<!Vintage )(?<!Stem )(EQ|Equali[sz]er)\b.*Band.*Gain"), about: "dB per band." },
                PluginParameterHint { role: "eq frequency", names: pattern(r"(?i)(?<!Dynamic )(?<!Match )(?<!Vintage )(?<!Stem )(EQ|Equali[sz]er)\b.*Band.*Freq"), about: "Hz per band." },
                PluginParameterHint { role: "eq q and shape", names: pattern(r"(?i)(?<!Dynamic )(?<!Match )(?<!Vintage )(?<!Stem )(EQ|Equali[sz]er)\b.*Band.*(\bQ\b|Width|Shape|Type)"), about: "Q (higher is narrower) and shape (bell, shelf, cut)." },
                PluginParameterHint { role: "dynamic eq", names: pattern(r"(?i)Dynamic EQ.*Band.*(Threshold|Gain|Freq|\bQ\b|Attack|Release|Mode)"), about: "Threshold (dB), the gain it moves to past it, frequency, Q, timing; cut or boost." },
            ] },
        PluginSection { name: "Dynamics, Imager, Exciter", about: "Up to four bands each, split by crossovers.",
            parameters: vec![
                PluginParameterHint { role: "dynamics", names: pattern(r"(?i)Dynamics.*(Threshold|Ratio|Attack|Release|Knee|Gain)"), about: "Per band: threshold (dB), ratio, attack and release (ms), knee, makeup gain." },
                PluginParameterHint { role: "crossovers", names: pattern(r"(?i)(Dynamics|Imager|Exciter).*(Crossover|Split)"), about: "Where bands split, Hz." },
                PluginParameterHint { role: "width", names: pattern(r"(?i)Imager.*(Width|Stereoi[sz]e)"), about: "Width per band, -100% (mono) to +100%; keep the lowest band at or under 0. Stereoize widens narrow parts; check mono." },
                PluginParameterHint { role: "exciter", names: pattern(r"(?i)Exciter.*(Amount|Drive|Mix|Mode)"), about: "Per band: the saturation mode (warm, tape, tube, retro…), its amount, and Mix." },
            ] },
        PluginSection { name: "Tone and balance", about: "Modules that reshape the whole mix's tone or balance, and the chain's levels.",
            parameters: vec![
                PluginParameterHint { role: "low end focus", names: pattern(r"(?i)Low End Focus.*(Contrast|Gain|Amount|Mode)"), about: "Contrast up tightens and punches the lows; down smooths them." },
                PluginParameterHint { role: "clarity and stabilizer", names: pattern(r"(?i)(Clarity|Stabili[sz]er).*(Amount|Mix|Speed|Tilt)"), about: "Clarity lifts masked detail; Stabilizer rides the tone toward a target as the song changes. A little goes far." },
                PluginParameterHint { role: "master rebalance", names: pattern(r"(?i)Rebalance.*(Vocal|Bass|Drum)"), about: "Level of vocals, bass or drums inside the finished mix, dB." },
                PluginParameterHint { role: "ozone 12 modules", names: pattern(r"(?i)Stem EQ|Unlimiter|Bass Control"), about: "Stem EQ: EQ one part (vocals, bass, drums…) inside the mix. Unlimiter: brings back transients a limiter crushed. Bass Control: tightens and focuses the lows." },
                PluginParameterHint { role: "chain in and out", names: pattern(r"(?i)^(Global[^A-Za-z0-9_]*)?(Input|Output) ?(Gain|Level)$"), about: "dB into and out of the whole chain." },
            ] },
    ],
    recipes: vec![
        PluginRecipe { name: "Streaming master", how: "Maximizer last: Ceiling -1.0 dB, True Peak on, Character 3–5. Lower Threshold until the loudest part limits 2–4 dB. Aim for -14 to -9 LUFS integrated by genre: streaming turns louder masters down." },
        PluginRecipe { name: "Loud club master", how: "Threshold for 4–6 dB of limiting, Character 1–3, some Transient Emphasis to keep punch, Soft Clip on, Ceiling -0.3 to -1.0 dB. Dynamics low band 2:1, attack 30 ms, release 100 ms to steady the bass. Imager's lowest band (under 120 Hz) toward -100%." },
        PluginRecipe { name: "Glue", how: "Dynamics with bands linked: ratio 1.5–2:1, attack 20–30 ms, release 100–200 ms, 1–2 dB of reduction. Exciter in a tape mode at 10–20% mix." },
        PluginRecipe { name: "Harsh top, boomy bottom", how: "Dynamic EQ band at 2.5–5 kHz, Q about 2, cutting up to 3 dB only when it gets loud. Equalizer low shelf -1 to -2 dB at 100–150 Hz, or Low End Focus contrast up for tighter lows." },
        PluginRecipe { name: "Toward a reference", how: "Match EQ: capture the reference and the mix in Ozone's window, apply 30–60% with smoothing. Or run Master Assistant with the reference track. Then listen to both and set the Maximizer for the same loudness." },
    ],
    beyond: "Master Assistant is a button, not a parameter: in Ozone's window (Kumi can open it) the producer picks a target (streaming, CD, or a reference track), starts it, and plays the loudest 10–20 seconds; Ozone builds a chain and sets the Maximizer. Its result is parameters Kumi can then adjust (configure the new ones). Adding, removing and reordering modules, loading reference tracks, Match EQ's captures and presets are in that window too, or the producer's to do.",
    folders: None,
    wavetable: None,
}
});
