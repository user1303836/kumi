//! Variations (M7): numbered takes V1…Vn of one source and the keep/change loop over them, built once for every DAW. Core owns
//! the model's tool and its answers (`tool`), the take sets and where they're kept, the checks, and the seeded draw that re-rolls
//! a take too close to its source or to another take, or too far from it to belong to it; how a render sounds (`heard`). A DAW
//! plugs in a backend (`backend`) with only its own facts: what its material is and how far apart two are, its operators, and
//! how it reads, writes, finds, renders and drops takes. Nothing here knows a DAW.
use std::collections::BTreeSet;

pub mod backend;
#[cfg(test)]
mod fake_backend_tests;
pub mod heard;
pub mod tool;

/// The most a take's name may be (Renoise's phrase names hold 64 characters).
pub const NAME_MOST: usize = 64;

/// A take's name in the DAW: "V3 · <source>".
pub fn take_name(number: usize, source: &str) -> String {
    clip(&format!("V{number} \u{b7} {source}"), NAME_MOST)
}

/// A kept take's name: "V3 · <source> · kept", the source cut short so the mark always fits.
pub fn kept_name(number: usize, source: &str) -> String {
    let mark = " \u{b7} kept";
    let head = format!("V{number} \u{b7} ");
    let room = NAME_MOST.saturating_sub(head.chars().count() + mark.chars().count());
    format!("{head}{}{mark}", clip(source, room))
}

fn clip(text: &str, most: usize) -> String {
    text.chars().take(most).collect()
}

/// A small, fast generator whose sequence a seed fixes (SplitMix64): the same seed makes the same take.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number from 0 to below `n` (0 when `n` is 0).
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        (self.next_u64() % n as u64) as usize
    }

    /// True with probability `p` (0 to 1).
    pub fn chance(&mut self, p: f64) -> bool {
        let unit = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        unit < p.clamp(0., 1.)
    }

    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i + 1);
            items.swap(i, j);
        }
    }

    /// `count` of the indices below `n`, in order (all of them when count ≥ n).
    pub fn choose(&mut self, n: usize, count: usize) -> Vec<usize> {
        let mut all: Vec<usize> = (0..n).collect();
        self.shuffle(&mut all);
        all.truncate(count.min(n));
        all.sort_unstable();
        all
    }
}

/// The seed of take `number`'s draw `try` from a set's seed: each take and each re-roll draws differently.
pub fn seed_of(set_seed: u64, number: usize, attempt: u32) -> u64 {
    let mut rng = Rng::new(set_seed ^ ((number as u64) << 32) ^ attempt as u64);
    rng.next_u64()
}

/// What a take is made of, as a DAW's variations see it.
pub trait Material {
    /// How far apart two are: 0 the same, 1 nothing in common.
    fn distance(&self, other: &Self) -> f64;
    /// Nothing in it would sound.
    fn silent(&self) -> bool;
}

/// How far a take may be from its source and from the other takes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    /// At least this far from the source (not a near-copy)…
    pub min_source: f64,
    /// …and at most this far (still the same idea).
    pub max_source: f64,
    /// At least this far from every other take.
    pub min_others: f64,
}

impl Default for Bounds {
    fn default() -> Self {
        Bounds { min_source: 0.08, max_source: 0.8, min_others: 0.05 }
    }
}

/// Why a take was or wasn't good enough.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Fine,
    Silent,
    /// A near-copy of the source (its distance).
    NearSource(f64),
    /// Too far from the source to be the same idea.
    FarFromSource(f64),
    /// A near-copy of another take (its number, the distance).
    NearTake(usize, f64),
    /// The take couldn't be made at all.
    Failed(String),
}

impl Verdict {
    pub fn is_fine(&self) -> bool {
        matches!(self, Verdict::Fine)
    }

    /// The reason in a few words, for the take's line.
    pub fn words(&self) -> String {
        match self {
            Verdict::Fine => "fine".into(),
            Verdict::Silent => "it has nothing in it that sounds".into(),
            Verdict::NearSource(d) => format!("a near-copy of the source ({d:.2} from it)"),
            Verdict::FarFromSource(d) => format!("too far from the source to be the same idea ({d:.2} from it)"),
            Verdict::NearTake(n, d) => format!("a near-copy of V{n} ({d:.2} from it)"),
            Verdict::Failed(why) => why.clone(),
        }
    }
}

