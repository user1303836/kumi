//! read_with_code: a script calls the tools that only read, as functions, and only what it returns comes back.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use async_trait::async_trait;
use kumi_agent::core::hidden::{named, HIDDEN};
use kumi_agent::kernel::code;
use kumi_common::abort::Signal;
use kumi_types::contracts::{JsonObject, KernelTool, Reach, ToolResult};
use kumi_types::daw::DawProfile;
use kumi_types::errors::RuntimeError;
use serde_json::{json, Value};

/// A tool that answers from its input, and keeps each input it ran with.
struct Fake {
    name: &'static str,
    reach: Reach,
    /// All it does is read.
    reads: bool,
    schema: Value,
    answer: fn(&JsonObject) -> ToolResult,
    ran: Rc<RefCell<Vec<Value>>>,
    /// Counts the times its refs were dropped, for a tool whose own failure drops them (as a read Live refuses does).
    drops: Option<Rc<Cell<u64>>>,
}

#[async_trait(?Send)]
impl KernelTool for Fake {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "A fake."
    }
    fn input_schema(&self) -> JsonObject {
        self.schema.as_object().unwrap().clone()
    }
    fn reach(&self) -> Reach {
        self.reach
    }
    fn only_reads(&self) -> bool {
        self.reads
    }
    fn dropped(&self) -> u64 {
        self.drops.as_ref().map_or(0, |drops| drops.get())
    }
    async fn execute(&self, input: JsonObject, _signal: Signal) -> Result<ToolResult, RuntimeError> {
        self.ran.borrow_mut().push(Value::Object(input.clone()));
        let result = (self.answer)(&input);
        if let Some(drops) = self.drops.as_ref().filter(|_| result.is_error) {
            drops.set(drops.get() + 1);
        }
        Ok(result)
    }
}

/// Device 3's name hides words: tag characters, which draw nothing.
fn hiding() -> String {
    format!("Compressor 3{}", "\u{E0068}\u{E0069}\u{E0064}\u{E0064}\u{E0065}\u{E006E}")
}

/// A project of 40 devices: `list_devices` pages through their refs ten at a time, the last page with no cursor (as
/// every DAW's is), and `read_device` gives one's settings (every seventh is set under −20 dB; there's no device past
/// 39). With them, a change tool, a tool that reads the DAW and writes a file, and a search that reads away from the
/// DAW without saying that's all it does (one that may render what it's to compare, or go online).
struct Project {
    tools: Vec<Rc<dyn KernelTool>>,
    read: Rc<RefCell<Vec<Value>>>,
    changed: Rc<RefCell<Vec<Value>>>,
}

fn project() -> Project {
    let ran = || Rc::new(RefCell::new(Vec::new()));
    let (listed, read, changed, searched) = (ran(), ran(), ran(), ran());
    let fake = |name, reach, schema, answer, ran: &Rc<RefCell<Vec<Value>>>| -> Rc<dyn KernelTool> {
        Rc::new(Fake { name, reach, reads: matches!(name, "list_devices" | "read_device"), schema, answer, ran: ran.clone(), drops: None })
    };
    let any = json!({"type":"object"});
    let tools = vec![
        fake(
            "list_devices",
            Reach::DawRead,
            json!({"type":"object","additionalProperties":false,"properties":{"cursor":{"type":"integer","minimum":0}}}),
            |input| {
                let from = input.get("cursor").and_then(Value::as_u64).unwrap_or(0);
                let items: Vec<String> = (from..from + 10).map(|n| format!("device:{n}")).collect();
                let mut page = json!({"items": items});
                if from + 10 < 40 {
                    page["nextCursor"] = json!(from + 10);
                }
                ToolResult::text(page.to_string())
            },
            &listed,
        ),
        fake(
            "read_device",
            Reach::DawRead,
            json!({"type":"object","additionalProperties":false,"required":["device"],"properties":{"device":{"type":"string","pattern":"^device:[0-9]+$"}}}),
            |input| {
                let n: i64 = input["device"].as_str().unwrap()["device:".len()..].parse().unwrap();
                if n >= 40 {
                    return ToolResult::error(format!("No device {n}."));
                }
                let name = if n == 3 { hiding() } else { format!("Compressor {n}") };
                ToolResult::text(json!({"name": name, "threshold": if n % 7 == 0 { -24.5 } else { -6.0 }}).to_string())
            },
            &read,
        ),
        fake("set_threshold", Reach::DawWrite, any.clone(), |_| ToolResult::text("set"), &changed),
        fake("make_device", Reach::DawRead, any.clone(), |_| ToolResult::text("made"), &changed),
        fake("find_sounds", Reach::AwayFromDaw, any, |_| ToolResult::text("found"), &searched),
    ];
    Project { tools, read, changed }
}

