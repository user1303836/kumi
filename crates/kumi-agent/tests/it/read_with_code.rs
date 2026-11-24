//! read_with_code: a script calls the tools that only read, as functions, and only what it returns comes back.

use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;
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
        self.reads || self.concurrent()
    }
    async fn execute(&self, input: JsonObject, _signal: Signal) -> Result<ToolResult, RuntimeError> {
        self.ran.borrow_mut().push(Value::Object(input.clone()));
        Ok((self.answer)(&input))
    }
}

/// A project of 40 devices: `list_devices` pages through their refs ten at a time, `read_device` gives one's settings
/// (every seventh is set under −20 dB; there's no device past 39). With them, a change tool, a tool that reads the DAW
/// and writes a file, and the web's search.
struct Project {
    tools: Vec<Rc<dyn KernelTool>>,
    read: Rc<RefCell<Vec<Value>>>,
    changed: Rc<RefCell<Vec<Value>>>,
}

fn project() -> Project {
    let ran = || Rc::new(RefCell::new(Vec::new()));
    let (listed, read, changed, searched) = (ran(), ran(), ran(), ran());
    let fake = |name, reach, schema, answer, ran: &Rc<RefCell<Vec<Value>>>| -> Rc<dyn KernelTool> {
        Rc::new(Fake { name, reach, reads: matches!(name, "list_devices" | "read_device"), schema, answer, ran: ran.clone() })
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
                ToolResult::text(json!({"items": items, "nextCursor": (from + 10 < 40).then_some(from + 10)}).to_string())
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
                ToolResult::text(json!({"name": format!("Compressor {n}"), "threshold": if n % 7 == 0 { -24.5 } else { -6.0 }}).to_string())
            },
            &read,
        ),
        fake("set_threshold", Reach::DawWrite, any.clone(), |_| ToolResult::text("set"), &changed),
        fake("make_device", Reach::DawRead, any.clone(), |_| ToolResult::text("made"), &changed),
        fake("search_web", Reach::AwayFromDaw, any, |_| ToolResult::text("found"), &searched),
    ];
    Project { tools, read, changed }
}

async fn run(tools: &[Rc<dyn KernelTool>], script: &str) -> ToolResult {
    let tool = code::tool(&DawProfile::ABLETON, tools).expect("a project to read");
    tool.execute(json!({"code": script}).as_object().unwrap().clone(), Signal::new()).await.unwrap()
}

#[tokio::test]
async fn a_script_reads_every_device_and_only_its_answer_comes_back() {
    let project = project();
    let tool = code::tool(&DawProfile::ABLETON, &project.tools).unwrap();
    // Its functions are the tools that only read the project: no change tool, none that reads the DAW and writes a
    // file, and nothing that leaves the computer.
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
} while (cursor !== null);
print("devices read:", {count: 40});
return {low, others: [typeof set_threshold, typeof make_device, typeof search_web]};"#,
    )
    .await;
    assert!(!result.is_error, "{}", result.text);
    let low = [0, 7, 14, 21, 28, 35].map(|n| json!({"device": format!("device:{n}"), "name": format!("Compressor {n}")}));
    assert_eq!(
        serde_json::from_str::<Value>(&result.text).unwrap(),
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
}

#[tokio::test]
async fn a_calls_error_is_the_scripts_to_catch_and_one_it_doesnt_catch_says_where() {
    let project = project();
    // An input the tool's schema refuses never reaches the tool; the script catches it and goes on.
    let result = run(
        &project.tools,
        r#"let refused;
try { read_device({device: 3}); } catch (error) { refused = error.message; }
return {refused, name: read_device({device: "device:3"}).name};"#,
    )
    .await;
    assert!(!result.is_error, "{}", result.text);
    let answer: Value = serde_json::from_str(&result.text).unwrap();
    assert_eq!(answer["result"]["name"], "Compressor 3");
    assert!(answer["result"]["refused"].as_str().unwrap().starts_with("read_device: "), "{answer}");
    assert_eq!(*project.read.borrow(), [json!({"device": "device:3"})]);

    // Uncaught, the tool's own error ends the script: with where it was, what it had done and what it printed.
    let result = run(
        &project.tools,
        "print(\"reading\");\nconst first = read_device({device: \"device:1\"});\nconst missing = read_device({device: \"device:99\"});\nreturn first;",
    )
    .await;
    assert!(result.is_error);
    assert_eq!(result.text, "The script stopped near line 3: read_device: No device 99. It had made 2 calls.\nPrinted:\nreading");

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
    assert_eq!(result.text, "The script returned nothing (1 call made): end it with `return` and the answer.");
}

#[tokio::test]
async fn a_script_that_doesnt_end_is_stopped_by_what_it_does() {
    let project = project();
    // A loop that never ends, whatever it catches on the way.
    let result = run(&project.tools, "for (;;) { try { for (;;) {} } catch (error) {} }").await;
    assert!(result.is_error);
    assert!(result.text.starts_with("The script was stopped: it ran too long without ending (0 calls made)."), "{}", result.text);

    // One that never stops reading: the tool runs the most a script calls, and no more.
    let result = run(&project.tools, "for (;;) { try { read_device({device: \"device:1\"}); } catch (error) {} }").await;
    assert!(result.is_error);
    assert!(result.text.starts_with("The script was stopped at 2000 calls, the most one makes: "), "{}", result.text);
    assert_eq!(project.read.borrow().len(), 2000);

    // One that waits on what never comes.
    let result = run(&project.tools, "await new Promise(() => {});").await;
    assert!(result.is_error);
    assert!(result.text.starts_with("The script waits for something that never comes (0 calls made): "), "{}", result.text);

    // And the engine has nothing of the computer in it to reach for.
    let result =
        run(&project.tools, "return [typeof require, typeof process, typeof fetch, typeof std, typeof os, typeof setTimeout];").await;
    assert_eq!(serde_json::from_str::<Value>(&result.text).unwrap()["result"], json!(vec!["undefined"; 6]));
}

/// A read that never answers until its call is stopped: it says when it started, and when it was dropped.
struct Stuck {
    started: Rc<tokio::sync::Notify>,
    dropped: Rc<tokio::sync::Notify>,
}

struct SaysDropped(Rc<tokio::sync::Notify>);
impl Drop for SaysDropped {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

#[async_trait(?Send)]
impl KernelTool for Stuck {
    fn name(&self) -> &str {
        "read_slowly"
    }
    fn description(&self) -> &str {
        "A read that never answers."
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
    async fn execute(&self, _input: JsonObject, _signal: Signal) -> Result<ToolResult, RuntimeError> {
        let _dropped = SaysDropped(self.dropped.clone());
        self.started.notify_one();
        std::future::pending().await
    }
}

#[tokio::test]
async fn stopping_the_answer_stops_the_script_and_the_read_it_waits_on() {
    let (started, dropped) = (Rc::new(tokio::sync::Notify::new()), Rc::new(tokio::sync::Notify::new()));
    let tools: Vec<Rc<dyn KernelTool>> = vec![Rc::new(Stuck { started: started.clone(), dropped: dropped.clone() })];
    let tool = code::tool(&DawProfile::ABLETON, &tools).unwrap();
    let signal = Signal::new();
    let input = json!({"code": "for (;;) { try { read_slowly({}); } catch (error) {} }"}).as_object().unwrap().clone();
    let stop = signal.clone();
    let (result, ()) = tokio::join!(tool.execute(input, signal), async {
        started.notified().await;
        stop.cancel();
    });
    assert_eq!(result.unwrap().text, "The script was stopped.");
    // The read under way went with it.
    dropped.notified().await;
}
