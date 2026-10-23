//! The control bar: the tempo (the LCD's tempo slider, whole BPM, no undo step of Logic's), the playhead (the LCD's
//! bar and beat sliders) and the transport (its Play and Stop buttons). The playhead is also what a marker or an event
//! is made at, so the marker and note changes move it and put it back through here.

use serde_json::{json, Value};

use super::nudge::{self, Control};
use super::undo::Probe;
use super::{changed, Applied, Ctx, Done, Preview, Restore, Restored};
use crate::ax::paths;
use crate::ax::Node;
use crate::focus::{Look, Transport};
use crate::queue::Answer;
use crate::refs::{bad, failed};
use crate::rows::Position;

/// How long a transport button gets before its state is read back.
const TRANSPORT_SETTLE_MS: u64 = 150;
/// Logic's tempo slider runs 5 to 990 BPM in whole steps.
pub const TEMPO_MIN: f64 = 5.0;
pub const TEMPO_MAX: f64 = 990.0;

/// One look at the control bar: the transport as it stands, and the bar's node for its controls.
pub(crate) fn control_bar(ctx: &mut Ctx) -> Done<(Node, Transport)> {
    nudge::read(super::tracks::ensure_layout(ctx))?;
    let bar = nudge::read(ctx.project.control_bar(ctx.ax))?;
    let transport = Transport::of(&bar, &ctx.project.strings);
    Ok((bar, transport))
}

/// One focus refresh through the reader, so the epoch moves as the observers would move it.
pub(crate) fn look(ctx: &mut Ctx) -> Done<Look> {
    nudge::read(super::tracks::ensure_layout(ctx))?;
    let (look, _, _) = nudge::read(ctx.project.look(ctx.ax, None))?;
    Ok(look)
}

fn lcd_control(ctx: &Ctx, bar: &Node, find: impl Fn(&Node) -> Option<String>, what: &str) -> Done<Control> {
    let lcd = paths::lcd(bar, &ctx.project.strings).ok_or_else(|| failed(format!("Logic Pro's control bar doesn't show {what}")))?;
    let path = find(lcd).ok_or_else(|| failed(format!("Logic Pro's control bar doesn't show {what}")))?;
    Ok(Control::new(&ctx.project.root, &path))
}

// ----- the tempo -----

fn wanted_bpm(input: &Value) -> Done<f64> {
    let bpm = input.get("bpm").and_then(Value::as_f64).filter(|bpm| bpm.is_finite()).ok_or_else(|| bad("bpm must be a number"))?;
    if !(TEMPO_MIN..=TEMPO_MAX).contains(&bpm) {
        return Err(bad(format!("bpm must be from {TEMPO_MIN} to {TEMPO_MAX}")));
    }
    Ok(bpm.round())
}

fn tempo_state(bpm: f64) -> Value {
    json!({"bpm": bpm})
}

fn same_tempo(a: &Value, b: &Value) -> bool {
    (a["bpm"].as_f64().unwrap_or(f64::NAN) - b["bpm"].as_f64().unwrap_or(f64::NAN)).abs() < 0.01
}

pub fn tempo_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let wanted = wanted_bpm(input)?;
    let (_, transport) = control_bar(ctx)?;
    let now = tempo_state(transport.tempo);
    let mut display = json!({"title": format!("Tempo {wanted} BPM"), "before": now, "after": tempo_state(wanted)});
    if input["bpm"].as_f64() != Some(wanted) {
        display["note"] =
            format!("Logic Pro's tempo display takes whole BPM from outside: {} was rounded to {wanted}.", input["bpm"]).into();
    }
    if same_tempo(&now, &tempo_state(wanted)) {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: json!({"ref": null, "kind": "project"}), prior: now.clone(), fence: now, display })
}

/// Steps the LCD's tempo slider to `bpm` and reads the control bar back: the tempo shown.
fn write_tempo(ctx: &mut Ctx, bar: &Node, now: f64, bpm: f64) -> Done<f64> {
    let control = lcd_control(ctx, bar, |lcd| paths::tempo(lcd, &ctx.project.strings).map(|n| n.path.clone()), "its tempo")?;
    nudge::wrote(nudge::step_to(ctx.ax, &control, now.round() as i64, bpm as i64))?;
    let (_, after) = control_bar(ctx).map_err(|(_, why)| super::uncertain(why))?;
    Ok(after.tempo)
}