/// Checks a take against its source (when it has one) and the other takes (their numbers and material).
pub fn check<M: Material>(take: &M, source: Option<&M>, others: &[(usize, &M)], bounds: &Bounds) -> Verdict {
    if take.silent() {
        return Verdict::Silent;
    }
    if let Some(source) = source {
        let d = take.distance(source);
        if d < bounds.min_source {
            return Verdict::NearSource(d);
        }
        if d > bounds.max_source {
            return Verdict::FarFromSource(d);
        }
    }
    for (number, other) in others {
        let d = take.distance(other);
        if d < bounds.min_others {
            return Verdict::NearTake(*number, d);
        }
    }
    Verdict::Fine
}

/// What a draw came to: the take (None when every try failed to make one), the seed that made it, how many tries it took,
/// and the last verdict (Fine, or why the last try didn't pass).
#[derive(Debug, Clone)]
pub struct Rolled<M> {
    pub take: Option<M>,
    pub seed: u64,
    pub tries: u32,
    pub verdict: Verdict,
}

/// Draws a take up to `tries` times (each from its own seed, told why the try before failed, so it can draw bolder or milder)
/// until one passes `check`. Answers the first that passes, else the last made with why it failed.
pub fn roll<M: Material>(
    set_seed: u64,
    number: usize,
    tries: u32,
    source: Option<&M>,
    others: &[(usize, &M)],
    bounds: &Bounds,
    mut draw: impl FnMut(u64, Option<&Verdict>) -> Result<M, String>,
) -> Rolled<M> {
    let mut last = Rolled { take: None, seed: 0, tries: 0, verdict: Verdict::Failed("nothing was drawn".into()) };
    for attempt in 0..tries.max(1) {
        let seed = seed_of(set_seed, number, attempt);
        let before = (attempt > 0).then(|| last.verdict.clone());
        last.tries = attempt + 1;
        match draw(seed, before.as_ref()) {
            Ok(take) => {
                let verdict = check(&take, source, others, bounds);
                let fine = verdict.is_fine();
                last = Rolled { take: Some(take), seed, tries: attempt + 1, verdict };
                if fine {
                    return last;
                }
            }
            Err(why) => last = Rolled { take: last.take.take(), seed, tries: attempt + 1, verdict: Verdict::Failed(why) },
        }
    }
    last
}

/// Where a take stands in the loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakeState {
    /// Made and shown, not yet kept: "the rest" are these.
    Shown,
    /// The producer keeps it: no change or drop touches it unless named with force.
    Kept,
    /// Dropped: no longer in the project.
    Dropped,
}

impl TakeState {
    pub fn word(self) -> &'static str {
        match self {
            TakeState::Shown => "shown",
            TakeState::Kept => "kept",
            TakeState::Dropped => "dropped",
        }
    }
}

/// One take: its number (V3 is 3), where it stands, how the DAW finds it again (`place`), its latest change in HISTORY,
/// the direction it went, what it held when Kumi last wrote it (`digest`), and its line for the answer.
#[derive(Debug, Clone)]
pub struct Take<P, M> {
    pub number: usize,
    pub state: TakeState,
    pub place: P,
    pub change: Option<String>,
    pub direction: String,
    pub digest: String,
    pub material: M,
    pub line: String,
}

/// What a request does to the takes it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Change,
    Keep,
    Drop,
}

/// The takes of one source.
#[derive(Debug, Clone)]
pub struct TakeSet<S, P, M> {
    pub id: usize,
    pub source: S,
    pub seed: u64,
    pub takes: Vec<Take<P, M>>,
    /// Numbers up to this were given before (another set of the same source's takes): the next take is past it.
    pub floor: usize,
}

impl<S, P, M> TakeSet<S, P, M> {
    pub fn new(id: usize, source: S, seed: u64) -> Self {
        TakeSet { id, source, seed, takes: Vec::new(), floor: 0 }
    }

    /// The number the next take gets.
    pub fn next_number(&self) -> usize {
        self.takes.iter().map(|take| take.number).max().unwrap_or(0).max(self.floor) + 1
    }

    pub fn take(&self, number: usize) -> Option<&Take<P, M>> {
        self.takes.iter().find(|take| take.number == number)
    }

    pub fn take_mut(&mut self, number: usize) -> Option<&mut Take<P, M>> {
        self.takes.iter_mut().find(|take| take.number == number)
    }

