//! The Mackie Control driver: virtual MIDI ports named "Kumi Logic" through the helper's `midi.*` ops, on which Logic
//! installs a control surface by itself (it scans new ports and sends device queries; the bridge answers only as a
//! Mackie Control, device 0x14, never as the extender or the C4, so one device is installed). From then on Logic
//! streams its state to the surface, which the bridge reads: the transport LEDs (play, record, cycle), the timecode
//! display (the playhead's bars, beats, divisions and ticks, which listening follows while Logic plays), the select
//! LEDs (which of the bank's eight strips is selected) and the LCD (the eight strips' names, seven characters each).
//! The bridge presses the surface's buttons too (select, solo, the transport). The protocol as the lab measured it:
//! `docs/evidence/logic-lab-2026-10-18.md`. [`Surface`] is the pure state machine over bytes; [`Mcu`] drives it
//! through the helper.

use std::time::Duration;

use serde_json::{json, Value};

use crate::helper::{Helper, HelperError};

/// The port name Logic shows in its Control Surfaces Setup.
pub const PORT_NAME: &str = "Kumi Logic";
/// Mackie's manufacturer id and the Mackie Control's device id.
const MACKIE: [u8; 3] = [0x00, 0x00, 0x66];
pub const MACKIE_CONTROL: u8 = 0x14;
/// The serial the surface answers with: seven bytes under 0x80, any will do.
const SERIAL: [u8; 7] = [0x4B, 0x55, 0x4D, 0x49, 0x4C, 0x47, 0x43];
/// The strips a Mackie Control shows at once.
pub const STRIPS: usize = 8;
/// Characters per strip on the LCD.
pub const LCD_COLUMNS: usize = 7;
/// The LCD's width.
const LCD_WIDTH: usize = 56;

/// The surface's buttons as note numbers (the Mackie Control's map).
pub mod button {
    /// The strips' solo buttons (and LEDs), the first strip's here, one note a strip.
    pub const SOLO: u8 = 0x08;
    pub const SELECT: u8 = 0x18;
    /// The V-Pot assignment buttons: Pan shows the strips' names on the LCD, where select selects a track.
    pub const TRACK: u8 = 0x28;
    pub const PAN: u8 = 0x2A;
    pub const BANK_LEFT: u8 = 0x2E;
    pub const BANK_RIGHT: u8 = 0x2F;
    pub const CHANNEL_LEFT: u8 = 0x30;
    pub const CHANNEL_RIGHT: u8 = 0x31;
    pub const CYCLE: u8 = 0x56;
    pub const REWIND: u8 = 0x5B;
    pub const FAST_FORWARD: u8 = 0x5C;
    pub const STOP: u8 = 0x5D;
    pub const PLAY: u8 = 0x5E;
    pub const RECORD: u8 = 0x5F;
    /// The timecode display's mode: the button that switches it, and the two LEDs that say which it shows.
    pub const SMPTE_BEATS: u8 = 0x35;
    pub const SMPTE_LED: u8 = 0x71;
    pub const BEATS_LED: u8 = 0x72;
}

/// What the surface learned from Logic since the last look.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub installed: bool,
    pub playing: bool,
    pub recording: bool,
    pub cycle: bool,
    /// Which of the bank's strips is selected (0 to 7), if Logic lit one.
    pub selected: Option<usize>,
    /// The LCD's two lines of 56 characters.
    pub lcd: [String; 2],
    /// The assignment display's two characters ("PN" in the Pan view, controllers 0x4B and 0x4A) and the timecode
    /// display's ten digits after them (controllers 0x49 to 0x40): in Logic's Beats mode three for the bar, two each
    /// for the beat and the division, three for the tick ("  1 1 1  1"), as characters.
    pub timecode: [char; 12],
    /// The LEDs lit, by note number.
    pub leds: Vec<u8>,
}

