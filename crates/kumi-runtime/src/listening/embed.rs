//! Learned embeddings, to catch what the measures miss: LAION-CLAP's music model for how alike two sounds or mixes
//! are in style and vibe, and AFx-Rep for the effects a sound went through. The models run in Kumi's own runtime
//! (`crate::models`, fetched the first time they're needed); this side reads the audio, shapes it as each model takes
//! it, and compares what comes out.

use crate::{
    audio::{decode::open_audio_to, dsp::fft},
    models::{self, pinned, Tensor},
};
use kumi_common::abort::Signal;
use std::path::{Path, PathBuf};

pub use crate::models::Say;

/// CLAP hears 10 s at 48 kHz: 1001 frames of 64 mel bands.
pub const CLAP_RATE: f64 = 48_000.;
pub const CLAP_SAMPLES: usize = 480_000;
pub const CLAP_FRAMES: usize = 1001;
pub const CLAP_MELS: usize = 64;
const CLAP_FFT: usize = 1024;
const CLAP_HOP: usize = 480;
/// AFx-Rep hears stereo at 48 kHz, a window of 262 144 samples (about 5.5 s) at a time.
pub const AFX_RATE: f64 = 48_000.;
pub const AFX_SAMPLES: usize = 262_144;
/// Windows heard at most, spread over what's heard (their embeddings averaged).
const MOST_WINDOWS: usize = 6;
/// The loudness (LUFS) a stretch is heard at by the style model when its own is known: CLAP's log-mel moves with gain,
/// and a change of level alone isn't one of style.
const STYLE_LOUDNESS: f64 = -14.;

/// A Slaney-style mel filter bank (as librosa and CLAP's feature extractor make it): `mels` triangles evenly spaced in
/// Slaney mels from `low` to `high` Hz over `bins` FFT bins up to half of `rate`, each scaled to about equal energy.
/// Rows are bins, columns mel bands.
pub fn slaney_mels(bins: usize, mels: usize, low: f64, high: f64, rate: f64) -> Vec<Vec<f64>> {
    let to_mel = |hz: f64| if hz >= 1000. { 15. + (hz / 1000.).ln() * 27. / 6.4f64.ln() } else { 3. * hz / 200. };
    let to_hz = |mel: f64| if mel >= 15. { 1000. * ((6.4f64.ln() / 27.) * (mel - 15.)).exp() } else { 200. * mel / 3. };
    let (bottom, top) = (to_mel(low), to_mel(high));
    let edges: Vec<f64> = (0..mels + 2).map(|k| to_hz(bottom + (top - bottom) * k as f64 / (mels + 1) as f64)).collect();
    let nyquist = (rate / 2.).floor();
    (0..bins)
        .map(|bin| {
            let hz = nyquist * bin as f64 / (bins - 1) as f64;
            (0..mels)
                .map(|mel| {
                    let down = (edges[mel + 2] - hz) / (edges[mel + 2] - edges[mel + 1]);
                    let up = (hz - edges[mel]) / (edges[mel + 1] - edges[mel]);
                    down.min(up).max(0.) * 2. / (edges[mel + 2] - edges[mel])
                })
                .collect()
        })
        .collect()
}

