//! The player through a fake output (never a real device): what it's handed, at the output's rate and in its channels,
//! one sound at a time.
use kumi_common::abort::Signal;
use kumi_media::audio::{
    decode::float_wav,
    player::{Format, Output, Player, PlayerError},
};
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
};

/// An output that keeps what it's handed.
struct Fake {
    format: Format,
    started: RefCell<Vec<Vec<f32>>>,
    stops: Cell<usize>,
    playing: Cell<bool>,
}

struct Shared(Rc<Fake>);

impl Output for Shared {
    fn format(&self) -> Format {
        self.0.format
    }
    fn start(&self, samples: Vec<f32>) -> Result<(), PlayerError> {
        self.0.started.borrow_mut().push(samples);
        self.0.playing.set(true);
        Ok(())
    }
    fn stop(&self) {
        self.0.stops.set(self.0.stops.get() + 1);
        self.0.playing.set(false);
    }
    fn playing(&self) -> bool {
        self.0.playing.get()
    }
}

fn fake(rate: u32, channels: u16) -> (Rc<Fake>, Player) {
    let fake =
        Rc::new(Fake { format: Format { rate, channels }, started: RefCell::default(), stops: Cell::new(0), playing: Cell::new(false) });
    let output = fake.clone();
    (fake, Player::with_opener(Box::new(move || Ok(Box::new(Shared(output.clone())) as Box<dyn Output>))))
}

/// A second of a tone at 44.1 kHz, mono: 440 Hz, then from half way 1 kHz.
fn tones(folder: &Path) -> PathBuf {
    let rate = 44_100.;
    let mono: Vec<f32> = (0..44_100)
        .map(|n| {
            let t = n as f64 / rate;
            let hz = if t < 0.5 { 440. } else { 1_000. };
            (0.5 * (2. * std::f64::consts::PI * hz * t).sin()) as f32
        })
        .collect();
    let file = folder.join("tones.wav");
    std::fs::write(&file, float_wav(&[mono], 44_100)).unwrap();
    file
}

/// How often a channel's samples cross zero, a second (its pitch, twice over), leaving out its faded ends.
fn crossings_a_second(samples: &[f32], rate: f64) -> f64 {
    let inner = &samples[samples.len() / 20..samples.len() * 19 / 20];
    let crossed = inner.windows(2).filter(|pair| (pair[0] < 0.) != (pair[1] < 0.)).count();
    crossed as f64 / (inner.len() as f64 / rate)
}

fn left(samples: &[f32], channels: usize) -> Vec<f32> {
    samples.chunks(channels).map(|frame| frame[0]).collect()
}

#[tokio::test(flavor = "current_thread")]
async fn a_stretch_of_a_file_plays_at_the_outputs_rate_in_each_of_its_channels() {
    let folder = tempfile::tempdir().unwrap();
    let file = tones(folder.path());
    let (fake, player) = fake(48_000, 2);
    player.play(&file, 0.5, 0.25, Signal::new()).await.unwrap();
    let started = fake.started.borrow();
    assert_eq!(started.len(), 1);
    let samples = &started[0];
    assert_eq!(samples.len(), 12_000 * 2, "a quarter second at 48 kHz, two channels");
    assert!(samples.chunks(2).all(|frame| frame[0] == frame[1]), "one channel heard in both");
    let pitch = crossings_a_second(&left(samples, 2), 48_000.) / 2.;
    assert!((pitch - 1_000.).abs() < 10., "the stretch from half way, its pitch kept: {pitch} Hz");
    let peak = samples.iter().fold(0f32, |peak, value| peak.max(value.abs()));
    assert!((peak - 0.5).abs() < 0.02, "its level kept: {peak}");
    assert!(samples[0].abs() < 0.01 && samples[samples.len() - 1].abs() < 0.01, "faded in and out");
    assert!(player.playing());
}

#[tokio::test(flavor = "current_thread")]
async fn one_sound_at_a_time_a_new_one_takes_over_and_a_stop_ends_it() {
    let folder = tempfile::tempdir().unwrap();
    let file = tones(folder.path());
    let (fake, player) = fake(44_100, 1);
    player.play(&file, 0., 0.1, Signal::new()).await.unwrap();
    player.play(&file, 0.5, 0.1, Signal::new()).await.unwrap();
    assert_eq!(fake.started.borrow().len(), 2, "each handed over in its turn");
    player.stop();
    assert!(!player.playing());
    assert_eq!(fake.stops.get(), 1);
    // A play asked for while another's file is still being read: only the later one plays.
    let (first, second) = tokio::join!(player.play(&file, 0., 0.1, Signal::new()), player.play(&file, 0.5, 0.1, Signal::new()));
    assert_eq!((first, second), (Ok(()), Ok(())), "the one overtaken isn't an error");
    let started = fake.started.borrow().clone();
    assert_eq!(started.len(), 3);
    let pitch = crossings_a_second(&started[2], 44_100.) / 2.;
    assert!((pitch - 1_000.).abs() < 25., "the later one: {pitch} Hz");
    // A stop while a file is still being read: nothing plays.
    let (stopped, _) = tokio::join!(player.play(&file, 0., 0.1, Signal::new()), async {
        tokio::task::yield_now().await;
        player.stop();
    });
    assert_eq!(stopped, Ok(()));
    assert_eq!(fake.started.borrow().len(), 3, "nothing more handed over");
}

#[tokio::test(flavor = "current_thread")]
async fn with_no_output_nothing_plays_and_it_says_so() {
    let opened = Rc::new(Cell::new(0));
    let tried = opened.clone();
    let player = Player::with_opener(Box::new(move || {
        tried.set(tried.get() + 1);
        Err(PlayerError::NoOutput)
    }));
    let folder = tempfile::tempdir().unwrap();
    let file = tones(folder.path());
    let refused = player.play(&file, 0., 0.1, Signal::new()).await;
    assert_eq!(refused, Err(PlayerError::NoOutput));
    assert_eq!(refused.unwrap_err().to_string(), "There's no sound output on this computer to play through.");
    assert!(!player.playing());
    player.stop();
    assert_eq!(player.play(&file, 0., 0.1, Signal::new()).await, Err(PlayerError::NoOutput));
    assert_eq!(opened.get(), 2, "looked for again on the next play: one may have been plugged in");
}

#[tokio::test(flavor = "current_thread")]
async fn a_file_that_cant_be_played_says_why_and_a_stopped_play_says_so() {
    let folder = tempfile::tempdir().unwrap();
    let file = tones(folder.path());
    let (fake, player) = fake(48_000, 2);
    let missing = player.play(folder.path().join("missing.wav"), 0., 1., Signal::new()).await;
    assert!(matches!(missing, Err(PlayerError::File(_))), "{missing:?}");
    let past = player.play(&file, 5., 1., Signal::new()).await;
    assert_eq!(past, Err(PlayerError::File("There's nothing to play there: it's at or past the end of the file.".into())));
    let signal = Signal::new();
    signal.cancel();
    assert_eq!(player.play(&file, 0., 0.1, signal).await, Err(PlayerError::Stopped));
    assert!(fake.started.borrow().is_empty());
}

/// Linux's Kumi is built without its player (ALSA), unless it's built with linux-player: it says so, and opens nothing.
#[cfg(all(target_os = "linux", not(feature = "linux-player")))]
#[tokio::test(flavor = "current_thread")]
async fn linuxs_kumi_without_its_player_says_it_cant_play() {
    let folder = tempfile::tempdir().unwrap();
    let file = tones(folder.path());
    assert_eq!(Player::new().play(&file, 0., 0.1, Signal::new()).await, Err(PlayerError::Off));
}
