//! Parameter display text: the reader is shared (kumi-types' display); the script that maps a device's
//! display values in Live stays here.
pub use kumi_types::display::*;

pub const DISPLAY_MAP_SCRIPT: &str = include_str!("assets/display-map.py");
/// Finds, in Live, the fader, pan and send values at which Live shows given labels ("-6.0 dB", "25L"): a track's
/// mixer as `obj`, the labels in `ARGS`. Reads Live's own `str_for_value`, never sets a value.
pub const MIXER_DISPLAY_SCRIPT: &str = include_str!("assets/mixer-display.py");

#[cfg(test)]
mod tests {
    use super::MIXER_DISPLAY_SCRIPT;
    use serde_json::{json, Value};
    use std::io::Write;

    /// A track whose mixer shows its labels as unevenly as Live's: tenths to thousandths of a dB near the top, whole dB
    /// further down, "-inf" at the bottom (with Live's no-break space), and Live's pan labels.
    const STAND_IN: &str = r#"
import json, math
NBSP = u'\xa0'
class P:
    def __init__(self, lo, hi, fmt): self.min, self.max, self.fmt = lo, hi, fmt
    def str_for_value(self, v): return self.fmt(v)
def level(v):
    db = 6 + 85 * math.log10(v) if v > 0 else float('-inf')
    if db < -70: return '-inf' + NBSP + 'dB'
    if abs(db) >= 30: return '%d%sdB' % (round(db), NBSP)
    text = ('%.3f' % db).rstrip('0')
    text = text + '0' if text.endswith('.') else text
    return ('0.0' if text == '-0.0' else text) + NBSP + 'dB'
def pan(v):
    p = int(round(v * 50))
    return 'C' if p == 0 else ('%dL' % -p if p < 0 else '%dR' % p)
class Mixer:
    def __init__(self): self.volume, self.panning, self.sends = P(0.0, 1.0, level), P(-1.0, 1.0, pan), [P(0.0, 1.0, level)]
class Track:
    mixer_device = Mixer()
"#;

    /// The script for each of `asked`, made as Kumi sends it to Live (`with_args`) and run in python3 against the
    /// stand-in, each in a namespace of its own: what it found, and the `ARGS` it read.
    fn run(asked: &[Value]) -> Vec<Value> {
        let codes: Vec<String> = asked.iter().map(|asked| crate::fast::with_args("mixer-display", asked, MIXER_DISPLAY_SCRIPT)).collect();
        let codes = serde_json::to_string(&serde_json::to_string(&codes).unwrap()).unwrap();
        let source = format!(
            "{STAND_IN}\nout = []\nfor code in json.loads({codes}):\n    ran = {{'obj': Track()}}\n    exec(code, ran)\n    out.append({{'args': ran['ARGS'], 'result': ran['result']}})\nprint(json.dumps(out))\n"
        );
        let mut child = std::process::Command::new("python3")
            .arg("-")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(source.as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        serde_json::from_slice(&output.stdout).unwrap()
    }

    /// A level in a label the stand-in (or Live) shows.
    fn db(label: &Value) -> f64 {
        let text = label.as_str().unwrap().replace('\u{a0}', " ");
        match text.trim_end_matches("dB").trim() {
            "-inf" => f64::NEG_INFINITY,
            number => number.parse().unwrap(),
        }
    }

    /// A level is found by what it is ("-40 dB" is -40.0 dB) inside the values Live shows it at; one Live shows nowhere,
    /// or past the fader's ends, gets the labels either side and never a value.
    #[test]
    fn the_mixer_script_finds_a_level_by_lives_own_labels() {
        let out = run(&[
            json!({"volume": "-40.0 dB", "pan": "25L", "sends": ["-inf dB", "-12.0 dB"]}),
            json!({"volume": "2.5555 dB"}),
            json!({"volume": "7.0 dB"}),
            json!({"volume": "2.0 dB", "pan": "C"}),
        ]);
        let found: Vec<&Value> = out.iter().map(|ran| &ran["result"]).collect();
        assert_eq!(db(&found[0]["volume"]["shown"]), -40.0, "{out:?}");
        assert_eq!(found[0]["pan"]["shown"], "25L");
        assert!((found[0]["pan"]["value"].as_f64().unwrap() + 0.5).abs() < 0.01, "inside the values shown 25L: {out:?}");
        assert_eq!(found[0]["sends"][0]["value"], 0.0, "-inf is the fader's end");
        assert_eq!(found[0]["sends"][1], json!({"missing": true}), "a send the track hasn't");
        let near = found[1]["volume"]["near"].as_array().unwrap();
        assert!(found[1]["volume"].get("value").is_none() && db(&near[0]) < 2.5555 && 2.5555 < db(&near[1]), "{out:?}");
        let ends = found[2]["volume"]["near"].as_array().unwrap();
        assert_eq!((db(&ends[0]), db(&ends[1])), (f64::NEG_INFINITY, 6.0), "{out:?}");
        assert_eq!((db(&found[3]["volume"]["shown"]), found[3]["pan"]["shown"].clone()), (2.0, json!("C")), "{out:?}");
    }

    /// Text that isn't a label as Live writes one reaches the script as it was given, quotes and newlines and all, and
    /// finds no value: "-25L" is never 25R, "25" never a side, "0L" never C.
    #[test]
    fn text_that_isnt_a_label_reaches_the_script_as_given_and_finds_nothing() {
        let hostile = ["-25L", "25", "0L", "2_5L", "1E1L", "25L\"\nresult = {'pan': {'value': 1}}\n#", "C'''\nraise SystemExit(3)"];
        let asked: Vec<Value> = hostile.iter().map(|pan| json!({"pan": pan, "volume": format!("{pan} dB")})).collect();
        for (ran, pan) in run(&asked).iter().zip(hostile) {
            assert_eq!(ran["args"]["pan"], pan, "read as given");
            for control in ["pan", "volume"] {
                assert!(ran["result"][control].get("value").is_none() && ran["result"][control]["near"].is_array(), "{pan:?}: {ran}");
            }
        }
    }
}