/// CLAP's input for a window at 48 kHz (`CLAP_FRAMES` × `CLAP_MELS`, frame by frame), as its feature extractor makes
/// it: a short window repeated to fill 10 s, then padded with silence; frames centred (the ends mirrored), a periodic
/// Hann window, power, Slaney mels, then dB.
pub fn clap_features(window: &[f32]) -> Vec<f32> {
    let mut audio: Vec<f64> = window.iter().take(CLAP_SAMPLES).map(|sample| *sample as f64).collect();
    if !audio.is_empty() && audio.len() < CLAP_SAMPLES {
        let repeats = CLAP_SAMPLES / audio.len();
        audio = audio.repeat(repeats);
    }
    audio.resize(CLAP_SAMPLES, 0.);
    // Mirrored half a window at each end, as frames are centred.
    let half = CLAP_FFT / 2;
    let mut padded = Vec::with_capacity(CLAP_SAMPLES + CLAP_FFT);
    padded.extend((1..=half).rev().map(|k| audio[k]));
    padded.extend_from_slice(&audio);
    padded.extend((1..=half).map(|k| audio[CLAP_SAMPLES - 1 - k]));
    let hann: Vec<f64> = (0..CLAP_FFT).map(|n| 0.5 - 0.5 * (2. * std::f64::consts::PI * n as f64 / CLAP_FFT as f64).cos()).collect();
    let filters = slaney_mels(CLAP_FFT / 2 + 1, CLAP_MELS, 50., 14_000., CLAP_RATE);
    let (mut re, mut im) = (vec![0.; CLAP_FFT], vec![0.; CLAP_FFT]);
    let mut features = Vec::with_capacity(CLAP_FRAMES * CLAP_MELS);
    for frame in 0..CLAP_FRAMES {
        let start = frame * CLAP_HOP;
        for n in 0..CLAP_FFT {
            re[n] = padded[start + n] * hann[n];
            im[n] = 0.;
        }
        fft(&mut re, &mut im);
        let power: Vec<f64> = (0..=CLAP_FFT / 2).map(|bin| re[bin] * re[bin] + im[bin] * im[bin]).collect();
        for mel in 0..CLAP_MELS {
            let energy: f64 = power.iter().zip(&filters).map(|(power, row)| power * row[mel]).sum();
            features.push((10. * energy.max(1e-10).log10()) as f32);
        }
    }
    features
}

/// Audio from one rate to another: a windowed sinc (Blackman, 32 zero crossings a side), its cutoff under the lower
/// rate's half. Between whole rates, each output sample's kernel is one of a few made once (polyphase: 44.1 to 48 kHz
/// is 160 kernels of 69 taps, 44.1 to 22.05 kHz one).
pub fn resample(input: &[f32], from: f64, to: f64) -> Vec<f32> {
    if (from - to).abs() < 1e-6 || input.is_empty() {
        return input.to_vec();
    }
    let ratio = to / from;
    // Cutoff in cycles per input sample.
    let cutoff = 0.5 * ratio.min(1.) * 0.95;
    let width = (32. / (2. * cutoff)).ceil();
    let length = (input.len() as f64 * ratio).round() as usize;
    let weight = |x: f64| {
        let y = 2. * cutoff * x;
        let sinc = if y.abs() < 1e-12 { 1. } else { (std::f64::consts::PI * y).sin() / (std::f64::consts::PI * y) };
        let phase = std::f64::consts::PI * x / width;
        2. * cutoff * sinc * (0.42 + 0.5 * phase.cos() + 0.08 * (2. * phase).cos())
    };
    // An output sample sits `up` phases between input samples, stepping `down` phases a sample.
    let phases = (from.fract() == 0. && to.fract() == 0. && from >= 1. && to >= 1.)
        .then(|| {
            let (from, to) = (from as u64, to as u64);
            let (mut a, mut b) = (from, to);
            while b != 0 {
                (a, b) = (b, a % b);
            }
            (to / a, from / a)
        })
        .filter(|(up, _)| *up <= 4096);
    let Some((up, down)) = phases else {
        return (0..length)
            .map(|n| {
                let at = n as f64 / ratio;
                let (first, last) = ((at - width).ceil().max(0.) as usize, ((at + width).floor() as usize).min(input.len() - 1));
                (first..=last).map(|k| input[k] as f64 * weight(at - k as f64)).sum::<f64>() as f32
            })
            .collect();
    };
    // Each phase's kernel: where its taps start against the input sample before it, and their weights.
    let kernels: Vec<(i64, Vec<f64>)> = (0..up)
        .map(|phase| {
            let offset = phase as f64 / up as f64;
            let (first, last) = ((offset - width).ceil() as i64, (offset + width).floor() as i64);
            (first, (first..=last).map(|tap| weight(offset - tap as f64)).collect())
        })
        .collect();
    (0..length as u64)
        .map(|n| {
            let before = (n * down / up) as i64;
            let (first, taps) = &kernels[(n * down % up) as usize];
            let mut sum = 0.;
            for (tap, weight) in taps.iter().enumerate() {
                let at = before + first + tap as i64;
                if at >= 0 && (at as usize) < input.len() {
                    sum += input[at as usize] as f64 * weight;
                }
            }
            sum as f32
        })
        .collect()
}