pub fn tempo_apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let wanted = wanted_bpm(input)?;
    let (bar, transport) = control_bar(ctx)?;
    let now = tempo_state(transport.tempo);
    if !same_tempo(&now, fence) {
        return Err(changed("the tempo changed since the preview"));
    }
    let after = if same_tempo(&now, &tempo_state(wanted)) { transport.tempo } else { write_tempo(ctx, &bar, transport.tempo, wanted)? };
    if (after - wanted).abs() >= 0.01 {
        return Err(nudge::kept("the tempo", wanted, after));
    }
    Ok(Applied {
        applied: tempo_state(after),
        created: None,
        target: None,
        material: None,
        display: json!({"title": format!("Tempo {after} BPM"), "before": now, "after": tempo_state(after)}),
        step: None,
        probe: Some(Probe::Tempo { prior: transport.tempo, applied: after }),
    })
}

pub fn tempo_restore(ctx: &mut Ctx, restore: &Restore) -> Done<Restored> {
    let prior = restore.prior["bpm"].as_f64().ok_or_else(|| bad("prior.bpm"))?;
    let (bar, transport) = control_bar(ctx)?;
    let now = tempo_state(transport.tempo);
    if !same_tempo(&now, restore.prior) {
        if !restore.discard && !same_tempo(&now, restore.applied) {
            return Err(changed("the tempo has been changed in Logic Pro since Kumi set it"));
        }
        let after = write_tempo(ctx, &bar, transport.tempo, prior)?;
        if (after - prior).abs() >= 0.01 {
            return Err(nudge::kept("the tempo", prior, after));
        }
    }
    Ok(Restored {
        restored: tempo_state(prior),
        display: json!({"title": format!("Tempo back to {prior} BPM"), "before": now, "after": tempo_state(prior)}),
        undone_in_logic: false,
    })
}

// ----- the playhead -----

/// Moves the playhead to a position through the LCD's bar and beat sliders: the position the control bar shows after
/// (its bar and beat; the LCD shows no finer). The sliders keep the division and tick the playhead had, and a stop
/// leaves those anywhere (Logic then plays from 2.1.3.162, and a split or a marker put at "bar 2" lands there), so a
/// playhead not known to be on a beat ([`Project::playhead_exact`]) goes to the project's start first, with the Stop
/// button that reads "Go to Beginning" while Logic is stopped away from it (at the start, where it reads "Stop", the
/// playhead is on the beat already).
///
/// [`Project::playhead_exact`]: crate::project::Project::playhead_exact
pub(crate) fn set_playhead(ctx: &mut Ctx, position: Position) -> Done<Transport> {
    let (mut bar, mut transport) = control_bar(ctx)?;
    let stopped = !transport.playing && !transport.recording;
    if stopped && !ctx.project.playhead_exact {
        if let Some(button) = paths::transport_button(&bar, &ctx.project.strings, "Go to Beginning") {
            nudge::wrote(nudge::press(ctx.ax, &Control::new(&ctx.project.root, &button.path), TRANSPORT_SETTLE_MS))?;
            (bar, transport) = control_bar(ctx).map_err(|(_, why)| super::uncertain(why))?;
        }
    }
    let after = place(ctx, &bar, &transport, position)?;
    if stopped {
        ctx.project.playhead_exact = true;
    }
    Ok(after)
}

/// The bar and beat sliders stepped from where the control bar shows the playhead.
fn place(ctx: &mut Ctx, bar: &Node, transport: &Transport, position: Position) -> Done<Transport> {
    let strings = &ctx.project.strings;
    let lcd = paths::lcd(bar, strings).ok_or_else(|| failed("Logic Pro's control bar doesn't show its LCD"))?;
    let group = paths::playhead(lcd, strings).ok_or_else(|| failed("Logic Pro's control bar doesn't show the playhead"))?;
    let sliders: Vec<&Node> = group.children.iter().filter(|c| c.role == "AXSlider").collect();
    let (Some(bar_slider), Some(beat_slider)) = (sliders.first(), sliders.get(1)) else {
        return Err(failed("Logic Pro's control bar shows the playhead without its bar and beat"));
    };
    let (bar_control, beat_control) =
        (Control::new(&ctx.project.root, &bar_slider.path), Control::new(&ctx.project.root, &beat_slider.path));
    if transport.bar != position.bar {
        nudge::wrote(nudge::step_to(ctx.ax, &bar_control, i64::from(transport.bar), i64::from(position.bar)))?;
    }
    // The beat after the bar: a bar change sets the beat to one.
    let (_, between) = control_bar(ctx).map_err(|(_, why)| super::uncertain(why))?;
    if between.beat != position.beat {
        nudge::wrote(nudge::step_to(ctx.ax, &beat_control, i64::from(between.beat), i64::from(position.beat)))?;
    }
    let (_, after) = control_bar(ctx).map_err(|(_, why)| super::uncertain(why))?;
    Ok(after)
}