    /// The takes still in the project (shown or kept), but `except`: what a new take is measured against.
    pub fn living_except(&self, except: &[usize]) -> Vec<&Take<P, M>> {
        self.takes.iter().filter(|take| take.state != TakeState::Dropped && !except.contains(&take.number)).collect()
    }

    /// The take numbers a request names, in order: "V3" or "3", "rest" (every shown take) or "all" (shown and kept). A
    /// dropped take, or one that doesn't exist, is refused; so is a kept take for a change or a drop, unless `force` (the
    /// producer named it).
    pub fn pick(&self, names: &[String], purpose: Purpose, force: bool) -> Result<Vec<usize>, String> {
        let mut picked: Vec<usize> = Vec::new();
        let mut seen = BTreeSet::new();
        for name in names {
            let name = name.trim();
            let numbers: Vec<usize> = match name.to_ascii_lowercase().as_str() {
                "rest" | "the rest" | "others" => {
                    self.takes.iter().filter(|take| take.state == TakeState::Shown).map(|take| take.number).collect()
                }
                "all" => self.takes.iter().filter(|take| take.state != TakeState::Dropped).map(|take| take.number).collect(),
                other => {
                    let digits = other.trim_start_matches('v');
                    let number: usize = digits.parse().map_err(|_| format!("{name} isn't a take: name takes as V1, V2… or \"rest\""))?;
                    vec![number]
                }
            };
            for number in numbers {
                let Some(take) = self.take(number) else {
                    return Err(format!("there is no V{number} in this set (it has V1 to V{})", self.next_number() - 1));
                };
                match (take.state, purpose) {
                    (TakeState::Dropped, _) => return Err(format!("V{number} was dropped")),
                    (TakeState::Kept, Purpose::Change | Purpose::Drop) if !force => {
                        return Err(format!(
                            "V{number} is kept, so Kumi leaves it as it is; name it with force: true only if the producer asked for that take by name"
                        ))
                    }
                    _ => {}
                }
                if seen.insert(number) {
                    picked.push(number);
                }
            }
        }
        if picked.is_empty() {
            return Err("that names no take (every take is kept or dropped)".into());
        }
        Ok(picked)
    }