impl State {
    /// The strips' names from the LCD's top line, seven characters each, trimmed.
    pub fn names(&self) -> Vec<String> {
        (0..STRIPS)
            .map(|strip| self.lcd[0].chars().skip(strip * LCD_COLUMNS).take(LCD_COLUMNS).collect::<String>().trim().to_string())
            .collect()
    }

    /// The timecode display's ten digits, without the assignment display's two characters before them.
    pub fn digits(&self) -> String {
        self.timecode[2..].iter().collect()
    }

    /// The playhead as the timecode display shows it in Logic's Beats mode: bar, beat, division and tick, each in its
    /// own run of digits. None while the display shows something else (SMPTE time, or digits on their way).
    pub fn position(&self) -> Option<[i64; 4]> {
        bbt(&self.digits())
    }

    /// The playhead's bar and beat from the timecode display.
    pub fn playhead(&self) -> Option<(u32, u32)> {
        let [bar, beat, _, _] = self.position()?;
        Some((u32::try_from(bar).ok()?, u32::try_from(beat).ok()?))
    }

    /// Whether the timecode display shows bars and beats (its BEATS LED lit), not SMPTE time.
    pub fn beats_mode(&self) -> bool {
        self.leds.contains(&button::BEATS_LED) && !self.leds.contains(&button::SMPTE_LED)
    }
}

/// Ten timecode digits in Logic's Beats mode read as bar (three), beat (two), division (two) and tick (three); a
/// field that isn't a number (blank, or SMPTE's) is None.
pub fn bbt(digits: &str) -> Option<[i64; 4]> {
    let chars: Vec<char> = digits.chars().collect();
    if chars.len() != 10 {
        return None;
    }
    let field = |from: usize, to: usize| chars[from..to].iter().collect::<String>().trim().parse::<i64>().ok();
    Some([field(0, 3)?, field(3, 5)?, field(5, 7)?, field(7, 10)?])
}

/// What the state machine wants done after some bytes: bytes to send back, and the events the change deserves.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reaction {
    pub send: Vec<Vec<u8>>,
    pub transport_changed: bool,
    pub selection_changed: bool,
    pub names_changed: bool,
    /// The LCD's whole top line was written (Logic names the strips so; a message it shows for a moment, such as
    /// "Solo" over the first strip's name when a solo button is pressed, writes a few characters only).
    pub names_written: bool,
}

/// The Mackie Control as a state machine over the bytes Logic sends.
#[derive(Debug, Clone, Default)]
pub struct Surface {
    pub state: State,
    /// A SysEx that arrived in pieces, so far.
    partial: Vec<u8>,
    /// When Logic's first query came and when its first LCD or LED write did: the install's timing.
    pub queried_at: Option<std::time::Instant>,
    pub installed_at: Option<std::time::Instant>,
}

impl Surface {
    pub fn new() -> Surface {
        let mut surface = Surface::default();
        surface.state.lcd = [" ".repeat(LCD_WIDTH), " ".repeat(LCD_WIDTH)];
        surface.state.timecode = [' '; 12];
        surface
    }

    /// Bytes from Logic (one packet; a long SysEx may come in several).
    pub fn receive(&mut self, bytes: &[u8]) -> Reaction {
        let mut reaction = Reaction::default();
        let mut bytes = bytes;
        if !self.partial.is_empty() {
            self.partial.extend_from_slice(bytes);
            if !self.partial.contains(&0xF7) {
                return reaction;
            }
            let whole = std::mem::take(&mut self.partial);
            return self.receive(&whole);
        }
        while !bytes.is_empty() {
            let status = bytes[0];
            let used = match status {
                0xF0 => match bytes.iter().position(|b| *b == 0xF7) {
                    Some(end) => {
                        self.sysex(&bytes[..=end], &mut reaction);
                        end + 1
                    }
                    None => {
                        self.partial = bytes.to_vec();
                        bytes.len()
                    }
                },
                0x90 | 0x80 if bytes.len() >= 3 => {
                    let on = status == 0x90 && bytes[2] != 0;
                    self.led(bytes[1], on, &mut reaction);
                    3
                }
                0xB0 if bytes.len() >= 3 => {
                    self.control(bytes[1], bytes[2]);
                    3
                }
                0xE0..=0xEF if bytes.len() >= 3 => 3,
                0xA0 | 0xD0 if bytes.len() >= 2 => 2,
                0xF8..=0xFF => 1,
                _ => 1,
            };
            bytes = &bytes[used..];
        }
        reaction
    }

