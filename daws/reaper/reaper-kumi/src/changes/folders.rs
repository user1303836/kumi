//! Folders as REAPER keeps them. A track's `I_FOLDERDEPTH` says how many folders open (1, a folder parent) or close
//! (negative: the track is the last in that many) after it, and a track's folder is the innermost one open when it comes.
//! Nothing here asks REAPER: a delete and its undo are worked out on the depths, so that they can be checked apart from it.

/// Which folder each track is in, read from the depths.
pub struct Layout {
    /// The track that is each track's folder parent.
    pub parent: Vec<Option<usize>>,
    /// The folders still open after the last track, outermost first.
    pub open_at_end: Vec<usize>,
}

pub fn layout(depths: &[i64]) -> Layout {
    let mut open: Vec<usize> = Vec::new();
    let mut parent = Vec::with_capacity(depths.len());
    for (at, depth) in depths.iter().enumerate() {
        parent.push(open.last().copied());
        if *depth > 0 {
            open.push(at);
        }
        for _ in 0..(-*depth).max(0) {
            open.pop();
        }
    }
    Layout { parent, open_at_end: open }
}

/// The depths that make tracks have these parents, leaving `end` folders open after the last. `None` when the parents are
/// no folders: a track whose parent isn't open where it comes.
fn depths_for(parent: &[Option<usize>], end: usize) -> Option<Vec<i64>> {
    // The track before and the folders around it, innermost last: what the next track can be in.
    let mut chain: Vec<usize> = Vec::new();
    let mut level = Vec::with_capacity(parent.len());
    for (at, wanted) in parent.iter().enumerate() {
        match wanted {
            None => chain.clear(),
            Some(wanted) => {
                while chain.last().is_some_and(|top| top != wanted) {
                    chain.pop();
                }
                chain.last()?;
            }
        }
        level.push(chain.len() as i64);
        chain.push(at);
    }
    Some((0..level.len()).map(|at| level.get(at + 1).copied().unwrap_or(end as i64) - level[at]).collect())
}

/// The depths the other tracks should have once track `gone` is deleted: every one stays in the folder it was in, except
/// that the tracks of a folder that goes move up into the folder it was in. Each is its depth now plus what the change
/// of the folders takes (so a depth that isn't the usual one for its place stays unusual).
pub fn after_delete(depths: &[i64], gone: usize) -> Vec<i64> {
    let before = layout(depths);
    let canon_before = depths_for(&before.parent, before.open_at_end.len()).expect("a layout made of depths is made of folders");
    let place = |at: usize| if at < gone { at } else { at - 1 };
    let parent: Vec<Option<usize>> = (0..depths.len())
        .filter(|at| *at != gone)
        .map(|at| {
            let wanted = if before.parent[at] == Some(gone) { before.parent[gone] } else { before.parent[at] };
            wanted.map(place)
        })
        .collect();
    let end = before.open_at_end.len() - usize::from(before.open_at_end.contains(&gone));
    let canon_after = depths_for(&parent, end).expect("folders without one of them are folders");
    (0..depths.len()).filter(|at| *at != gone).map(|at| depths[at] + canon_after[place(at)] - canon_before[at]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_track_is_in_the_innermost_folder_open_when_it_comes() {
        // Outer holds Inner (a, b) and c; d is outside.
        let layout = layout(&[1, 1, 0, -1, -1, 0]);
        assert_eq!(layout.parent, [None, Some(0), Some(1), Some(1), Some(0), None]);
        assert!(layout.open_at_end.is_empty());
        // A folder never closed stays open after the last track.
        assert_eq!(super::layout(&[1, 0]).open_at_end, [0]);
        // Closing more than is open closes nothing more.
        assert_eq!(super::layout(&[0, -3, 0]).parent, [None, None, None]);
    }

    #[test]
    fn depths_are_made_from_parents_and_refuse_what_is_no_folder() {
        assert_eq!(depths_for(&[None, Some(0), Some(0), Some(1), None], 0), None, "track 3 can't be in a folder that closed");
        assert_eq!(depths_for(&[None, Some(0), Some(0), None], 0), Some(vec![1, 0, -1, 0]));
        assert_eq!(depths_for(&[None, Some(0), Some(1)], 0), Some(vec![1, 1, -2]));
        assert_eq!(depths_for(&[None, Some(0)], 1), Some(vec![1, 0]));
    }

    #[test]
    fn the_last_track_of_a_folder_going_hands_its_closing_to_the_one_before() {
        // Bus holds 10 and 11; 12 is outside it. 11 goes: 10 closes the Bus, and 12 stays out.
        assert_eq!(after_delete(&[1, 0, -1, 0], 2), [1, -1, 0]);
        // Alone in its folder, a track that goes leaves the folder empty, which is no folder.
        assert_eq!(after_delete(&[1, -1, 0], 1), [0, 0]);
    }

    #[test]
    fn a_folder_going_moves_its_tracks_up_and_never_closes_the_one_around_it_early() {
        // Outer holds Inner (x, y) and z. Inner closes with -1: z would be left out of Outer if y still closed one folder.
        assert_eq!(after_delete(&[1, 1, 0, -1, -1], 1), [1, 0, 0, -1]);
        // Closing two levels at once is one level now.
        assert_eq!(after_delete(&[1, 1, 0, -2, 0], 1), [1, 0, -1, 0]);
        // A folder at the top level that goes leaves its tracks at the top.
        assert_eq!(after_delete(&[1, 0, -1, 0], 0), [0, 0, 0]);
    }

    #[test]
    fn a_track_that_is_in_no_folder_leaves_everything_as_it_is() {
        assert_eq!(after_delete(&[1, 0, 0, -1, 0], 2), [1, 0, -1, 0]);
        assert_eq!(after_delete(&[0, 0, 0], 1), [0, 0]);
        // A project that ends inside a folder still does.
        assert_eq!(after_delete(&[1, 0, 0], 1), [1, 0]);
        assert_eq!(after_delete(&[1, 0, 0], 2), [1, 0]);
        assert_eq!(after_delete(&[1, 0], 1), [1]);
    }
}