async fn run(tools: &[Rc<dyn KernelTool>], script: &str) -> ToolResult {
    let tool = code::tool(&DawProfile::ABLETON, tools).expect("a project to read");
    tool.execute(json!({"code": script}).as_object().unwrap().clone(), Signal::new()).await.unwrap()
}

fn answered(result: &ToolResult) -> Value {
    assert!(!result.is_error, "{}", result.text);
    serde_json::from_str(&result.text).unwrap()
}

#[tokio::test]
async fn a_script_reads_every_device_and_only_its_answer_comes_back() {
    let project = project();
    let tool = code::tool(&DawProfile::ABLETON, &project.tools).unwrap();
    // Its functions are the tools that say all they do is read: no change tool, none that reads the DAW and writes a
    // file, and no search that doesn't say so of itself (by its name it runs beside others, which isn't the same).
    assert!(tool.description().ends_with("Functions: list_devices, read_device."), "{}", tool.description());
    assert_eq!(tool.reach(), Reach::DawRead);
    let result = run(
        &project.tools,
        r#"const low = [];
let cursor;
do {
  const page = list_devices(cursor === undefined ? {} : {cursor});
  for (const device of page.items) {
    const settings = await read_device({device});
    if (settings.threshold < -20) low.push({device, name: settings.name});
  }
  cursor = page.nextCursor;
} while (cursor);
print("devices read:", {count: 40});
return {low, others: [typeof set_threshold, typeof make_device, typeof find_sounds]};"#,
    )
    .await;
    let low = [0, 7, 14, 21, 28, 35].map(|n| json!({"device": format!("device:{n}"), "name": format!("Compressor {n}")}));
    assert_eq!(
        answered(&result),
        json!({
            "result": {"low": low, "others": ["undefined", "undefined", "undefined"]},
            "printed": "devices read: {\"count\":40}",
            "calls": 44,
        })
    );
    assert_eq!(project.read.borrow().len(), 40);
    assert!(project.changed.borrow().is_empty());
    // Without a DAW to read, there's no such tool: a few searches need no script.
    assert!(code::tool(&DawProfile::ABLETON, &project.tools[2..]).is_none());

    // What a script starts and doesn't wait for still runs before its answer is read (the reads are made either way),
    // and what JSON has no form for comes back with what it holds: a Map's entries, a Set's members, an error's words.
    let result = run(
        &project.tools,
        r#"const thresholds = new Map(), seen = new Set();
list_devices().items.forEach(async (device) => {
  const settings = await read_device({device});
  thresholds.set(device, settings.threshold);
  seen.add(settings.name);
});
print(new RangeError("too low"), thresholds.size);
return {thresholds, seen, failed: new Error("none")};"#,
    )
    .await;
    let answer = answered(&result);
    assert_eq!(answer["calls"], 11);
    assert_eq!(answer["printed"], "RangeError: too low 0");
    assert_eq!(answer["result"]["failed"], "none");
    assert_eq!(answer["result"]["thresholds"]["device:7"], -24.5);
    assert_eq!(answer["result"]["thresholds"].as_object().unwrap().len(), 10);
    assert_eq!(answer["result"]["seen"].as_array().unwrap().len(), 10);
    // A name that hides words, which the script's reads returned, is put back when the model names it as it was shown
    // it, though here the kernel sees it only as one of a set (and would as a key, or within a longer line).
    assert_eq!(answer["result"]["seen"][3], hiding());
    assert_eq!(named(&format!("Compressor 3{HIDDEN}")), hiding());

    // What's printed is kept to its cap in one unit: a line that fits whole isn't marked cut, whatever it's written in,
    // and lines with nothing on them don't get past the cap.
    let result = run(&project.tools, "print(\"漢\".repeat(1500));\nprint(\"next\");\nreturn 1;").await;
    assert_eq!(answered(&result)["printed"], format!("{}\nnext", "漢".repeat(1500)));
    let result = run(&project.tools, "print(\"a\".repeat(3999));\nfor (let i = 0; i < 50; i++) print(\"\");\nreturn 1;").await;
    assert_eq!(answered(&result)["printed"], format!("{}\n\n…", "a".repeat(3999)));
}

