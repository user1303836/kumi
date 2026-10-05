//! The device check's own process: the spec in on stdin, the result out on stdout, nothing else. Kumi
//! starts it with no environment, a memory limit, no code generation from strings and a deadline, so a
//! device's code can't reach Kumi, the producer's files or keys, or hang Kumi (see check_midi_device_isolated).

use std::io::{Read, Write};
use std::sync::atomic::Ordering;

use kumi_common::js::json::stringify;
use kumi_common::js::string::head;
use serde::Deserialize;
use serde_json::json;

use super::harness::{check_midi_device, MEMORY_LIMIT};
use super::spec::{Control, MidiSpec, MidiTest};

/// What the check is given: the parts of the spec its code runs on.
#[derive(Deserialize)]
struct Input {
    controls: Vec<Control>,
    code: String,
    tests: Vec<MidiTest>,
    #[serde(rename = "runsFree", default)]
    runs_free: bool,
}

pub fn main() -> i32 {
    MEMORY_LIMIT.store(128 * 1024 * 1024, Ordering::Relaxed);
    let mut input: Vec<u8> = Vec::new();
    let read = std::io::stdin().read_to_end(&mut input).map_err(|error| error.to_string());
    let result = match read.and_then(|_| serde_json::from_slice::<Input>(&input).map_err(|error| error.to_string())) {
        Ok(given) => {
            let spec = MidiSpec {
                name: String::new(),
                about: String::new(),
                controls: given.controls,
                code: given.code,
                tests: given.tests,
                runs_free: given.runs_free,
            };
            serde_json::to_value(check_midi_device(&spec)).unwrap_or_else(|error| json!({ "passed": 0, "of": 0, "problems": [format!("Kumi's check couldn't run it: {}", head(&error.to_string(), 200))] }))
        }
        Err(error) => json!({ "passed": 0, "of": 0, "problems": [format!("Kumi's check couldn't run it: {}", head(&error, 200))] }),
    };
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(stringify(&result).as_bytes());
    let _ = stdout.flush();
    0
}
