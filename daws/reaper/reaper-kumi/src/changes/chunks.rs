//! An item's chunk (the RPP text REAPER writes for it), for the changes the API has no precise inverse for: a split, a
//! take deleted or rendered, a glue. Such a change keeps the item's chunk from before it and puts it back whole, but only
//! while the item is exactly as the change left it (its print, below): then nothing of the producer's is overwritten.
//!
//! The print leaves out what only the screen or a selection changes (the item's and the track's selection, the MIDI
//! editor's view and filter, which notes and envelope points are selected, open FX windows, track and envelope lane
//! heights), so looking at an item or a track, or clicking another, doesn't make it "changed".

use crate::reaper::{Item, Reaper};

/// The most of an item's state a change keeps to put back: it travels in a frame and sits in the host's journal.
pub const MOST_KEPT: usize = 24 << 20;

/// Lines that say only how the item or track is shown or selected, at any depth: the MIDI editor's view, FX windows, and
/// the heights of the track and its envelope lanes (`TRACKHEIGHT` moves when the producer resizes the track, 7.82 lab).
const VIEW_LINES: [&str; 13] = [
    "CFGEDITVIEW ",
    "CFGEDIT ",
    "EVTFILTER ",
    "KEYSNAP ",
    "TRACKSEL ",
    "VELLANE ",
    "SHOW ",
    "LASTSEL ",
    "DOCKED ",
    "FLOATPOS ",
    "WNDRECT ",
    "TRACKHEIGHT ",
    "LANEHEIGHT ",
];

/// The chunk without what only the screen or a selection changes: an item's selection and list number (in an item's chunk,
/// or in each item of a track's), the track's own selection, the MIDI editor's view, which events and envelope points are
/// selected, FX windows, lane heights.
pub fn normalized(chunk: &str) -> String {
    let mut out = String::with_capacity(chunk.len());
    // The blocks open around each line, by their first word: an item's own lines are those right inside `<ITEM`, a
    // track's those right inside `<TRACK`.
    let mut blocks: Vec<&str> = Vec::new();
    for line in chunk.lines() {
        let trimmed = line.trim_start();
        let opens = trimmed.starts_with('<');
        let closes = trimmed.trim_end() == ">";
        let in_item = blocks.last() == Some(&"<ITEM");
        let in_track = blocks.last() == Some(&"<TRACK");
        let item_level = in_item && (trimmed.starts_with("SEL ") || trimmed.starts_with("IID "));
        let track_level = in_track && trimmed.starts_with("SEL ");
        if item_level || track_level || VIEW_LINES.iter().any(|prefix| trimmed.starts_with(prefix)) || trimmed == "FLOAT" {
            continue;
        }
        // A track's lane-recording line naming no recording or comp lane: REAPER writes it once it has put the track back
        // from its undo history, and its last number once lanes have been picked to play (which lanes play is the
        // `LANESOLO` line); it says nothing else (lab, 7.82).
        if trimmed.trim_end().strip_prefix("LANEREC -1 -1 -1 ").is_some_and(|rest| rest.parse::<i64>().is_ok()) {
            continue;
        }
        // A selected MIDI event (`e`, `em`, `<x`) is the same event as an unselected one, and so is a selected envelope
        // point (`PT 0 0.14 0` is written `PT 0 0.14 0 0 1` once selected: time, value, shape, tension, selected; 7.82 lab).
        let line = if trimmed.starts_with("e ") || trimmed.starts_with("em ") {
            format!("E{}", &trimmed[1..])
        } else if trimmed.starts_with("<x ") {
            format!("<X{}", &trimmed[2..])
        } else if trimmed.starts_with("PT ") {
            unselected_point(trimmed)
        } else {
            trimmed.to_string()
        };
        out.push_str(line.trim_end());
        out.push('\n');
        if opens {
            blocks.push(trimmed.split_whitespace().next().unwrap_or("<"));
        } else if closes {
            blocks.pop();
        }
    }
    out
}

/// An envelope point's line with its selection taken out: the fifth number cleared, and the numbers after the shape that
/// are left at zero dropped, so a point that was never selected and one selected and let go read the same.
fn unselected_point(line: &str) -> String {
    let mut fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() > 5 {
        fields[5] = "0";
    }
    while fields.len() > 4 && fields.last().and_then(|field| field.parse::<f64>().ok()) == Some(0.0) {
        fields.pop();
    }
    fields.join(" ")
}

