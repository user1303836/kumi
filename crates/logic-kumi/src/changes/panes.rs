//! What the Library, Smart Controls and patch kinds need around them: the track they act on selected (they show the
//! selected track's strip), and a pane of the control bar's shown (the Library, the Smart Controls), each put back as
//! it was after. A track is selected the way the producer does it, with a click on its header's name (which brings
//! Logic in front); a pane opens with its control bar button, as the readers open the Mixer and the List Editors.

use std::time::Duration;

use super::{nudge, tracks, Ctx, Done};
use crate::ax::Node;
use crate::refs::failed;

/// How long a click on a header gets before the selection is read back, and a pane before it is read.
const SELECT_SETTLE: Duration = Duration::from_millis(350);
const PANE_SETTLE: Duration = Duration::from_millis(400);

/// Runs `with` with the track at `place` selected: a click on its header when it isn't, the selection put back after
/// (the track selected before clicked again).
pub fn with_selected<T>(ctx: &mut Ctx, place: usize, with: impl FnOnce(&mut Ctx) -> Done<T>) -> Done<T> {
    let all = nudge::read(tracks::headers(ctx))?;
    let was = all.iter().find(|f| f.header.selected).map(|f| f.place);
    if was != Some(place) {
        super::front::ensure(ctx.ax)?;
        nudge::read(ctx.project.select_track(ctx.ax, place))?;
        super::pause(SELECT_SETTLE);
        let now = nudge::read(tracks::headers(ctx))?;
        if now.iter().find(|f| f.header.selected).map(|f| f.place) != Some(place) {
            return Err(failed(format!("Logic Pro didn't select track {place}; nothing changed")));
        }
    }
    let result = with(ctx);
    if let Some(was) = was.filter(|was| *was != place) {
        let _ = ctx.project.select_track(ctx.ax, was);
        super::pause(SELECT_SETTLE);
    }
    result
}

/// The pane a control bar button shows, by the button's label ("Library", "Smart Controls"): opened when closed, its
/// node (the window's top-level group of that name) passed to `with`, closed again after when Kumi opened it.
pub fn with_pane<T>(ctx: &mut Ctx, label: &str, with: impl FnOnce(&mut Ctx, &Node) -> Done<T>) -> Done<T> {
    nudge::read(tracks::ensure_layout(ctx))?;
    let opened = nudge::read(ctx.project.open_pane(ctx.ax, label))?;
    if opened {
        super::pause(PANE_SETTLE);
    }
    let result = (|| {
        let window = nudge::read(ctx.ax.tree(&ctx.project.root.clone(), "", 1, 60, Some(&["role", "desc"])))?;
        let strings = &ctx.project.strings;
        let pane = window
            .children
            .iter()
            .find(|c| c.role == "AXGroup" && strings.is(&c.desc, label))
            .cloned()
            .ok_or_else(|| failed(format!("Logic Pro's window doesn't show its {label}")))?;
        with(ctx, &pane)
    })();
    if opened {
        let _ = ctx.project.close_pane(ctx.ax, label);
    }
    ctx.project.forget();
    result
}