/// A position in beats from the project's start: `positionBeats`, or `position` in seconds at the project's tempo
/// (a constant one; Logic's tempo map isn't followed).
fn beats_input(input: &Value, tempo: f64) -> Done<f64> {
    let number = |key: &str| -> Done<Option<f64>> {
        match input.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => value
                .as_f64()
                .filter(|n| n.is_finite() && (0.0..=1.0e6).contains(n))
                .map(Some)
                .ok_or_else(|| bad(format!("{key} must be a number from 0"))),
        }
    };
    match (number("positionBeats")?, number("position")?) {
        (Some(_), Some(_)) => Err(bad("pass positionBeats or position, not both")),
        (Some(beats), None) => Ok(beats),
        (None, Some(seconds)) => Ok(seconds * tempo.max(1.0) / 60.0),
        (None, None) => Err(bad("pass positionBeats (beats from the project's start) or position (seconds)")),
    }
}

fn position_input(input: &Value, beats_per_bar: u32, tempo: f64) -> Done<Position> {
    Ok(Position::from_beats(beats_input(input, tempo)?, beats_per_bar))
}

/// `cursor_set {positionBeats | position}`: the playhead to a bar and beat (no undo step).
pub fn cursor_set(ctx: &mut Ctx, params: &Value) -> Answer {
    let (_, transport) = control_bar(ctx)?;
    let position = position_input(params, transport.beats_per_bar(), transport.tempo)?;
    let after = set_playhead(ctx, position)?;
    if (after.bar, after.beat) != (position.bar, position.beat) {
        return Err(nudge::kept(
            "the playhead",
            format!("bar {} beat {}", position.bar, position.beat),
            format!("bar {} beat {}", after.bar, after.beat),
        ));
    }
    let beats = after.beats();
    let seconds = if after.tempo > 0.0 { beats * 60.0 / after.tempo } else { 0.0 };
    Ok(json!({
        "epoch": ctx.project.epoch.to_string(),
        "positionBeats": beats,
        "position": seconds,
        "playhead": {"bar": after.bar, "beat": after.beat},
        "playing": after.playing,
    }))
}

/// Where a position is, as a producer says it: `bar 17`, or `bar 17 beat 3`.
pub(crate) fn bar_text(position: &Position) -> String {
    let mut text = format!("bar {}", position.bar);
    if position.beat > 1 || position.division > 1 || position.tick > 1 {
        text.push_str(&format!(" beat {}", position.beat));
    }
    if position.division > 1 || position.tick > 1 {
        text.push_str(&format!(".{}.{}", position.division, position.tick));
    }
    text
}

/// Moves the playhead to a whole beat exactly whatever was known of it: taken as unsure, so it goes to the project's
/// start first ([`set_playhead`]). `busy` while Logic plays.
pub(crate) fn set_playhead_exactly(ctx: &mut Ctx, position: Position) -> Done<Transport> {
    let (_, transport) = control_bar(ctx)?;
    if transport.playing || transport.recording {
        return Err(super::busy("Logic Pro is playing, so its playhead can't be placed; nothing changed"));
    }
    ctx.project.playhead_exact = false;
    let after = set_playhead(ctx, position)?;
    if (after.bar, after.beat) != (position.bar, position.beat) {
        return Err(nudge::kept(
            "the playhead",
            format!("bar {} beat {}", position.bar, position.beat),
            format!("bar {} beat {}", after.bar, after.beat),
        ));
    }
    Ok(after)
}

/// The control bar's Metronome Click button turned on or off; whether it was on before. `failed` when the control bar
/// shows no such button.
pub(crate) fn set_metronome(ctx: &mut Ctx, on: bool) -> Done<bool> {
    let (bar, _) = control_bar(ctx)?;
    let button = paths::transport_button(&bar, &ctx.project.strings, METRONOME)
        .ok_or_else(|| failed("Logic Pro's control bar doesn't show its Metronome Click button"))?;
    let was = button.is_on();
    if was != on {
        nudge::wrote(nudge::press(ctx.ax, &Control::new(&ctx.project.root, &button.path), TRANSPORT_SETTLE_MS))?;
        let (bar, _) = control_bar(ctx).map_err(|(_, why)| super::uncertain(why))?;
        let now = paths::transport_button(&bar, &ctx.project.strings, METRONOME).is_some_and(Node::is_on);
        if now != on {
            return Err(nudge::kept("the metronome", if on { "on" } else { "off" }, if now { "on" } else { "off" }));
        }
    }
    Ok(was)
}

