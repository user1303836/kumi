//! Assets from the timeline's items: items stacked or close together are one sound (three layers of a footstep make one asset),
//! each asset a stretch of the timeline and the tracks it plays on.
use std::collections::BTreeSet;

/// An item as the DAW reads it: its ref, its track's ref and name, where it plays (seconds), its name.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub reference: String,
    pub track: String,
    pub track_name: String,
    pub start: f64,
    pub end: f64,
    pub name: String,
}

/// One sound: its stretch (seconds) and its items (by index) and tracks.
#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    pub start: f64,
    pub end: f64,
    pub items: Vec<usize>,
    pub tracks: BTreeSet<String>,
}

/// The assets of a set of items: items that overlap, or start within `gap` seconds of the last one's end, are one asset; in
/// time order.
pub fn assets(items: &[Item], gap: f64) -> Vec<Asset> {
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|a, b| items[*a].start.total_cmp(&items[*b].start).then(items[*a].end.total_cmp(&items[*b].end)));
    let mut out: Vec<Asset> = Vec::new();
    for k in order {
        let item = &items[k];
        match out.last_mut() {
            Some(asset) if item.start <= asset.end + gap => {
                asset.end = asset.end.max(item.end);
                asset.items.push(k);
                asset.tracks.insert(item.track.clone());
            }
            _ => out.push(Asset { start: item.start, end: item.end, items: vec![k], tracks: BTreeSet::from([item.track.clone()]) }),
        }
    }
    out
}

/// The item that alone makes an asset of several sounds (a bed or a guide running under them): its index and how many assets
/// the others would make without it, when that's two or more.
pub fn bridging(items: &[Item], asset: &Asset, gap: f64) -> Option<(usize, usize)> {
    if asset.items.len() < 3 {
        return None;
    }
    let length = |k: &usize| items[*k].end - items[*k].start;
    let longest = *asset.items.iter().max_by(|a, b| length(a).total_cmp(&length(b)))?;
    let rest: Vec<Item> = asset.items.iter().filter(|&&k| k != longest).map(|&k| items[k].clone()).collect();
    let apart = assets(&rest, gap).len();
    (apart >= 2).then_some((longest, apart))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(track: &str, start: f64, end: f64) -> Item {
        Item { reference: format!("{track}@{start}"), track: track.into(), track_name: track.into(), start, end, name: String::new() }
    }

    #[test]
    fn a_bed_under_the_sounds_is_the_item_that_makes_them_one() {
        let items = vec![item("bed", 0.0, 30.0), item("heel", 1.0, 1.4), item("heel", 4.0, 4.5), item("heel", 9.0, 9.2)];
        let found = assets(&items, 0.25);
        assert_eq!(found.len(), 1, "the bed joins them");
        assert_eq!(bridging(&items, &found[0], 0.25), Some((0, 3)));
        // Layers stacked on one sound join nothing.
        let layers = vec![item("heel", 1.0, 1.4), item("toe", 1.05, 1.3), item("gravel", 1.38, 1.9)];
        assert_eq!(bridging(&layers, &assets(&layers, 0.25)[0], 0.25), None);
    }

    #[test]
    fn stacked_layers_and_close_items_are_one_asset_and_a_gap_starts_the_next() {
        let items = vec![
            // a footstep: three layers on three tracks, the last starting a little late
            item("heel", 1.0, 1.4),
            item("toe", 1.05, 1.3),
            item("gravel", 1.38, 1.9),
            // the next, two seconds on
            item("heel", 4.0, 4.5),
            item("gravel", 4.6, 5.0),
            // alone, far after
            item("heel", 9.0, 9.2),
        ];
        let found = assets(&items, 0.25);
        assert_eq!(found.len(), 3);
        assert_eq!((found[0].start, found[0].end, found[0].items.len(), found[0].tracks.len()), (1.0, 1.9, 3, 3));
        assert_eq!((found[1].start, found[1].end), (4.0, 5.0), "0.1 s apart is one sound");
        assert_eq!(found[2].items, vec![5]);
        // A tighter gap splits the second.
        assert_eq!(assets(&items, 0.05).len(), 4);
        assert!(assets(&[], 0.25).is_empty());
    }
}
