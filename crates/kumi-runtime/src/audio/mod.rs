pub mod analyze;
pub mod decode;
pub mod dsp;
pub mod matching;
pub mod structure;
pub mod tools;
pub mod wavetable;
pub mod worker;

pub use analyze::{analyze_file, Analysis, AnalyzeOptions, SoundAnalysis, ANALYSIS_VERSION, BANDS};
pub use decode::{open_audio, AudioError, AUDIO_EXTENSIONS};
use kumi_common::{
    abort::SignalExt,
    js::number::{round, to_fixed, to_string},
};
pub use matching::{closeness, Closeness, Feature};
use serde::{Deserialize, Serialize};

/// Analyze a file in a worker thread; stopping also stops format conversion and removes its copy.
pub async fn hear(path: &str, options: AnalyzeOptions) -> Result<Analysis, AudioError> {
    if let Some(signal) = &options.signal {
        signal.check()?;
    }
    let prepared = decode::prepare_audio(path, options.signal.clone()).await?;
    let result = worker::in_worker(prepared.path.to_string_lossy().into_owned(), options, path.into(), prepared.format.clone()).await;
    prepared.cleanup().await;
    result
}
/// Relative paths start at the home folder, and `~` names that folder.
pub fn audio_path(value: &str) -> String {
    let trimmed = kumi_common::js::string::trim(value);
    let home = home::home_dir().unwrap_or_default();
    let path = if trimmed == "~" || trimmed.starts_with("~/") || trimmed.starts_with("~\\") {
        home.join(trimmed[1..].trim_start_matches(['\\', '/']))
    } else {
        let path = std::path::Path::new(trimmed);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            home.join(path)
        }
    };
    path.to_string_lossy().into_owned()
}
macro_rules! object {($name:ident {$($field:ident : $ty:ty),*$(,)?})=>{#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]#[serde(rename_all="camelCase")]pub struct $name{$(pub $field:$ty),*}}}
object!(ComparisonLoudness{mine_lufs:Option<f64>,reference_lufs:Option<f64>,difference_lu:Option<f64>,true_peak:[f64;2],range:[Option<f64>;2]});
object!(ComparisonBand { band: String, hz: String, difference: f64 });
object!(ComparisonWidth { band: String, mine: f64, reference: f64 });
object!(ComparisonStereo { correlation: [f64; 2], low_end_mono: [bool; 2] });
object!(ComparisonDynamics{crest_db:[f64;2],peak_to_loudness_db:[Option<f64>;2],onsets_per_second:[f64;2]});
object!(Comparison{kumi_audio_comparison:u32,mine:String,reference:String,headlines:Vec<String>,loudness:ComparisonLoudness,balance:Vec<ComparisonBand>,tilt:[f64;2],width:Vec<ComparisonWidth>,stereo:Option<ComparisonStereo>,dynamics:ComparisonDynamics,tempo:[Option<f64>;2],key:[Option<String>;2]});
fn signed(value: f64) -> String {
    format!(
        "{}{}",
        if value > 0.0 {
            "+"
        } else if value < 0.0 {
            "−"
        } else {
            "±"
        },
        to_fixed(value.abs(), 1)
    )
}
/// Loudness-matched differences between an analysis and a reference.
pub fn compare(mine: &Analysis, reference: &Analysis) -> Comparison {
    let balance: Vec<_> = mine
        .balance
        .bands
        .iter()
        .zip(&reference.balance.bands)
        .map(|(b, r)| ComparisonBand { band: b.name.clone(), hz: b.hz.clone(), difference: round((b.db - r.db) * 10.0) / 10.0 })
        .collect();
    let width: Vec<_> = mine
        .balance
        .bands
        .iter()
        .zip(&reference.balance.bands)
        .map(|(b, r)| ComparisonWidth { band: b.name.clone(), mine: b.width, reference: r.width })
        .filter(|b| (b.mine - b.reference).abs() >= 0.08)
        .collect();
    let mut headlines = Vec::new();
    let difference = mine.loudness.integrated_lufs.zip(reference.loudness.integrated_lufs).map(|(m, r)| round((m - r) * 10.0) / 10.0);
    if let Some(diff) = difference.filter(|v| v.abs() >= 1.0) {
        headlines.push((
            format!(
                "{} LU {} overall ({} vs {} LUFS)",
                to_fixed(diff.abs(), 1),
                if diff < 0.0 { "quieter" } else { "louder" },
                to_string(mine.loudness.integrated_lufs.unwrap()),
                to_string(reference.loudness.integrated_lufs.unwrap())
            ),
            diff.abs(),
        ));
    }
    for band in &balance {
        if band.difference.abs() >= 1.5 {
            headlines.push((
                format!(
                    "{} ({} Hz) {} dB {} the reference",
                    band.band,
                    band.hz,
                    signed(band.difference),
                    if band.difference > 0.0 { "over" } else { "under" }
                ),
                band.difference.abs(),
            ));
        }
    }
    let tilt = mine.balance.tilt_db_per_octave - reference.balance.tilt_db_per_octave;
    if tilt.abs() >= 0.7 {
        headlines.push((
            format!(
                "{} overall (tilt {} vs {} dB/octave)",
                if tilt > 0.0 { "brighter" } else { "darker" },
                to_string(mine.balance.tilt_db_per_octave),
                to_string(reference.balance.tilt_db_per_octave)
            ),
            tilt.abs() * 2.0,
        ));
    }
    for band in &width {
        headlines.push((
            format!(
                "{} {} (width {} vs {})",
                band.band,
                if band.mine > band.reference { "wider" } else { "narrower" },
                to_string(band.mine),
                to_string(band.reference)
            ),
            (band.mine - band.reference).abs() * 8.0,
        ));
    }
    let crest = mine.dynamics.crest_db - reference.dynamics.crest_db;
    if crest.abs() >= 2.0 {
        headlines.push((
            format!(
                "{} (crest {} vs {} dB)",
                if crest > 0.0 { "more dynamic, less compressed" } else { "more compressed" },
                to_string(mine.dynamics.crest_db),
                to_string(reference.dynamics.crest_db)
            ),
            crest.abs(),
        ));
    }
    let stereo = mine.stereo.as_ref().zip(reference.stereo.as_ref()).map(|(m, r)| {
        if m.low_end_mono != r.low_end_mono {
            headlines.push((
                format!("low end {}", if m.low_end_mono { "mono, the reference's isn't" } else { "not mono, the reference's is" }),
                2.0,
            ));
        }
        ComparisonStereo { correlation: [m.correlation, r.correlation], low_end_mono: [m.low_end_mono, r.low_end_mono] }
    });
    headlines.sort_by(|a, b| b.1.total_cmp(&a.1));
    Comparison {
        kumi_audio_comparison: 1,
        mine: mine.file.clone(),
        reference: reference.file.clone(),
        headlines: headlines.into_iter().take(8).map(|h| h.0).collect(),
        loudness: ComparisonLoudness {
            mine_lufs: mine.loudness.integrated_lufs,
            reference_lufs: reference.loudness.integrated_lufs,
            difference_lu: difference,
            true_peak: [mine.loudness.true_peak_dbtp, reference.loudness.true_peak_dbtp],
            range: [mine.loudness.range_lu, reference.loudness.range_lu],
        },
        balance,
        tilt: [mine.balance.tilt_db_per_octave, reference.balance.tilt_db_per_octave],
        width,
        stereo,
        dynamics: ComparisonDynamics {
            crest_db: [mine.dynamics.crest_db, reference.dynamics.crest_db],
            peak_to_loudness_db: [mine.dynamics.peak_to_loudness_db, reference.dynamics.peak_to_loudness_db],
            onsets_per_second: [mine.dynamics.onsets_per_second, reference.dynamics.onsets_per_second],
        },
        tempo: [mine.tempo.as_ref().map(|t| t.bpm), reference.tempo.as_ref().map(|t| t.bpm)],
        key: [mine.key.as_ref().map(|k| k.name.clone()), reference.key.as_ref().map(|k| k.name.clone())],
    }
}

pub use structure::{hear_form, Form, FormSection};