/// How far apart two embeddings point: 0 alike, 1 unrelated, 2 opposite.
pub fn distance(a: &[f32], b: &[f32]) -> Option<f64> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let (mut dot, mut aa, mut bb) = (0., 0., 0.);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (*x as f64, *y as f64);
        dot += x * y;
        aa += x * x;
        bb += y * y;
    }
    (aa > 0. && bb > 0.).then(|| 1. - dot / (aa.sqrt() * bb.sqrt()))
}

/// To unit length.
pub fn normalized(mut vector: Vec<f32>) -> Vec<f32> {
    let length = vector.iter().map(|value| (*value as f64).powi(2)).sum::<f64>().sqrt();
    if length > 0. {
        vector.iter_mut().for_each(|value| *value = (*value as f64 / length) as f32);
    }
    vector
}

/// The mean of unit embeddings, back to unit length.
pub fn averaged(all: &[Vec<f32>]) -> Option<Vec<f32>> {
    let first = all.first()?;
    let mut sum = vec![0f32; first.len()];
    for vector in all {
        for (total, value) in sum.iter_mut().zip(normalized(vector.clone())) {
            *total += value;
        }
    }
    Some(normalized(sum))
}

/// Up to `count` windows of `length` frames spread evenly over `total` frames: their starts.
fn windows(total: usize, length: usize, count: usize) -> Vec<usize> {
    if total <= length {
        return vec![0];
    }
    let room = total - length;
    let count = count.min(total / length).max(1);
    (0..count).map(|k| if count == 1 { room / 2 } else { room * k / (count - 1) }).collect()
}

/// Up to `count` windows of `length` seconds spread over a stretch of a file, each as channels at `rate` (stereo when
/// it has two or more). Only the windows are read, and they're resampled off the app's thread. Silence is refused.
async fn read_windows(
    file: &Path,
    (start, seconds): (f64, f64),
    (length, count): (f64, usize),
    rate: f64,
    signal: &Signal,
) -> Result<Vec<Vec<Vec<f32>>>, String> {
    let mut source = open_audio_to(file, Some(signal.clone()), Some(start + seconds + 1.)).await.map_err(|error| error.to_string())?;
    let native = source.sample_rate;
    let channels = source.channels.clamp(1, 2);
    let total = (seconds * native).round() as usize;
    let span = (length * native).round() as usize;
    let mut raw: Vec<Vec<Vec<f32>>> = vec![];
    for at in windows(total, span, count) {
        source.seek(start * native + at as f64);
        let wanted = span.min(total.saturating_sub(at)).max(1);
        let mut audio: Vec<Vec<f32>> = vec![vec![]; channels];
        while audio[0].len() < wanted {
            let Some(chunk) = source.read((wanted - audio[0].len()).min(65_536)).await.map_err(|error| error.to_string())? else { break };
            for (channel, samples) in audio.iter_mut().enumerate() {
                samples.extend_from_slice(&chunk[channel.min(chunk.len() - 1)]);
            }
        }
        raw.push(audio);
    }
    let _ = source.close().await;
    if raw.iter().flatten().flatten().all(|sample| sample.abs() < 1e-4) {
        return Err("there's only silence there".into());
    }
    let signal = signal.clone();
    tokio::task::spawn_blocking(move || {
        raw.into_iter()
            .map(|window| match signal.is_cancelled() {
                true => Err("stopped".to_string()),
                false => Ok(window.iter().map(|channel| resample(channel, native, rate)).collect()),
            })
            .collect()
    })
    .await
    .map_err(|error| error.to_string())?
}