/// Whether the control bar's Metronome Click button is on (false when it shows none).
pub(crate) fn metronome_on(bar: &Node, strings: &crate::ax::strings::Strings) -> bool {
    paths::transport_button(bar, strings, METRONOME).is_some_and(Node::is_on)
}

/// Whether the control bar shows its Metronome Click button at all.
pub(crate) fn shows_metronome(bar: &Node, strings: &crate::ax::strings::Strings) -> bool {
    paths::transport_button(bar, strings, METRONOME).is_some()
}

const METRONOME: &str = "Metronome Click";

// ----- the transport -----

fn transport_button(ctx: &mut Ctx, label: &str) -> Done<(Control, Transport)> {
    let (bar, transport) = control_bar(ctx)?;
    let button = paths::transport_button(&bar, &ctx.project.strings, label)
        .ok_or_else(|| failed(format!("Logic Pro's control bar doesn't show its {label} button")))?;
    Ok((Control::new(&ctx.project.root, &button.path), transport))
}

/// `transport_play`: Logic plays from the playhead.
pub fn play(ctx: &mut Ctx) -> Answer {
    let (play, before) = transport_button(ctx, "Play")?;
    if !before.playing {
        nudge::wrote(nudge::press(ctx.ax, &play, TRANSPORT_SETTLE_MS))?;
    }
    ctx.project.playhead_exact = false;
    let (_, now) = control_bar(ctx).map_err(|(_, why)| super::uncertain(why))?;
    if !now.playing {
        return Err(super::uncertain("Logic Pro's Play button was pressed, but it doesn't show itself playing"));
    }
    Ok(json!({"started": !before.playing, "playing": now.playing}))
}

/// `transport_stop`: Logic stops playing and recording. (Standing at the start, the Stop button reads "Go to
/// Beginning": it is looked for only while Logic plays.)
pub fn stop(ctx: &mut Ctx) -> Answer {
    let (_, before) = control_bar(ctx)?;
    let was_running = before.playing || before.recording;
    if was_running {
        ctx.project.playhead_exact = false;
        let (stop, _) = transport_button(ctx, "Stop")?;
        nudge::wrote(nudge::press(ctx.ax, &stop, TRANSPORT_SETTLE_MS))?;
        let (_, now) = control_bar(ctx).map_err(|(_, why)| super::uncertain(why))?;
        if now.playing || now.recording {
            return Err(super::uncertain("Logic Pro's Stop button was pressed, but it still shows itself playing"));
        }
    }
    Ok(json!({"stopped": was_running, "wasRecording": before.recording}))
}

impl Transport {
    /// The playhead as a position.
    pub fn position(&self) -> Position {
        Position { bar: self.bar.max(1), beat: self.beat.max(1), ..Position::START }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tempos_and_positions_are_checked_and_worded() {
        assert_eq!(wanted_bpm(&json!({"bpm": 120.4})).unwrap(), 120.0);
        assert!(wanted_bpm(&json!({"bpm": 2})).is_err());
        assert!(wanted_bpm(&json!({"bpm": "fast"})).is_err());
        assert_eq!(position_input(&json!({"positionBeats": 64}), 4, 120.0).unwrap(), Position { bar: 17, ..Position::START });
        assert_eq!(position_input(&json!({"positionBeats": 6.5}), 4, 120.0).unwrap(), Position { bar: 2, beat: 3, division: 3, tick: 1 });
        assert_eq!(
            position_input(&json!({"position": 2.0}), 4, 120.0).unwrap(),
            Position { bar: 2, ..Position::START },
            "seconds at the tempo"
        );
        assert!(position_input(&json!({"position": 2.0, "positionBeats": 4}), 4, 120.0).is_err());
        assert!(position_input(&json!({}), 4, 120.0).is_err());
        assert_eq!(bar_text(&Position { bar: 17, ..Position::START }), "bar 17");
        assert_eq!(bar_text(&Position { bar: 2, beat: 3, division: 1, tick: 1 }), "bar 2 beat 3");
        assert_eq!(bar_text(&Position { bar: 2, beat: 3, division: 3, tick: 1 }), "bar 2 beat 3.3.1");
        let transport = Transport { bar: 9, beat: 2, time_signature: "4/4".into(), ..Transport::default() };
        assert_eq!(transport.position(), Position { bar: 9, beat: 2, division: 1, tick: 1 });
        assert!(same_tempo(&tempo_state(120.0), &json!({"bpm": 120})));
    }
}