/// A short, stable word for what the chunk holds: two chunks that differ only in their view and selection share it.
pub fn print_of(chunk: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in normalized(chunk).bytes() {
        hash = (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The item's print, or None when REAPER won't give its chunk.
pub fn item_print(reaper: &Reaper, item: Item) -> Option<String> {
    reaper.item_chunk(item).map(|chunk| print_of(&chunk))
}

/// The chunk without its ids: the item's, its takes', their MIDI sources' and pools, their FX's. REAPER gives an item made
/// from it new ones, so it is a copy that shares nothing with the item it came from (a MIDI copy is not pooled with it).
pub fn without_ids(chunk: &str) -> String {
    chunk
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !["IGUID ", "GUID ", "POOLEDEVTS ", "FXID "].iter().any(|prefix| trimmed.starts_with(prefix))
        })
        .map(|line| format!("{line}\n"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ITEM: &str = "<ITEM\nPOSITION 8\nSEL 1\nIGUID {A}\nIID 2\nNAME \"vox\"\nGUID {T}\n<SOURCE MIDI\nHASDATA 1 960 QN\nPOOLEDEVTS {P}\ne 0 90 3c 64\nE 480 80 3c 00\n<x 100 0\n8H5/CQH3\n>\nCFGEDITVIEW 0 0 0\nGUID {S}\n>\n<TAKEFX\nSHOW 0\nLASTSEL 0\nDOCKED 0\nFLOATPOS 0 0 0 0\nFXID {F}\n>\n>\n";

    #[test]
    fn the_print_leaves_out_the_selection_and_the_view_and_nothing_else() {
        let looked_at = ITEM
            .replace("SEL 1", "SEL 0")
            .replace("IID 2", "IID 7")
            .replace("e 0 90", "E 0 90")
            .replace("<x 100", "<X 100")
            .replace("CFGEDITVIEW 0 0 0", "CFGEDITVIEW 12 4 0")
            .replace("SHOW 0", "SHOW 1")
            .replace("FLOATPOS 0 0 0 0", "FLOATPOS 10 10 300 200");
        assert_eq!(print_of(ITEM), print_of(&looked_at));
        for changed in
            [ITEM.replace("POSITION 8", "POSITION 9"), ITEM.replace("E 480 80", "E 481 80"), ITEM.replace("NAME \"vox\"", "NAME \"vox 2\"")]
        {
            assert_ne!(print_of(ITEM), print_of(&changed));
        }
        // The `SEL` of a nested block or of an active take is not the item's selection.
        assert_ne!(print_of(ITEM), print_of(&ITEM.replace("NAME \"vox\"", "TAKE SEL\nNAME \"vox\"")));
    }

    #[test]
    fn a_tracks_print_leaves_out_its_items_selection_too() {
        let track = format!("<TRACK {{X}}\nNAME \"Vox\"\nSEL 1\n{ITEM}>\n");
        let looked_at = track.replacen("SEL 1\nIGUID", "SEL 0\nIGUID", 1);
        assert_ne!(track, looked_at);
        assert_eq!(print_of(&track), print_of(&looked_at));
    }

    #[test]
    fn a_tracks_print_leaves_out_its_own_selection_its_heights_its_windows_and_which_points_are_selected() {
        let envelope = "<VOLENV2\nEGUID {E}\nACT 1 -1\nVIS 1 1 1\nLANEHEIGHT 0 0\nARM 1\nPT 0 0.14 0\nPT 2 0.5 5 0.25\n>\n";
        let fx = "<FXCHAIN\nWNDRECT 24 52 699 416\nSHOW 0\nLASTSEL 0\nDOCKED 0\n>\n";
        let track = format!("<TRACK {{X}}\nNAME \"Vox\"\nSEL 0\nTRACKHEIGHT 0 0 0 0 0 0 0\n{fx}{envelope}{ITEM}>\n");
        let clicked = track
            .replacen("SEL 0\nTRACKHEIGHT 0 0 0 0 0 0 0", "SEL 1\nTRACKHEIGHT 112 0 0 0 0 0 0", 1)
            .replace("WNDRECT 24 52 699 416", "WNDRECT 80 80 600 400")
            .replace("LANEHEIGHT 0 0", "LANEHEIGHT 90 0")
            .replace("PT 0 0.14 0\n", "PT 0 0.14 0 0 1\n")
            .replace("PT 2 0.5 5 0.25\n", "PT 2 0.5 5 0.25 1\n");
        assert_ne!(track, clicked);
        assert_eq!(print_of(&track), print_of(&clicked));
        // What the producer changes still shows: a point moved, its shape or tension, the envelope shown or armed.
        for changed in [
            track.replace("PT 0 0.14 0\n", "PT 0 0.15 0\n"),
            track.replace("PT 2 0.5 5 0.25\n", "PT 2 0.5 5 0.3\n"),
            track.replace("PT 0 0.14 0\n", "PT 0 0.14 2\n"),
            track.replace("VIS 1 1 1", "VIS 0 1 1"),
            track.replace("ARM 1", "ARM 0"),
        ] {
            assert_ne!(print_of(&track), print_of(&changed), "{changed}");
        }
    }

    #[test]
    fn a_tracks_lane_recording_line_naming_no_lane_is_the_same_as_none() {
        let track = format!("<TRACK {{X}}\nNAME \"Vox\"\n{ITEM}>\n");
        for line in ["LANEREC -1 -1 -1 0", "LANEREC -1 -1 -1 1"] {
            let put_back = track.replacen("NAME \"Vox\"\n", &format!("NAME \"Vox\"\n{line}\n"), 1);
            assert_eq!(print_of(&track), print_of(&put_back), "{line}");
        }
        // (one that names a lane is not)
        assert_ne!(print_of(&track), print_of(&track.replacen("NAME \"Vox\"\n", "NAME \"Vox\"\nLANEREC -1 0 -1 0\n", 1)));
    }

    #[test]
    fn a_copy_has_no_ids_and_keeps_everything_else() {
        let copy = without_ids(ITEM);
        for id in ["IGUID", "GUID {T}", "GUID {S}", "POOLEDEVTS", "FXID"] {
            assert!(!copy.contains(id), "{id}: {copy}");
        }
        assert!(copy.contains("POSITION 8\n") && copy.contains("E 480 80 3c 00\n") && copy.contains("8H5/CQH3\n"));
        assert_eq!(copy.lines().count(), ITEM.lines().count() - 5);
    }
}
