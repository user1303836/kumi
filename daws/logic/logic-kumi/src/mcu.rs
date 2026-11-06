//! The Mackie Control driver: virtual MIDI ports named "Kumi Logic" through the helper's `midi.*` ops, on which Logic
//! installs a control surface by itself (it scans new ports and sends device queries; the bridge answers only as a
//! Mackie Control, device 0x14, never as the extender or the C4, so one device is installed). From then on Logic
//! streams its state to the surface, which the bridge reads: the transport LEDs (play, record, cycle), the timecode
//! display (the playhead's bar and beat), the select LEDs (which of the bank's eight strips is selected) and the
//! LCD (the eight strips' names, seven characters each). The bridge presses the surface's buttons too (select, the
//! transport) when a reader needs Logic to select a track. The protocol as the lab measured it: `docs/evidence/
//! logic-lab-2026-10-18.md`. [`Surface`] is the pure state machine over bytes; [`Mcu`] drives it through the helper.

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
    /// The timecode display's twelve digits (bars, beats, divisions, ticks in Logic's "Beats" mode), as characters.
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

    /// The playhead's bar and beat from the timecode display (Logic shows "bars.beats.divisions.ticks" in it).
    pub fn playhead(&self) -> Option<(u32, u32)> {
        let text: String = self.timecode.iter().collect();
        let mut parts = text.split(|c: char| !c.is_ascii_digit()).filter(|part| !part.is_empty());
        let bar = parts.next()?.parse().ok()?;
        let beat = parts.next().and_then(|beat| beat.parse().ok()).unwrap_or(1);
        Some((bar, beat))
    }
}

/// What the state machine wants done after some bytes: bytes to send back, and the events the change deserves.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reaction {
    pub send: Vec<Vec<u8>>,
    pub transport_changed: bool,
    pub selection_changed: bool,
    pub names_changed: bool,
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

    /// The timecode display: controllers 0x40 to 0x4B, one a digit, right to left; the value's low bits are the
    /// character (0x30 + digit), bit 6 a dot after it.
    fn control(&mut self, controller: u8, value: u8) {
        if (0x40..=0x4B).contains(&controller) {
            self.state.installed = true;
            let digit = 11 - (controller - 0x40) as usize;
            let c = value & 0x3F;
            self.state.timecode[digit] = if c == 0 { ' ' } else { char::from(0x20 + c) };
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
    /// The strips' names Logic last wrote on the LCD, so a track's bank place can be looked up by name.
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
        if reaction.names_changed {
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
        // The timecode display: bar 9, beat 1, written right to left.
        for (controller, c) in [
            (0x4B, b'9' - 0x20),
            (0x4A, 0),
            (0x49, 0),
            (0x48, b'1' - 0x20),
            (0x47, 0),
            (0x46, b'1' - 0x20),
            (0x45, 0),
            (0x44, 0),
            (0x43, b'1' - 0x20),
            (0x42, 0),
            (0x41, 0),
            (0x40, 0),
        ] {
            surface.receive(&[0xB0, controller, c]);
        }
        assert_eq!(surface.state.playhead(), Some((9, 1)));
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
        hands.reply("midi.close", Ok(json!({"closed": true})));
        let mut launcher = hands.clone();
        let mut helper = crate::helper::Launch::launch(&mut launcher).unwrap();
        let mut mcu = Mcu::default();
        mcu.open(helper.as_mut()).unwrap();
        assert!(mcu.open);
        let reaction = mcu.on_event(&json!({"event": "midi", "hex": "F0 00 00 66 14 00 F7"}), helper.as_mut());
        assert_eq!(reaction.send.len(), 1);
        let mut lcd = String::from("F0 00 00 66 14 12 00");
        for byte in b"Vocal  Keys   " {
            lcd.push_str(&format!(" {byte:02X}"));
        }
        lcd.push_str(" F7");
        assert!(mcu.on_event(&json!({"event": "midi", "hex": lcd}), helper.as_mut()).names_changed);
        assert!(mcu.select_named("Keys", helper.as_mut()).unwrap());
        assert!(!mcu.select_named("Drums", helper.as_mut()).unwrap());
        assert!(!mcu.select_strip(9, helper.as_mut()).unwrap());
        assert!(mcu.install_ms().is_some(), "the LCD write after the query dates the install");
        mcu.close(helper.as_mut());
        let ops: Vec<String> = hands.calls().into_iter().map(|(op, _)| op).collect();
        assert_eq!(ops, ["midi.open", "midi.send", "midi.send", "midi.send", "midi.close"]);
        let sent: Vec<Value> =
            hands.calls().into_iter().filter(|(op, _)| op == "midi.send").map(|(_, params)| params["hex"].clone()).collect();
        assert_eq!(sent[1], "90 19 7F");
        assert_eq!(sent[2], "90 19 00");
        assert!(!mcu.open);
    }
}