/// The style-and-vibe model to run: the embeddings slot's file when it holds one, else Kumi's own, fetched the first
/// time. A slot's file that's gone since gives way to Kumi's own, said once.
async fn clap_model(slot: Option<PathBuf>, say: Say<'_>, signal: &Signal) -> Result<PathBuf, String> {
    if let Some(file) = slot {
        if file.is_file() {
            return Ok(file);
        }
        if GONE.lock().map(|mut said| said.insert(file.clone())).unwrap_or(true) {
            say(&format!(
                "The embeddings slot's model file {} is gone, so Kumi's own style model hears in its place: /slots embeddings default swaps back to it for good.",
                file.display()
            ));
        }
    }
    let path = models::dir().join(pinned::CLAP.name);
    models::fetch(&pinned::CLAP, &path, "its style model, LAION-CLAP", say, signal).await?;
    Ok(path)
}

/// The style model, ready to run: the runtime and the model fetched when they aren't here yet.
pub async fn style_model(slot: Option<PathBuf>, say: Say<'_>, signal: &Signal) -> Result<(), String> {
    models::runtime(say, signal).await?;
    clap_model(slot, say, signal).await.map(|_| ())
}

/// The slot files said to be gone, each said once.
static GONE: std::sync::Mutex<std::collections::BTreeSet<PathBuf>> = std::sync::Mutex::new(std::collections::BTreeSet::new());

/// Which style model a slot's choice runs, as recorded with every vector it makes: the model file's SHA-256, Kumi's own
/// known from its pin (when the slot holds no file, or its file is gone) and a slot's file hashed once. Vectors compare
/// only with ones the same model made: two models' numbers don't mean the same thing, however alike their length.
pub async fn style_id(slot: Option<&Path>) -> String {
    match slot.filter(|file| file.is_file()) {
        Some(file) => file_id(file).await,
        None => own_style_id(),
    }
}

/// Kumi's own style model's identity.
pub fn own_style_id() -> String {
    format!("sha256:{}", pinned::CLAP.sha256)
}

/// A model file's identity, its SHA-256, hashed once for its size and time changed.
async fn file_id(file: &Path) -> String {
    type Hashed = std::collections::BTreeMap<(PathBuf, u64, Option<std::time::SystemTime>), String>;
    static HASHED: std::sync::Mutex<Hashed> = std::sync::Mutex::new(std::collections::BTreeMap::new());
    let meta = std::fs::metadata(file).ok();
    let key = (file.to_path_buf(), meta.as_ref().map_or(0, |meta| meta.len()), meta.and_then(|meta| meta.modified().ok()));
    if let Some(known) = HASHED.lock().ok().and_then(|hashed| hashed.get(&key).cloned()) {
        return known;
    }
    let path = file.to_path_buf();
    let hashed = tokio::task::spawn_blocking(move || -> Option<String> {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        std::io::copy(&mut std::io::BufReader::new(std::fs::File::open(&path).ok()?), &mut hash).ok()?;
        Some(format!("sha256:{}", hex::encode(hash.finalize())))
    })
    .await
    .ok()
    .flatten();
    let id = hashed.unwrap_or_else(|| format!("file:{}", file.display()));
    if let Ok(mut known) = HASHED.lock() {
        known.insert(key, id.clone());
    }
    id
}

