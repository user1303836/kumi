//! A fixture's cases as several tests, so nextest runs them side by side. One long test holds up each test that must
//! run alone (`.config/nextest.toml`) until it ends; a chunk ends in seconds. This changes only how tests are split:
//! every case is still checked, by exactly one chunk, and a failure still names its case.

/// `chunked!(check; part_00 = 0, part_01 = 1, …)` makes a test of each name, calling `check(k, n)`: check the cases
/// at index `k`, `k + n`, `k + 2n`, …, `n` being the number of names. `chunks_cover_every_case` makes sure the `k`s
/// are numbered 0, 1, 2, … in order, so no case is left out.
macro_rules! chunked {
    ($check:path; $($name:ident = $k:literal),+ $(,)?) => {
        const CHUNKS: &[usize] = &[$($k),+];
        $(
            #[test]
            fn $name() {
                $check($k, CHUNKS.len());
            }
        )+
        #[test]
        fn chunks_cover_every_case() {
            assert!(CHUNKS.iter().copied().eq(0..CHUNKS.len()), "chunks must be numbered 0, 1, 2, … in order: {CHUNKS:?}");
        }
    };
}
pub(crate) use chunked;