#[tokio::test]
async fn a_calls_error_is_the_scripts_to_catch_and_one_it_doesnt_catch_says_where() {
    let project = project();
    // An input the tool's schema refuses never reaches the tool; the script catches it and goes on.
    let result = run(
        &project.tools,
        r#"let refused;
try { read_device({device: 3}); } catch (error) { refused = error.message; }
return {refused, threshold: read_device({device: "device:5"}).threshold};"#,
    )
    .await;
    let answer = answered(&result);
    assert_eq!(answer["result"]["threshold"], -6.0);
    assert!(answer["result"]["refused"].as_str().unwrap().starts_with("read_device: "), "{answer}");
    assert_eq!(*project.read.borrow(), [json!({"device": "device:5"})]);

    // Uncaught, the tool's own error ends the script: with where it was, what it had done and what it printed.
    let result = run(
        &project.tools,
        "print(\"reading\");\nconst first = read_device({device: \"device:1\"});\nconst missing = read_device({device: \"device:99\"});\nreturn first;",
    )
    .await;
    assert!(result.is_error);
    for part in ["near line 3", "read_device: No device 99", "2 calls", "\nPrinted:\nreading"] {
        assert!(result.text.contains(part), "{part}: {}", result.text);
    }

    // So does one in work the script started and didn't wait for: its top level ended well, but its answer would be
    // short of what that work was to add, and isn't passed on as whole.
    let result = run(
        &project.tools,
        "const names = [];\nlist_devices().items.forEach(async (device) => { names.push((await read_device({device: \"device:99\"})).name); });\nreturn names;",
    )
    .await;
    assert!(result.is_error);
    assert!(result.text.contains("read_device: No device 99") && result.text.contains("didn't wait for"), "{}", result.text);
    // And a promise left in the answer is said, not sent as an empty object.
    let result = run(&project.tools, "return {first: (async () => read_device({device: \"device:1\"}))()};").await;
    assert!(result.is_error);
    assert!(result.text.contains("a promise that wasn't awaited"), "{}", result.text);

    // And so does the script's own mistake: a field the result doesn't have, answered with the fields it has.
    let result = run(&project.tools, "const page = list_devices();\nreturn page.rows.length;").await;
    assert!(result.is_error);
    assert!(result.text.starts_with("The script stopped near line ") && result.text.contains(": TypeError: "), "{}", result.text);
    assert!(result.text.ends_with(" It had made 1 call. Its reads returned list_devices: {items: [text], nextCursor}."), "{}", result.text);
    let result = run(&project.tools, "return list_devices(").await;
    assert!(result.is_error);
    assert!(result.text.starts_with("The script stopped") && result.text.contains(": SyntaxError: "), "{}", result.text);
    assert!(result.text.ends_with(" It had made 0 calls."), "{}", result.text);

    // Nothing returned is nothing read: said, not passed on as an empty answer.
    let result = run(&project.tools, "list_devices();").await;
    assert!(result.is_error);
    assert!(result.text.contains("returned nothing") && result.text.contains("`return`"), "{}", result.text);
}