    /// One line per take still in the project, in order.
    pub fn lines(&self) -> Vec<String> {
        self.takes
            .iter()
            .filter(|take| take.state != TakeState::Dropped)
            .map(|take| match take.state {
                TakeState::Kept => format!("{} (kept)", take.line),
                _ => take.line.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct Dots(Vec<u32>);

    impl Material for Dots {
        fn distance(&self, other: &Self) -> f64 {
            let a: BTreeSet<_> = self.0.iter().collect();
            let b: BTreeSet<_> = other.0.iter().collect();
            let union = a.union(&b).count();
            if union == 0 {
                return 0.;
            }
            1. - a.intersection(&b).count() as f64 / union as f64
        }
        fn silent(&self) -> bool {
            self.0.is_empty()
        }
    }

    #[test]
    fn a_seed_makes_the_same_draws_and_another_seed_others() {
        let draws = |seed| {
            let mut rng = Rng::new(seed);
            (0..8).map(|_| rng.below(100)).collect::<Vec<_>>()
        };
        assert_eq!(draws(7), draws(7));
        assert_ne!(draws(7), draws(8));
        assert_ne!(seed_of(1, 2, 0), seed_of(1, 2, 1), "each re-roll draws anew");
        assert_ne!(seed_of(1, 2, 0), seed_of(1, 3, 0), "each take draws anew");
        let mut rng = Rng::new(3);
        let chosen = rng.choose(10, 4);
        assert_eq!(chosen.len(), 4);
        assert!(chosen.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn a_take_must_differ_from_its_source_and_the_others_yet_stay_its_idea() {
        let bounds = Bounds::default();
        let source = Dots(vec![0, 4, 8, 12]);
        assert_eq!(check(&Dots(vec![]), Some(&source), &[], &bounds), Verdict::Silent);
        assert!(matches!(check(&source.clone(), Some(&source), &[], &bounds), Verdict::NearSource(_)));
        assert!(matches!(check(&Dots(vec![1, 3, 5, 7, 9]), Some(&source), &[], &bounds), Verdict::FarFromSource(_)));
        let v1 = Dots(vec![0, 4, 8, 14]);
        assert_eq!(check(&v1, Some(&source), &[], &bounds), Verdict::Fine);
        assert!(matches!(check(&v1.clone(), Some(&source), &[(1, &v1)], &bounds), Verdict::NearTake(1, _)));
        assert_eq!(check(&v1, None, &[], &bounds), Verdict::Fine, "takes of an instrument have no source to be near");
    }

    #[test]
    fn a_draw_re_rolls_until_a_take_passes_and_says_why_when_none_does() {
        let source = Dots(vec![0, 4, 8, 12]);
        let mut calls = 0;
        let mut told = Vec::new();
        let rolled = roll(42, 1, 8, Some(&source), &[], &Bounds::default(), |_, before| {
            told.push(before.cloned());
            calls += 1;
            Ok(if calls < 3 { source.clone() } else { Dots(vec![0, 4, 8, 13]) })
        });
        assert_eq!(rolled.tries, 3);
        assert!(rolled.verdict.is_fine());
        assert_eq!(rolled.take, Some(Dots(vec![0, 4, 8, 13])));
        assert_eq!(told[0], None, "the first try is told nothing");
        assert!(matches!(told[1], Some(Verdict::NearSource(_))), "the next is told why the one before failed: {told:?}");
        let never = roll(42, 1, 4, Some(&source), &[], &Bounds::default(), |_, _| Ok(source.clone()));
        assert_eq!(never.tries, 4);
        assert!(matches!(never.verdict, Verdict::NearSource(_)), "{:?}", never.verdict);
        assert!(never.verdict.words().contains("near-copy of the source"));
    }

    fn set() -> TakeSet<(), (), Dots> {
        let mut set = TakeSet::new(1, (), 9);
        for number in 1..=9 {
            set.takes.push(Take {
                number,
                state: TakeState::Shown,
                place: (),
                change: None,
                direction: String::new(),
                digest: String::new(),
                material: Dots(vec![number as u32]),
                line: format!("V{number}"),
            });
        }
        set
    }

    #[test]
    fn keep_five_change_the_rest_names_exactly_the_four_left() {
        let mut set = set();
        let kept = set.pick(&["V1".into(), "3".into(), "v5".into(), "V7".into(), "9".into()], Purpose::Keep, false).unwrap();
        assert_eq!(kept, vec![1, 3, 5, 7, 9]);
        for number in kept {
            set.take_mut(number).unwrap().state = TakeState::Kept;
        }
        assert_eq!(set.pick(&["rest".into()], Purpose::Change, false).unwrap(), vec![2, 4, 6, 8]);
        let refused = set.pick(&["V3".into()], Purpose::Change, false).unwrap_err();
        assert!(refused.contains("V3 is kept"), "{refused}");
        assert_eq!(set.pick(&["V3".into()], Purpose::Change, true).unwrap(), vec![3], "named by the producer");
        set.take_mut(2).unwrap().state = TakeState::Dropped;
        assert!(set.pick(&["V2".into()], Purpose::Change, false).unwrap_err().contains("dropped"));
        assert!(set.pick(&["V12".into()], Purpose::Keep, false).unwrap_err().contains("no V12"));
        assert_eq!(set.pick(&["all".into()], Purpose::Keep, false).unwrap().len(), 8);
        assert_eq!(set.next_number(), 10);
        set.floor = 12;
        assert_eq!(set.next_number(), 13, "numbers never come back");
        assert_eq!(set.living_except(&[4]).len(), 7);
        assert!(set.lines().contains(&"V3 (kept)".to_string()));
    }

    #[test]
    fn takes_are_named_after_their_source_and_a_kept_mark_always_fits() {
        assert_eq!(take_name(3, "Amen chop"), "V3 \u{b7} Amen chop");
        assert_eq!(kept_name(3, "Amen chop"), "V3 \u{b7} Amen chop \u{b7} kept");
        // (a kept take is looked for by its kept name first)
        let [first, second] = super::tool::names_to_find(3, "Amen chop", TakeState::Kept);
        assert_eq!((first.as_str(), second.as_str()), ("V3 \u{b7} Amen chop \u{b7} kept", "V3 \u{b7} Amen chop"));
        assert_eq!(super::tool::names_to_find(3, "Amen chop", TakeState::Shown)[0], "V3 \u{b7} Amen chop");
        let long = "x".repeat(80);
        assert_eq!(take_name(12, &long).chars().count(), NAME_MOST);
        let kept = kept_name(12, &long);
        assert_eq!(kept.chars().count(), NAME_MOST);
        assert!(kept.ends_with(" \u{b7} kept"));
    }
}