    fn sysex(&mut self, bytes: &[u8], reaction: &mut Reaction) {
        // F0 00 00 66 <device> <command> … F7
        if bytes.len() < 7 || bytes[1..4] != MACKIE {
            return;
        }
        let (device, command, body) = (bytes[4], bytes[5], &bytes[6..bytes.len() - 1]);
        if device != MACKIE_CONTROL {
            // Logic Control, the extenders, the C4: not us.
            return;
        }
        match command {
            // Device query: the host connection query, with our serial and a challenge.
            0x00 => {
                self.queried_at.get_or_insert_with(std::time::Instant::now);
                reaction.send.push(sysex(&[0x01], &[&SERIAL, &[0x01, 0x02, 0x03, 0x04]]));
            }
            // Host connection reply: the confirmation, and the surface is installed.
            0x02 => {
                reaction.send.push(sysex(&[0x03], &[&SERIAL]));
                self.state.installed = true;
            }
            // Host connection error: say nothing, Logic will query again.
            0x04 => {}
            // Version request: the version reply.
            0x13 => reaction.send.push(sysex(&[0x14], &[b"V1.0 "])),
            // The LCD: an offset, then the characters.
            0x12 => {
                if let Some((&offset, text)) = body.split_first() {
                    self.state.installed = true;
                    self.installed_at.get_or_insert_with(std::time::Instant::now);
                    let before = self.state.names();
                    let mut chars: Vec<char> = self.state.lcd.concat().chars().collect();
                    chars.resize(LCD_WIDTH * 2, ' ');
                    for (i, byte) in text.iter().enumerate() {
                        let at = offset as usize + i;
                        if at < chars.len() {
                            chars[at] = char::from(*byte);
                        }
                    }
                    self.state.lcd = [chars[..LCD_WIDTH].iter().collect(), chars[LCD_WIDTH..].iter().collect()];
                    if self.state.names() != before {
                        reaction.names_changed = true;
                    }
                    if offset == 0 && text.len() >= LCD_WIDTH {
                        reaction.names_written = true;
                    }
                }
            }
            _ => {}
        }
    }

    fn led(&mut self, note: u8, on: bool, reaction: &mut Reaction) {
        self.state.installed = true;
        self.installed_at.get_or_insert_with(std::time::Instant::now);
        let lit = self.state.leds.contains(&note);
        if on && !lit {
            self.state.leds.push(note);
        } else if !on && lit {
            self.state.leds.retain(|n| *n != note);
        }
        match note {
            button::PLAY => {
                if self.state.playing != on {
                    self.state.playing = on;
                    reaction.transport_changed = true;
                }
            }
            button::RECORD => {
                if self.state.recording != on {
                    self.state.recording = on;
                    reaction.transport_changed = true;
                }
            }
            button::CYCLE => {
                if self.state.cycle != on {
                    self.state.cycle = on;
                    reaction.transport_changed = true;
                }
            }
            button::SELECT..=0x1F => {
                let strip = (note - button::SELECT) as usize;
                let selected = if on {
                    Some(strip)
                } else if self.state.selected == Some(strip) {
                    None
                } else {
                    self.state.selected
                };
                if selected != self.state.selected {
                    self.state.selected = selected;
                    reaction.selection_changed = true;
                }
            }
            _ => {}
        }
    }