#[tokio::test]
async fn a_failed_read_that_drops_the_turns_refs_ends_the_script_there() {
    // Live drops the refs a turn has read when it refuses a read. A script that skips the failure would be refused
    // every read after it, skip those too, and answer with what it had as if that were all.
    let (dropped, ran) = (Rc::new(Cell::new(0)), Rc::new(RefCell::new(Vec::new())));
    let tools: Vec<Rc<dyn KernelTool>> = vec![Rc::new(Fake {
        name: "read_notes",
        reach: Reach::DawRead,
        reads: true,
        schema: json!({"type":"object"}),
        answer: |input| match input["clip"].as_i64() {
            Some(2) => ToolResult::error("An audio clip has no notes."),
            _ => ToolResult::text(json!({"notes": 4}).to_string()),
        },
        ran: ran.clone(),
        drops: Some(dropped),
    })];
    let result = run(
        &tools,
        "const notes = [];\nfor (const clip of [1, 2, 3, 4]) { try { notes.push(read_notes({clip}).notes); } catch (error) {} }\nreturn notes;",
    )
    .await;
    assert!(result.is_error, "{}", result.text);
    assert!(result.text.contains("read_notes: An audio clip has no notes") && result.text.contains("drops the refs"), "{}", result.text);
    // It ended at the read that failed: the clips after it were never asked for.
    assert_eq!(ran.borrow().len(), 2);
}

#[tokio::test]
async fn a_script_that_doesnt_end_is_stopped_by_what_it_does() {
    let project = project();
    // A loop that never ends, whatever it catches on the way.
    let result = run(&project.tools, "for (;;) { try { for (;;) {} } catch (error) {} }").await;
    assert!(result.is_error);
    assert!(result.text.starts_with("The script was stopped: it ran too long without ending (0 calls made)."), "{}", result.text);
    // One whose stop lands while a printed value is being written: the stop isn't taken for a value that can't be.
    let result = run(&project.tools, "const stuck = {toJSON() { for (;;) {} }};\nfor (;;) print(stuck);").await;
    assert!(result.is_error);
    assert!(result.text.starts_with("The script was stopped: it ran too long"), "{}", result.text);

    // One that never stops reading: the tool runs the most a script calls, and no more. The same read again and again
    // is a loop that isn't moving on (a cursor followed past the last page, which has none), and it's told so.
    let result = run(
        &project.tools,
        "let cursor;\ndo { cursor = list_devices(cursor === undefined ? {} : {cursor}).nextCursor; } while (cursor !== null);",
    )
    .await;
    assert!(result.is_error);
    assert!(result.text.starts_with("The script was stopped at 2000 calls, the most one makes: "), "{}", result.text);
    assert!(result.text.contains("the same list_devices call 500 times") && result.text.contains("no nextCursor"), "{}", result.text);

    // One that waits on what never comes.
    let result = run(&project.tools, "await new Promise(() => {});").await;
    assert!(result.is_error);
    assert!(result.text.starts_with("The script waits for something that never comes (0 calls made): "), "{}", result.text);

    // And the engine has nothing of the computer in it to reach for.
    let result =
        run(&project.tools, "return [typeof require, typeof process, typeof fetch, typeof std, typeof os, typeof setTimeout];").await;
    assert_eq!(answered(&result)["result"], json!(vec!["undefined"; 6]));
}

/// The engine asks whether to go on inside a regular expression's match too: one that would take years is stopped as a
/// loop is. (The step bound is two hundred million steps of the match, which is what takes this test its seconds.)
#[tokio::test]
async fn a_pattern_that_backtracks_without_end_over_a_long_name_is_stopped_like_a_loop() {
    let project = project();
    let result = run(&project.tools, "return /^(\\w+\\s?)+$/.test(\"Kick_Processed_Final_Bounce_Layer_v2_old!\");").await;
    assert!(result.is_error);
    assert!(result.text.starts_with("The script was stopped: it ran too long"), "{}", result.text);
}

