//! How a take's render sounded (M7): its peak, its level and where its hits start, against the source's render. A take that is
//! silent, or clips where the source doesn't, can't be shown as it is; one that clips is turned down to the source's peak
//! (#407). Its hits against the source's say how much of its rhythm it keeps.
use kumi_common::abort::Signal;
use kumi_types::contracts::Render;

/// What a render sounded like: its peak and level (dBFS) and where its hits start (s).
#[derive(Debug, Clone, PartialEq)]
pub struct Heard {
    pub peak: f64,
    pub level: f64,
    pub onsets: Vec<f64>,
}

/// Quieter than this throughout is silent; a peak this close to full scale clips.
pub const SILENT_DB: f64 = -60.;
pub const CLIPS_DB: f64 = -0.1;
/// Where a take that clips is turned down to when no source was heard to match (dBFS).
pub const UNDER_FULL_SCALE_DB: f64 = -1.;
/// Onsets this close (s) are the same hit.
const SAME_HIT_S: f64 = 0.03;
/// A take louder than a source that clips by this much clips by itself.
const LOUDER_DB: f64 = 0.5;

/// A level in dB as Kumi writes it: a true minus.
pub fn db_text(db: f64) -> String {
    format!("{db:.1}").replace('-', "\u{2212}")
}

/// Why a take as heard can't be shown as it is.
#[derive(Debug, Clone, PartialEq)]
pub enum Fault {
    /// Nothing in it sounds, where the source does.
    Silent,
    /// It clips where the source doesn't, or is louder than a source that clips: its peak, and how many dB quieter it peaks
    /// where the source does (`to_source`), or 1 dB under full scale when no source that sounds was heard; to a tenth of a dB,
    /// rounded up.
    Clips { peak: f64, over: f64, to_source: bool },
}

impl Fault {
    pub fn words(&self) -> String {
        match self {
            Fault::Silent => "is silent".into(),
            Fault::Clips { peak, .. } => format!("clips (peak {} dBFS)", db_text(*peak)),
        }
    }
}

impl Heard {
    pub fn of(samples: &[f32], rate: f64) -> Heard {
        let db = |value: f64| 20. * value.max(1e-9).log10();
        let peak = samples.iter().fold(0f32, |peak, sample| peak.max(sample.abs())) as f64;
        let power = samples.iter().map(|sample| (*sample as f64).powi(2)).sum::<f64>() / samples.len().max(1) as f64;
        Heard { peak: db(peak), level: db(power.sqrt()), onsets: kumi_listening::notes::onsets(samples, rate) }
    }

    /// Why it can't be shown as it is, or None: silent where the source isn't, or clipping where the source doesn't (a source
    /// that clips makes no take turn down for clipping as much as it does).
    pub fn fault(&self, source: Option<&Heard>) -> Option<Fault> {
        let source_silent = source.is_some_and(|source| source.level < SILENT_DB);
        if self.level < SILENT_DB && !source_silent {
            return Some(Fault::Silent);
        }
        let as_loud_as_a_clipping_source = source.is_some_and(|source| source.peak >= CLIPS_DB && self.peak <= source.peak + LOUDER_DB);
        if self.peak >= CLIPS_DB && !as_loud_as_a_clipping_source {
            let to = source.filter(|_| !source_silent).map(|source| source.peak);
            // (a hair under the tenth, so a float's last bit doesn't round a whole tenth more)
            let over = ((self.peak - to.unwrap_or(UNDER_FULL_SCALE_DB)) * 10. - 1e-9).ceil().max(1.) / 10.;
            return Some(Fault::Clips { peak: self.peak, over, to_source: to.is_some() });
        }
        None
    }

    /// How much of its rhythm it shares with the source's render, 0 to 1: hits within 30 ms of one of the source's.
    pub fn family(&self, source: &Heard) -> f64 {
        let most = self.onsets.len().max(source.onsets.len());
        if most == 0 {
            return 1.;
        }
        let shared = self.onsets.iter().filter(|at| source.onsets.iter().any(|other| (*at - other).abs() <= SAME_HIT_S)).count();
        shared as f64 / most as f64
    }