    /// The timecode and assignment displays: controllers 0x40 to 0x4B, one a character, right to left. The value's
    /// low six bits are the character in the Mackie's seven-segment set (0x20 to 0x3F as in ASCII, so 0x31 is "1"
    /// and 0x20 a blank; 0x01 to 0x1F the letters, 0x40 on), bit 6 a dot after it, which isn't kept.
    fn control(&mut self, controller: u8, value: u8) {
        if (0x40..=0x4B).contains(&controller) {
            self.state.installed = true;
            let digit = 11 - (controller - 0x40) as usize;
            let c = value & 0x3F;
            self.state.timecode[digit] = if c < 0x20 { char::from(0x40 + c) } else { char::from(c) };
        }
    }
}

/// A SysEx to Logic as a Mackie Control: the command, then the body parts.
fn sysex(command: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    let mut out = vec![0xF0];
    out.extend_from_slice(&MACKIE);
    out.push(MACKIE_CONTROL);
    out.extend_from_slice(command);
    for part in parts {
        out.extend_from_slice(part);
    }
    out.push(0xF7);
    out
}

/// A button press as the surface sends it: note on at full velocity, then note off.
pub fn press(note: u8) -> Vec<Vec<u8>> {
    vec![vec![0x90, note, 0x7F], vec![0x90, note, 0x00]]
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

pub fn from_hex(text: &str) -> Vec<u8> {
    text.split_whitespace().filter_map(|pair| u8::from_str_radix(pair, 16).ok()).collect()
}

/// The driver: the ports through the helper, and the surface behind them.
pub struct Mcu {
    pub surface: Surface,
    pub open: bool,
    /// The strips' names Logic last wrote on the LCD's whole top line, so a track's bank place can be looked up by
    /// name (a message Logic shows over a name for a moment doesn't change them).
    pub names: Vec<String>,
    /// How many of Logic's packets were logged: the first few, for the lab.
    logged: u32,
}

impl Default for Mcu {
    fn default() -> Self {
        Mcu { surface: Surface::new(), open: false, names: vec![], logged: 0 }
    }
}

impl Mcu {
    /// Opens the ports; Logic scans them and sends its device queries, which `on_event` answers.
    pub fn open(&mut self, helper: &mut dyn Helper) -> Result<(), HelperError> {
        match helper.call("midi.open", json!({"name": PORT_NAME, "uid": PORT_NAME})) {
            Ok(_) => {
                self.open = true;
                Ok(())
            }
            Err(HelperError::Refused { code, .. }) if code == "midi-open" => {
                self.open = true;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    pub fn close(&mut self, helper: &mut dyn Helper) {
        if self.open {
            let _ = helper.call("midi.close", json!({}));
            self.open = false;
            self.surface = Surface::new();
        }
    }

    /// A helper event: bytes from Logic, answered as the surface says. What changed.
    pub fn on_event(&mut self, event: &Value, helper: &mut dyn Helper) -> Reaction {
        if event.get("event").and_then(Value::as_str) != Some("midi") {
            return Reaction::default();
        }
        let bytes = event.get("hex").and_then(Value::as_str).map(from_hex).unwrap_or_default();
        let reaction = self.surface.receive(&bytes);
        if self.logged < 12 {
            self.logged += 1;
            crate::log::line(&format!("Mackie: Logic Pro sent {}", hex(&bytes)));
        }
        for reply in &reaction.send {
            if self.logged <= 12 {
                crate::log::line(&format!("Mackie: answering {}", hex(reply)));
            }
            if let Err(error) = helper.call("midi.send", json!({"hex": hex(reply)})) {
                crate::log::line(&format!("Mackie: couldn't answer: {error}"));
            }
        }
        if reaction.names_written {
            self.names = self.surface.state.names();
        }
        reaction
    }

    /// Presses a button on the surface.
    pub fn press(&mut self, note: u8, helper: &mut dyn Helper) -> Result<(), HelperError> {
        for packet in press(note) {
            helper.call("midi.send", json!({"hex": hex(&packet)}))?;
        }
        Ok(())
    }

    /// Selects the strip at a place in the bank (from 1, up to the bank's eight): Logic selects the track. The surface
    /// is put in the Pan view first, where the LCD names the strips and select means the track (in the Track view
    /// the strips are the selected track's parameters, and select opens its plug-ins). The bank shows the Mixer's
    /// strips from the first; a track past the eighth isn't reached in M2.
    pub fn select_strip(&mut self, place: usize, helper: &mut dyn Helper) -> Result<bool, HelperError> {
        if place == 0 || place > STRIPS {
            return Ok(false);
        }
        self.press(button::PAN, helper)?;
        self.pump(helper, Duration::from_millis(400));
        self.press(button::SELECT + (place - 1) as u8, helper)?;
        Ok(true)
    }

    /// How long Logic took from its first device query to its first write to the surface, in milliseconds.
    pub fn install_ms(&self) -> Option<u64> {
        Some(self.surface.installed_at?.saturating_duration_since(self.surface.queried_at?).as_millis() as u64)
    }

    /// Selects the strip of the bank whose name is `name`, if the LCD shows it.
    pub fn select_named(&mut self, name: &str, helper: &mut dyn Helper) -> Result<bool, HelperError> {
        let short: String = name.chars().take(LCD_COLUMNS).collect();
        let Some(strip) = self.names.iter().position(|n| n == short.trim() || (!n.is_empty() && short.trim().starts_with(n.as_str())))
        else {
            return Ok(false);
        };
        self.press(button::SELECT + strip as u8, helper)?;
        Ok(true)
    }

    /// The bank's strip (from 0) the LCD names `name` on, when exactly one does: Logic writes a name's first seven
    /// characters there (a longer name it may shorten otherwise, and then it isn't found).
    pub fn strip_named(&self, name: &str) -> Option<usize> {
        let short: String = name.chars().take(LCD_COLUMNS).collect();
        let short = short.trim();
        if short.is_empty() {
            return None;
        }
        let mut found = self.names.iter().enumerate().filter(|(_, n)| n.as_str() == short);
        let strip = found.next()?.0;
        found.next().is_none().then_some(strip)
    }

    /// Whether the strip's solo LED is lit.
    pub fn soloed(&self, strip: usize) -> bool {
        strip < STRIPS && self.surface.state.leds.contains(&(button::SOLO + strip as u8))
    }

    /// Presses the strip's solo button: Logic solos or unsolos its track.
    pub fn press_solo(&mut self, strip: usize, helper: &mut dyn Helper) -> Result<bool, HelperError> {
        if strip >= STRIPS {
            return Ok(false);
        }
        self.press(button::SOLO + strip as u8, helper)?;
        Ok(true)
    }

    /// Waits for Logic's bytes for a while, answering them: used right after the ports open, for the install.
    pub fn pump(&mut self, helper: &mut dyn Helper, wait: Duration) -> Reaction {
        let mut all = Reaction::default();
        let until = std::time::Instant::now() + wait;
        while let Some(left) = until.checked_duration_since(std::time::Instant::now()) {
            let Some(event) = helper.event(left.min(Duration::from_millis(50))) else { continue };
            let reaction = self.on_event(&event, helper);
            all.transport_changed |= reaction.transport_changed;
            all.selection_changed |= reaction.selection_changed;
            all.names_changed |= reaction.names_changed;
            all.names_written |= reaction.names_written;
        }
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_surface_answers_only_the_mackie_controls_device_query_and_reads_logics_feedback() {
        let mut surface = Surface::new();
        // Logic queries every model it knows; only the Mackie Control's gets the host connection query.
        assert_eq!(surface.receive(&from_hex("F0 00 00 66 10 00 F7")).send, Vec::<Vec<u8>>::new());
        assert_eq!(surface.receive(&from_hex("F0 00 00 66 11 00 F7")).send.len(), 0);
        let reaction = surface.receive(&from_hex("F0 00 00 66 14 00 F7"));
        assert_eq!(reaction.send.len(), 1);
        assert_eq!(&reaction.send[0][..6], &[0xF0, 0x00, 0x00, 0x66, 0x14, 0x01]);
        assert_eq!(reaction.send[0].len(), 6 + 7 + 4 + 1);
        assert!(!surface.state.installed);
        // The host connection reply gets the confirmation.
        let reaction = surface.receive(&from_hex("F0 00 00 66 14 02 4B 55 4D 49 4C 47 43 01 02 03 04 F7"));
        assert_eq!(reaction.send, vec![sysex(&[0x03], &[&SERIAL])]);
        assert!(surface.state.installed);
        // The version request gets a version reply.
        let reaction = surface.receive(&from_hex("F0 00 00 66 14 13 00 F7"));
        assert_eq!(&reaction.send[0][..6], &[0xF0, 0x00, 0x00, 0x66, 0x14, 0x14]);
        // LEDs: play, cycle, select 2 (0x19).
        let reaction = surface.receive(&from_hex("90 5E 7F"));
        assert!(reaction.transport_changed && surface.state.playing);
        assert!(!surface.receive(&from_hex("90 5E 7F")).transport_changed, "the same LED again changes nothing");
        assert!(surface.receive(&from_hex("90 56 7F 90 19 7F")).selection_changed);
        assert_eq!((surface.state.cycle, surface.state.selected), (true, Some(1)));
        assert!(surface.receive(&from_hex("90 19 00")).selection_changed);
        assert_eq!(surface.state.selected, None);
        assert!(surface.receive(&from_hex("90 5E 00")).transport_changed);
        assert!(!surface.state.playing);
        // The LCD in pieces: the top line's names, seven characters a strip.
        let mut lcd = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x12, 0x00];
        lcd.extend_from_slice(b"Vocal  Keys   ");
        lcd.push(0xF7);
        let (first, second) = lcd.split_at(9);
        assert_eq!(surface.receive(first), Reaction::default());
        let reaction = surface.receive(second);
        assert!(reaction.names_changed);
        assert_eq!(&surface.state.names()[..3], ["Vocal", "Keys", ""]);
        // The timecode and assignment displays as Logic writes them (the lab's bytes): "PN" in the assignment
        // display, then bar 7 at its first beat, division and tick, digit by digit from the left, each its ASCII.
        for packet in [
            "B0 49 20",
            "B0 48 20",
            "B0 47 37",
            "B0 46 20",
            "B0 45 31",
            "B0 44 20",
            "B0 43 31",
            "B0 42 20 B0 41 20",
            "B0 40 31",
            "B0 4B 10",
            "B0 4A 0E",
        ] {
            surface.receive(&from_hex(packet));
        }
        assert_eq!(surface.state.timecode.iter().collect::<String>(), "PN  7 1 1  1");
        assert_eq!((surface.state.digits().as_str(), surface.state.position()), ("  7 1 1  1", Some([7, 1, 1, 1])));
        assert_eq!(surface.state.playhead(), Some((7, 1)));
        // Playing: the ticks' digits move on their own (bar 1, beat 1, division 1, tick 183), and a dot bit is dropped.
        for packet in ["B0 49 20", "B0 48 20", "B0 47 31", "B0 45 71", "B0 43 31", "B0 42 31", "B0 41 38", "B0 40 33"] {
            surface.receive(&from_hex(packet));
        }
        assert_eq!(surface.state.position(), Some([1, 1, 1, 183]));
        assert_eq!(bbt("  1 1 1   "), None, "a field on its way isn't a number");
        assert!(!surface.state.beats_mode(), "no BEATS LED yet");
        surface.receive(&from_hex("90 72 7F"));
        assert!(surface.state.beats_mode());
        assert_eq!(press(button::SELECT), vec![vec![0x90, 0x18, 0x7F], vec![0x90, 0x18, 0x00]]);
        assert_eq!(hex(&[0x90, 0x18, 0x7F]), "90 18 7F");
    }

    #[test]
    fn the_driver_opens_the_ports_answers_through_the_helper_and_selects_by_name() {
        let hands = crate::helper::MockHands::new();
        hands.reply("midi.open", Ok(json!({"opened": true, "source": 1, "destination": 2})));
        hands.reply("midi.send", Ok(json!({"sent": true})));
        hands.reply("midi.send", Ok(json!({"sent": true})));
        hands.reply("midi.send", Ok(json!({"sent": true})));
        hands.reply("midi.send", Ok(json!({"sent": true})));
        hands.reply("midi.send", Ok(json!({"sent": true})));
        hands.reply("midi.send", Ok(json!({"sent": true})));
        hands.reply("midi.close", Ok(json!({"closed": true})));
        let mut launcher = hands.clone();
        let mut helper = crate::helper::Launch::launch(&mut launcher).unwrap();
        let mut mcu = Mcu::default();
        mcu.open(helper.as_mut()).unwrap();
        assert!(mcu.open);
        let reaction = mcu.on_event(&json!({"event": "midi", "hex": "F0 00 00 66 14 00 F7"}), helper.as_mut());
        assert_eq!(reaction.send.len(), 1);
        let lcd = |offset: u8, text: &str| {
            let mut hex = format!("F0 00 00 66 14 12 {offset:02X}");
            for byte in text.bytes() {
                hex.push_str(&format!(" {byte:02X}"));
            }
            hex + " F7"
        };
        // Logic names the strips by writing the LCD's whole top line.
        let reaction = mcu.on_event(&json!({"event": "midi", "hex": lcd(0, &format!("{:56}", "Vocal  Keys"))}), helper.as_mut());
        assert!(reaction.names_changed && reaction.names_written);
        // A message over the first name for a moment ("Solo" when a solo button is pressed) leaves the names as they were.
        let reaction = mcu.on_event(&json!({"event": "midi", "hex": lcd(0, "Solo ")}), helper.as_mut());
        assert!(reaction.names_changed && !reaction.names_written);
        assert_eq!(&mcu.surface.state.names()[..2], ["Solo", "Keys"]);
        assert_eq!(&mcu.names[..2], ["Vocal", "Keys"]);
        assert!(mcu.select_named("Keys", helper.as_mut()).unwrap());
        assert!(!mcu.select_named("Drums", helper.as_mut()).unwrap());
        assert!(!mcu.select_strip(9, helper.as_mut()).unwrap());
        assert!(mcu.install_ms().is_some(), "the LCD write after the query dates the install");
        // Solo by the strip the LCD names: the second strip's solo button, its LED read from Logic's feedback.
        assert_eq!((mcu.strip_named("Keys"), mcu.strip_named("Drums"), mcu.strip_named("")), (Some(1), None, None));
        assert!(mcu.press_solo(1, helper.as_mut()).unwrap());
        assert!(!mcu.press_solo(8, helper.as_mut()).unwrap());
        assert!(!mcu.soloed(1));
        mcu.on_event(&json!({"event": "midi", "hex": "90 09 7F"}), helper.as_mut());
        assert!(mcu.soloed(1) && !mcu.soloed(0));
        mcu.close(helper.as_mut());
        let ops: Vec<String> = hands.calls().into_iter().map(|(op, _)| op).collect();
        assert_eq!(ops, ["midi.open", "midi.send", "midi.send", "midi.send", "midi.send", "midi.send", "midi.close"]);
        let sent: Vec<Value> =
            hands.calls().into_iter().filter(|(op, _)| op == "midi.send").map(|(_, params)| params["hex"].clone()).collect();
        assert_eq!(sent[1], "90 19 7F");
        assert_eq!(sent[2], "90 19 00");
        assert_eq!((sent[3].as_str(), sent[4].as_str()), (Some("90 09 7F"), Some("90 09 00")));
        assert!(!mcu.open);
    }
}