#[tokio::test]
async fn a_script_that_holds_or_nests_too_much_ends_with_an_error_and_kumi_goes_on() {
    // A value nested thousands deep is an error where it's returned, printed or passed, not the end of the process.
    let project = project();
    let result = run(&project.tools, "let deep = [];\nfor (let i = 0; i < 20000; i++) deep = [deep];\nreturn deep;").await;
    assert!(result.is_error);
    assert!(result.text.contains("nests too deep"), "{}", result.text);

    // A script that keeps what 2,000 big reads return runs out of its memory in the middle of its promises' own
    // work, where the engine can't always carry on: it's stopped before that, and says it was memory.
    let tools: Vec<Rc<dyn KernelTool>> = vec![Rc::new(Fake {
        name: "read_page",
        reach: Reach::DawRead,
        reads: true,
        schema: json!({"type":"object"}),
        answer: |_| {
            let items: Vec<Value> = (0..600)
                .map(|n| json!({"ref": format!("clip:{n}"), "name": format!("Clip {n} of a long take, kept as it was read")}))
                .collect();
            ToolResult::text(json!({"items": items}).to_string())
        },
        ran: Rc::default(),
        drops: None,
    })];
    let result = run(
        &tools,
        r#"const refs = Array.from({length: 2000}, (_, i) => "track:" + i);
const settled = await Promise.allSettled(refs.map(async (ref) => {
  const page = read_page({parent: ref});
  return page.items.map(row => row.name + " on " + ref);
}));
return settled.filter(one => one.status === "fulfilled").length;"#,
    )
    .await;
    assert!(result.is_error);
    assert!(result.text.starts_with("The script was stopped: it held more than 64 MB"), "{}", result.text);
}

/// A read that doesn't answer until its call is stopped, and then has something to put back, as a real one does.
struct Slow {
    started: Rc<tokio::sync::Notify>,
    put_back: Rc<Cell<bool>>,
}

#[async_trait(?Send)]
impl KernelTool for Slow {
    fn name(&self) -> &str {
        "read_slowly"
    }
    fn description(&self) -> &str {
        "A read that answers only once it's stopped."
    }
    fn input_schema(&self) -> JsonObject {
        json!({"type":"object"}).as_object().unwrap().clone()
    }
    fn reach(&self) -> Reach {
        Reach::DawRead
    }
    fn only_reads(&self) -> bool {
        true
    }
    async fn execute(&self, _input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        self.started.notify_one();
        signal.cancelled().await;
        // What it puts back takes a turn of the runtime: a read dropped where it stood never gets to it.
        tokio::task::yield_now().await;
        self.put_back.set(true);
        Err(RuntimeError::Aborted)
    }
}

#[tokio::test]
async fn stopping_the_answer_stops_the_script_and_lets_the_read_it_waits_on_wind_down() {
    let (started, put_back) = (Rc::new(tokio::sync::Notify::new()), Rc::new(Cell::new(false)));
    let tools: Vec<Rc<dyn KernelTool>> = vec![Rc::new(Slow { started: started.clone(), put_back: put_back.clone() })];
    let tool = code::tool(&DawProfile::ABLETON, &tools).unwrap();
    let signal = Signal::new();
    let input = json!({"code": "for (;;) { try { read_slowly({}); } catch (error) {} }"}).as_object().unwrap().clone();
    let stop = signal.clone();
    let (result, ()) = tokio::join!(tool.execute(input, signal), async {
        started.notified().await;
        stop.cancel();
    });
    let result = result.unwrap();
    assert!(result.is_error && result.text.contains("stopped"), "{}", result.text);
    // The read under way heard the stop itself and put back what it had to, as it does for a call of the answer's own:
    // it wasn't dropped where it stood.
    assert!(put_back.get());
}