    pub fn words(&self, family: Option<f64>, source: Option<&Heard>) -> String {
        let mut peak = format!("peak {} dBFS", db_text(self.peak));
        if source.is_some_and(|source| source.peak >= CLIPS_DB) && self.peak >= CLIPS_DB {
            peak.push_str(", clipping as the source does");
        }
        match family {
            Some(family) => format!("{peak}, {:.0}% of its hits where the source's are", family * 100.),
            None => peak,
        }
    }
}

/// A render's part heard: mono, from where the part starts, as long as it lasts (two minutes at most).
pub async fn heard_of(render: &Render, signal: &Signal) -> Result<Heard, String> {
    let mut source = kumi_media::audio::decode::open_audio(&render.file, Some(signal.clone())).await.map_err(|error| error.0)?;
    let rate = source.sample_rate;
    source.seek(render.start * rate);
    let most = render.seconds.map_or(60. * rate, |seconds| seconds * rate).min(120. * rate) as usize;
    let mut samples: Vec<f32> = Vec::new();
    while samples.len() < most {
        let Ok(Some(block)) = source.read(65536.min(most - samples.len())).await else { break };
        let right = block.get(1).unwrap_or(&block[0]);
        samples.extend(block[0].iter().zip(right).map(|(left, right)| (left + right) / 2.));
    }
    let _ = source.close().await;
    if samples.is_empty() {
        return Err("the render was empty".into());
    }
    Ok(Heard::of(&samples, rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heard(peak: f64, level: f64) -> Heard {
        Heard { peak, level, onsets: vec![] }
    }

    #[test]
    fn a_take_is_faulted_for_silence_or_clipping_only_where_the_source_isnt() {
        assert_eq!(heard(-70., -80.).fault(Some(&heard(-6., -18.))), Some(Fault::Silent));
        assert_eq!(Fault::Silent.words(), "is silent");
        assert_eq!(heard(-70., -80.).fault(Some(&heard(-70., -80.))), None, "a silent source makes a silent take no fault");
        assert!(heard(0., -10.).fault(Some(&heard(-6., -18.))).unwrap().words().starts_with("clips (peak"));
        assert_eq!(heard(0., -10.).fault(Some(&heard(0., -10.))), None, "as loud as a source that clips");
        assert!(heard(1.2, -8.).fault(Some(&heard(0., -10.))).is_some(), "louder than a clipping source clips by itself");
        assert_eq!(heard(-3., -15.).fault(None), None);
        assert!(heard(0., -10.).words(None, Some(&heard(0., -10.))).contains("clipping as the source does"));
    }

    /// (#407) A take that clips is turned down by how far it peaks over where the source peaks, to a tenth of a dB: a hot
    /// source's takes to its own peak, a clipping source's to its, and with no source that sounds, to 1 dB under full scale.
    #[test]
    fn a_take_that_clips_is_turned_down_to_where_the_source_peaks() {
        let clips = |take: Heard, source: Option<Heard>| match take.fault(source.as_ref()) {
            Some(Fault::Clips { over, to_source, .. }) => (over, to_source),
            other => panic!("{other:?}"),
        };
        assert_eq!(clips(heard(1.1, -9.), Some(heard(-0.3, -12.))), (1.4, true), "P31's hot footstep");
        assert_eq!(clips(heard(1.13, -9.), Some(heard(-0.3, -12.))), (1.5, true), "rounded up: never short of the source's peak");
        assert_eq!(clips(heard(0., -10.), Some(heard(-6., -18.))), (6., true));
        assert_eq!(clips(heard(1.2, -8.), Some(heard(0., -10.))), (1.2, true), "to a clipping source's peak");
        assert_eq!(clips(heard(0., -10.), None), (1., false), "no source heard: 1 dB under full scale");
        assert_eq!(clips(heard(0.5, -10.), Some(heard(-90., -100.))), (1.5, false), "a silent source has no peak to match");
        assert_eq!(heard(1.1, -9.).fault(None).unwrap().words(), "clips (peak 1.1 dBFS)");
    }

    #[test]
    fn a_takes_family_is_the_share_of_its_hits_where_the_sources_are() {
        let source = Heard { peak: -6., level: -18., onsets: vec![0., 0.5, 1.0, 1.5] };
        let take = Heard { peak: -6., level: -18., onsets: vec![0.01, 0.5, 1.25, 1.5] };
        assert!((take.family(&source) - 0.75).abs() < 1e-9);
        assert_eq!(heard(-6., -18.).family(&heard(-6., -18.)), 1.);
    }
}