/// What a stretch of a file sounds like to CLAP, by style and vibe: up to six 10 s windows, averaged, unit length, and
/// which model heard it (`style_id`: the slot's file, or Kumi's own when the slot holds none or its file is gone).
/// `loudness` is the stretch's integrated loudness (LUFS) when it's known, and the stretch is then heard at
/// `STYLE_LOUDNESS`, so takes and references at different levels compare by their style alone.
pub async fn vibe(
    file: &Path,
    start: f64,
    seconds: f64,
    loudness: Option<f64>,
    slot: Option<PathBuf>,
    say: Say<'_>,
    signal: &Signal,
) -> Result<(Vec<f32>, String), String> {
    models::runtime(say, signal).await?;
    let model = clap_model(slot.clone(), say, signal).await?;
    let id = if slot.as_deref() == Some(model.as_path()) { file_id(&model).await } else { own_style_id() };
    let heard = read_windows(file, (start, seconds), (CLAP_SAMPLES as f64 / CLAP_RATE, MOST_WINDOWS), CLAP_RATE, signal).await?;
    let gain = loudness.filter(|loudness| loudness.is_finite()).map_or(1., |loudness| 10f64.powf((STYLE_LOUDNESS - loudness) / 20.)) as f32;
    // Each window as one channel, its log-mel made off the app's thread.
    let features: Vec<Vec<f32>> = tokio::task::spawn_blocking(move || {
        heard
            .iter()
            .map(|window| {
                let mono: Vec<f32> = (0..window[0].len())
                    .map(|n| gain * window.iter().map(|channel| channel[n]).sum::<f32>() / window.len() as f32)
                    .collect();
                clap_features(&mono)
            })
            .collect()
    })
    .await
    .map_err(|error| error.to_string())?;
    let mut all = vec![];
    for features in features {
        let input = Tensor { shape: vec![1, 1, CLAP_FRAMES, CLAP_MELS], data: features };
        let out = models::run(&model, vec![("input_features".into(), input)], vec!["audio_embeds".into()]).await?;
        all.push(out.into_iter().next().map(|tensor| tensor.data).unwrap_or_default());
    }
    averaged(&all).map(|vector| (vector, id)).ok_or_else(|| "Nothing was heard to embed.".into())
}

/// What a stretch of a file's effects sound like to AFx-Rep: its mid's and side's embeddings side by side, each unit
/// length (up to six windows, averaged).
pub async fn effects(file: &Path, start: f64, seconds: f64, say: Say<'_>, signal: &Signal) -> Result<Vec<f32>, String> {
    models::runtime(say, signal).await?;
    let model = models::dir().join(pinned::AFX_REP.name);
    models::fetch(&pinned::AFX_REP, &model, "its effects model, AFx-Rep", say, signal).await?;
    let heard = read_windows(file, (start, seconds), (AFX_SAMPLES as f64 / AFX_RATE, MOST_WINDOWS), AFX_RATE, signal).await?;
    let (mut mids, mut sides) = (vec![], vec![]);
    for window in &heard {
        let (left, right) = (&window[0], window.get(1).unwrap_or(&window[0]));
        let length = left.len().min(right.len()).min(AFX_SAMPLES);
        // Each window at its own peak, as the model was trained.
        let peak = left[..length].iter().chain(&right[..length]).fold(1e-8f32, |peak, sample| peak.max(sample.abs()));
        let mut data: Vec<f32> = left[..length].iter().map(|sample| sample / peak).collect();
        data.extend(right[..length].iter().map(|sample| sample / peak));
        let input = Tensor { shape: vec![1, 2, length], data };
        let out = models::run(&model, vec![("audio".into(), input)], vec!["mid".into(), "side".into()]).await?;
        let mut out = out.into_iter();
        mids.push(out.next().map(|tensor| tensor.data).unwrap_or_default());
        sides.push(out.next().map(|tensor| tensor.data).unwrap_or_default());
    }
    let (Some(mid), Some(side)) = (averaged(&mids), averaged(&sides)) else { return Err("Nothing was heard to embed.".into()) };
    Ok(mid.into_iter().chain(side).collect())
}

/// A style model from a link (a Hugging Face file, say) for the embeddings slot, fetched over https into Kumi's models
/// folder (a folder per link, so two called model.onnx stay apart): where it's kept. `hf:<owner>/<repo>/<file>` is that
/// file on the repo's main branch, and a Hugging Face page for a file (…/blob/…) is fetched as the file (…/resolve/…).
pub async fn fetch_link(link: &str, signal: &Signal) -> Result<PathBuf, String> {
    let url = match link.strip_prefix("hf:") {
        Some(rest) => match rest.trim_start_matches('/').splitn(3, '/').collect::<Vec<_>>()[..] {
            [owner, repo, file] => format!("https://huggingface.co/{owner}/{repo}/resolve/main/{file}"),
            _ => return Err(format!("{link} names no file: hf:<owner>/<repo>/<file>.onnx is one.")),
        },
        None if link.starts_with("hf.co/") || link.starts_with("huggingface.co/") => format!("https://{link}"),
        None => link.to_string(),
    };
    let url = if url.starts_with("https://huggingface.co/") || url.starts_with("https://hf.co/") {
        url.replacen("/blob/", "/resolve/", 1)
    } else {
        url
    };
    let path = link_path(&url);
    if !path.starts_with(models::dir().join("slots")) {
        return Err(format!("Kumi wouldn't keep the model from {link} outside its models folder."));
    }
    models::fetch_unpinned(&url, &path, signal).await?;
    Ok(path)
}

/// Where a model fetched from `url` is kept: in a folder of its own under models/slots, by the file's own name when
/// that's a plain .onnx name (letters, digits, dots, dashes and underscores, so no `..\` or drive reaches outside
/// it), else as model.onnx.
pub fn link_path(url: &str) -> PathBuf {
    let last = url.split(['?', '#']).next().unwrap_or(url).rsplit(['/', '\\']).next().unwrap_or("");
    let plain = |name: &&str| {
        name.ends_with(".onnx") && !name.starts_with('.') && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    };
    let name = Some(last).filter(plain).unwrap_or("model.onnx");
    let tag: String = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(url.as_bytes()))[..12].to_string()
    };
    models::dir().join("slots").join(tag).join(name)
}

/// The quick test for a style model in the embeddings slot: two known tones, a plain one and a bright one, have to
/// come out as embeddings that tell them apart. It takes CLAP's input (`input_features`, a 10 s log-mel) and gives
/// `audio_embeds`.
pub async fn tells_tones_apart(model: &Path, say: Say<'_>, signal: &Signal) -> Result<String, String> {
    models::runtime(say, signal).await?;
    let tone = |bright: bool| -> Vec<f32> {
        let partials = if bright { 40 } else { 1 };
        (0..(2. * CLAP_RATE) as usize)
            .map(|n| {
                let t = n as f64 / CLAP_RATE;
                (1..=partials).map(|k| (2. * std::f64::consts::PI * 220. * k as f64 * t).sin() / k as f64).sum::<f64>() as f32 * 0.2
            })
            .collect()
    };
    let mut heard = vec![];
    for bright in [false, true] {
        let input = Tensor { shape: vec![1, 1, CLAP_FRAMES, CLAP_MELS], data: clap_features(&tone(bright)) };
        let out = models::run(model, vec![("input_features".into(), input)], vec!["audio_embeds".into()])
            .await
            .map_err(|why| format!("it doesn't run as a style model in this slot does (input_features in, audio_embeds out): {why}"))?;
        let embedding = out.into_iter().next().map(|tensor| tensor.data).unwrap_or_default();
        if embedding.len() < 16 || embedding.iter().any(|value| !value.is_finite()) {
            return Err("what it gives isn't an embedding".into());
        }
        heard.push(embedding);
    }
    let apart = distance(&heard[0], &heard[1]).ok_or("its embeddings can't be compared")?;
    if apart < 0.001 {
        return Err(format!("it heard a plain and a bright tone as the same (distance {apart:.4})"));
    }
    Ok(format!("it told a plain tone from a bright one (distance {apart:.2})"))
}
