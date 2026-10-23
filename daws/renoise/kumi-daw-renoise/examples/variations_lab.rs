//! M7's variations lab (a lab check, not a test): Renoise's variations loop through Kumi's whole stack (the variations tool,
//! undo_change and HISTORY; the host in process; the tool in Renoise), in the song Renoise opened: use a scratch song.
//!
//! variations_lab <kumi-home>
//!
//! - Makes an instrument "K Synth" and a chopped break ("K Break": an instrument of one sample, a track and a slot of lines
//!   written with 0Sxx offsets, a 0Rxy roll and a 0B00 reverse).
//! - "9 phrases for this instrument": V1's lines and eight takes Kumi picks; keep five; change the other four in different
//!   ways; the kept ones read back unchanged; drop the rest in one change.
//! - Variations of the chop as slots of their own (rendered and heard); keep one; drop the rest; the source reads back as it was.
//! - Kumi's undo of one take; Renoise's own undo of the drop, told in HISTORY.
use daw_bridge::Daw;
use daw_mcp_server::host::Host;
use futures::FutureExt;
use kumi_common::abort::Signal;
use kumi_runtime::core::{context::IntegrationContext, contracts::*};
use kumi_runtime::integrations::{
    own_bridge::{Connect, OwnOptions},
    renoise::create_renoise_integration,
};
use kumi_runtime::mcp::client::McpEndpoint;
use kumi_runtime::mcp::types::{CallToolResult, ContentBlock};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::{cell::Cell, cell::RefCell, rc::Rc, sync::Arc, time::Duration, time::Instant};

kumi_testkit::in_process!();
use in_process::InProcess;

/// Between steps Renoise's undo must tell apart.
const GAP_MS: u64 = 450;

fn answer(result: &CallToolResult) -> Result<Value, String> {
    let text: String = result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::String(text.clone()));
    if result.is_error == Some(true) {
        return Err(value.get("error").and_then(Value::as_str).map(str::to_owned).unwrap_or(text));
    }
    Ok(value)
}

async fn host_call(host: &InProcess, name: &str, args: Value) -> Result<Value, String> {
    match host.call(name, args.as_object().cloned().unwrap_or_default(), Signal::new()).await {
        Ok(result) => answer(&result),
        Err(error) => Err(format!("{error:?}")),
    }
}

/// Lua run in Renoise through the host (no HISTORY row): what it returned.
async fn lua(host: &InProcess, code: &str) -> Result<Value, String> {
    let code = format!("{PRELUDE}\n{code}");
    let song = host_call(host, "read", json!({"kind": "song"})).await?;
    let preview =
        host_call(host, "change_preview", json!({"kind": "run-lua", "input": {"code": code, "timeoutMs": 800}, "epoch": song["epoch"]}))
            .await?;
    let (Some(id), Some(confirmation)) = (preview["transactionId"].as_str(), preview["confirmation"].as_str()) else {
        return Err(format!("no transaction: {preview}"));
    };
    let key = uuid::Uuid::new_v4().to_string();
    let applied =
        host_call(host, "change_apply", json!({"transactionId": id, "confirmation": confirmation, "idempotencyKey": key})).await?;
    host_call(host, "change_release", json!({"transactionIds": [id]})).await.ok();
    if let Some(failed) = applied.pointer("/applied/failed") {
        return Err(format!("Lua: {}", failed["message"].as_str().unwrap_or_default()));
    }
    Ok(applied.pointer("/applied/result").cloned().unwrap_or(Value::Null))
}

const PRELUDE: &str = r#"
local R = renoise
local function master_index()
  for k = 1, #song.tracks do if song.tracks[k].type == R.Track.TRACK_TYPE_MASTER then return k end end
end
local function bursts(sample, frames, count)
  local b = sample.sample_buffer
  b:create_sample_data(44100, 16, 1, frames)
  b:prepare_sample_data_changes()
  local step = math.floor(frames / count)
  for f = 1, frames do
    local at = (f - 1) % step
    local env = math.exp(-at / 1500)
    b:set_sample_data(1, f, 0.5 * env * math.sin(f * (80 + 40 * math.floor((f - 1) / step)) * 6.2831853 / 44100))
  end
  b:finalize_sample_data_changes()
end
local function instrument(name, frames, count)
  local i = #song.instruments + 1
  local ins = song:insert_instrument_at(i)
  ins.name = name
  local s = ins:insert_sample_at(1)
  s.name = name
  bursts(s, frames, count)
  return i
end
local function lines_of(p, t, n)
  local out, pt = {}, song:pattern(p):track(t)
  for l = 1, n do
    local line = pt:line(l)
    if not line.is_empty then
      local nc, e1, e2 = line:note_column(1), line:effect_column(1), line:effect_column(2)
      out[#out + 1] = (l - 1) .. ":" .. nc.note_string .. nc.volume_string .. nc.delay_string .. nc.effect_number_string .. nc.effect_amount_string
        .. "|" .. e1.number_string .. e1.amount_string .. "|" .. e2.number_string .. e2.amount_string
    end
  end
  return table.concat(out, " ")
end
local function phrase_lines(i, p)
  local out, ph = {}, song:instrument(i):phrase(p)
  for l = 1, ph.number_of_lines do
    local line = ph:line(l)
    if not line.is_empty then
      local nc, effects = line:note_column(1), {}
      for c = 1, 4 do effects[c] = line:effect_column(c).number_string .. line:effect_column(c).amount_string end
      out[#out + 1] = (l - 1) .. ":" .. nc.note_string .. nc.volume_string .. nc.effect_number_string .. nc.effect_amount_string .. "|" .. table.concat(effects, "|")
    end
  end
  return table.concat(out, " ")
end
local function phrases(i)
  local out = {}
  for k, ph in ipairs(song:instrument(i).phrases) do out[k] = ph.name .. "=" .. phrase_lines(i, k) end
  return out
end
"#;

struct Lab {
    host: Rc<InProcess>,
    tools: Vec<Rc<dyn KernelTool>>,
    results: RefCell<Vec<(String, bool, String)>>,
}

impl Lab {
    async fn lua(&self, code: &str) -> Value {
        match lua(&self.host, code).await {
            Ok(value) => value,
            Err(why) => {
                println!("   lua failed: {why}");
                Value::Null
            }
        }
    }

    async fn tool(&self, name: &str, input: Value) -> (Value, bool, u128) {
        let Some(tool) = self.tools.iter().find(|tool| tool.name() == name).cloned() else {
            return (json!(format!("no tool {name}")), true, 0);
        };
        let t = Instant::now();
        let done = tool.execute(input.as_object().cloned().unwrap_or_default(), Signal::new()).await;
        let ms = t.elapsed().as_millis();
        match done {
            Ok(result) => (serde_json::from_str(&result.text).unwrap_or(Value::String(result.text)), result.is_error, ms),
            Err(error) => (json!(format!("{error:?}")), true, ms),
        }
    }

    fn check(&self, name: &str, ok: bool, detail: impl Into<String>) {
        let detail = detail.into();
        println!("{} {name}: {}", if ok { "PASS" } else { "FAIL" }, short(&detail, 4000));
        self.results.borrow_mut().push((name.into(), ok, detail));
    }

    async fn pause(&self) {
        tokio::time::sleep(Duration::from_millis(GAP_MS)).await;
    }

    async fn epoch(&self) -> String {
        host_call(&self.host, "read", json!({"kind": "song"}))
            .await
            .ok()
            .and_then(|song| song["epoch"].as_str().map(str::to_owned))
            .unwrap_or_default()
    }
}

fn short(text: &str, n: usize) -> String {
    let flat: String = text.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    if flat.chars().count() > n {
        format!("{}…", flat.chars().take(n).collect::<String>())
    } else {
        flat
    }
}

fn main() {
    let Some(home) = std::env::args().nth(1) else {
        eprintln!("variations_lab <kumi-home>");
        std::process::exit(2);
    };
    let home = PathBuf::from(home);
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
    tokio::task::LocalSet::new().block_on(&runtime, async {
        let host = Rc::new(InProcess { host: Arc::new(Host::new(Daw::Renoise, &home)), id: Cell::new(0) });
        let endpoint = host.clone();
        let connect: Connect = Rc::new(move |_| {
            let endpoint = endpoint.clone();
            async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
        });
        let rows: Rc<RefCell<Vec<ChangeRecord>>> = Rc::default();
        let into = rows.clone();
        let mut context = IntegrationContext::new(Rc::new(|_, _| {}));
        context.on_change = Some(Rc::new(move |record| {
            let mut rows = into.borrow_mut();
            match rows.iter_mut().find(|row| row.id == record.id) {
                Some(row) => *row = record,
                None => rows.push(record),
            }
        }));
        let daw = create_renoise_integration(OwnOptions { context, kumi_home: home.clone(), connect: Some(connect) });
        if let Err(error) = daw.start(Signal::new()).await {
            eprintln!("no Renoise: {error:?}");
            std::process::exit(1);
        }
        let observed = daw.observe(Signal::new(), None).await.expect("a look at Renoise");
        // (once more after the listening flags are read: the tools Kumi offers then)
        tokio::time::sleep(Duration::from_millis(500)).await;
        let observed = daw.observe(Signal::new(), None).await.unwrap_or(observed);
        let lab = Lab { host: host.clone(), tools: observed.tools.clone(), results: RefCell::default() };
        println!("== connected ({} tools; variations: {})", lab.tools.len(), lab.tools.iter().any(|tool| tool.name() == "variations"));
        lab.lua("song.artist = 'K undo check'").await;
        lab.pause().await;
        if lab.lua("return song:can_undo()").await != json!(true) {
            eprintln!("Renoise keeps no undo step in this session: quit and start it again");
            daw.close().await.ok();
            std::process::exit(3);
        }
        lab.lua("song:undo()").await;
        lab.pause().await;

        // The scratch material: K Synth, and K Break with its chop on a track of its own.
        let made = lab
            .lua(
                r#"
local synth = instrument('K Synth', 22050, 4)
local brk = instrument('K Break', 88200, 8)
local m = master_index()
local tr = song:insert_track_at(m)
tr.name = 'K Break'
tr.visible_effect_columns = 2
tr.volume_column_visible = true
tr.delay_column_visible = true
local q = #song.sequencer.pattern_sequence + 1
local p = song.sequencer:insert_new_pattern_at(q)
song:pattern(p).name = 'K Chop'
local pt = song:pattern(p):track(m)
local chop = { {0, '00', '..', '..'}, {3, '40', '20', '..'}, {4, '80', '..', '..'}, {6, '60', '..', '40'}, {7, '20', '30', '..'},
  {10, '80', '..', '..'}, {12, '80', '..', '..'}, {14, 'A0', '..', '..'} }
for _, c in ipairs(chop) do
  local line = pt:line(c[1] + 1)
  local nc = line:note_column(1)
  nc.note_string = 'C-4'
  nc.instrument_value = brk - 1
  if c[3] ~= '..' then nc.volume_string = c[3] end
  if c[4] ~= '..' then nc.delay_string = c[4] end
  line:effect_column(1).number_string = '0S'
  line:effect_column(1).amount_string = c[2]
end
pt:line(7):effect_column(2).number_string = '0R'
pt:line(7):effect_column(2).amount_string = '03'
pt:line(11):effect_column(2).number_string = '0B'
pt:line(11):effect_column(2).amount_string = '00'
return {synth = synth, brk = brk, t = m, p = p, q = q}
"#,
            )
            .await;
        println!("== material: {made}");
        let (synth, t, p) = (made["synth"].as_u64().unwrap_or(1), made["t"].as_u64().unwrap_or(1), made["p"].as_u64().unwrap_or(1));
        lab.pause().await;
        let source_lines = lab.lua(&format!("return lines_of({p}, {t}, 16)")).await;
        let e = lab.epoch().await;

        // A: nine phrases, keep five, change four, drop the rest.
        let arp = json!([
            {"line": 0, "notes": [{"col": 1, "note": "C-4"}]}, {"line": 2, "notes": [{"col": 1, "note": "G-4", "volume": "50"}]},
            {"line": 4, "notes": [{"col": 1, "note": "E-4"}]}, {"line": 6, "notes": [{"col": 1, "note": "G-4", "volume": "40"}]},
            {"line": 8, "notes": [{"col": 1, "note": "B-4"}]}, {"line": 12, "notes": [{"col": 1, "note": "G-4"}]},
        ]);
        let before_rows = rows.borrow().len();
        let (made, error, ms) = lab
            .tool("variations", json!({"action": "make", "source": {"instrumentRef": format!("{e}:instrument:{synth}")}, "options": {"lines": 16},
                "takes": [{"direction": "an arp", "lines": arp}], "count": 9, "seed": 11}))
            .await;
        let count = made["takes"].as_array().map_or(0, Vec::len);
        lab.check("9 phrases made", !error && count == 9, format!("{ms} ms: {made}"));
        let heard = made["takes"].as_array().is_some_and(|takes| takes.iter().all(|take| take["line"].as_str().unwrap_or("").contains("heard:")));
        lab.check("each phrase take heard (players of its phrase)", heard, made.get("heard").map(Value::to_string).unwrap_or_default());
        lab.check("each take one HISTORY row", rows.borrow().len() - before_rows == count, format!("{} rows", rows.borrow().len() - before_rows));
        let phrases_before = lab.lua(&format!("return phrases({synth})")).await;
        lab.check("9 phrases in Renoise", phrases_before.as_array().map_or(0, Vec::len) == 9, phrases_before.to_string());
        lab.pause().await;
        let (kept, error, _) = lab.tool("variations", json!({"action": "keep", "names": ["V1", "V3", "V5", "V7", "V9"]})).await;
        lab.check("keep 5", !error && kept["rest"] == json!(["V2", "V4", "V6", "V8"]), kept.to_string());
        let phrases_before = lab.lua(&format!("return phrases({synth})")).await;
        let marked = phrases_before.as_array().is_some_and(|names| {
            names.iter().enumerate().all(|(k, name)| name.as_str().unwrap_or("").split('=').next().unwrap_or("").ends_with(" \u{b7} kept") == (k % 2 == 0))
        });
        lab.check("the kept 5 marked “· kept” in Renoise", marked, phrases_before.to_string());
        let (changed, error, ms) = lab.tool("variations", json!({"action": "change", "names": ["rest"]})).await;
        lab.check("change the other 4", !error && changed["changed"].as_array().map_or(0, Vec::len) == 4, format!("{ms} ms: {changed}"));
        let phrases_after = lab.lua(&format!("return phrases({synth})")).await;
        let same = |k: usize| phrases_before.get(k) == phrases_after.get(k);
        lab.check("the kept 5 unchanged", [0, 2, 4, 6, 8].iter().all(|k| same(*k)), phrases_after.to_string());
        lab.check("the other 4 changed", [1, 3, 5, 7].iter().all(|k| !same(*k)), String::new());
        lab.pause().await;
        let (dropped, error, ms) = lab.tool("variations", json!({"action": "drop"})).await;
        let left = lab.lua(&format!("local out = {{}} for k, ph in ipairs(song:instrument({synth}).phrases) do out[k] = ph.name end return out")).await;
        lab.check("drop the rest in one change", !error && left.as_array().map_or(0, Vec::len) == 5, format!("{ms} ms: {dropped} → {left}"));
        lab.pause().await;
        // Renoise's own undo of the drop: HISTORY says so.
        lab.lua("song:undo()").await;
        tokio::time::sleep(Duration::from_millis(2500)).await;
        let back = lab.lua(&format!("return #song:instrument({synth}).phrases")).await;
        let row = rows.borrow().last().cloned();
        lab.check(
            "Renoise's undo of the drop, told",
            back == json!(9) && row.as_ref().is_some_and(|row| row.state == ChangeState::Undone),
            format!("{back} phrases; last row {:?}", row.map(|row| (row.title, row.state))),
        );
        lab.lua("song:redo()").await;
        lab.pause().await;

        // B: variations of the chop as slots of their own, rendered, from a slot whose break track is an alias of the chop's (V2):
        // the takes copy it, never write through it.
        let aliased = lab
            .lua(&format!(
                "local s = song.sequencer local q = #s.pattern_sequence + 1 local p2 = s:insert_new_pattern_at(q) song:pattern(p2).name = 'K Chop alias' song:pattern(p2):track({t}).alias_pattern_index = {p} return {{q = q, p2 = p2}}"
            ))
            .await;
        let (q2, p2) = (aliased["q"].as_u64().unwrap_or(0), aliased["p2"].as_u64().unwrap_or(0));
        lab.pause().await;
        let e = lab.epoch().await;
        let (made, error, ms) = lab
            .tool("variations", json!({"action": "make", "source": {"patternRef": format!("{e}:seq:{q2}"), "trackRef": format!("{e}:track:{t}"), "first": 0, "last": 15},
                "anchors": [0, 4], "seed": 23, "takes": [
                    {"direction": "swapped slices", "ops": [{"op": "shuffle", "amount": 0.8}]},
                    {"direction": "rolls moved", "ops": [{"op": "retrig", "amount": 0.4}, {"op": "ghosts", "amount": 0.6}]},
                    {"direction": "more reverses", "ops": [{"op": "reverse", "amount": 0.4}]},
                    {"direction": "other slices", "ops": [{"op": "offsets", "amount": 0.5}]}]}))
            .await;
        lab.check("4 chop variations made", !error && made["takes"].as_array().map_or(0, Vec::len) == 4, format!("{ms} ms: {made}"));
        let slots = lab.lua("local out = {} local s = song.sequencer for k, p in ipairs(s.pattern_sequence) do out[k] = p .. ':' .. song:pattern(p).name .. (s:sequence_is_start_of_section(k) and ('[' .. s:sequence_section_name(k) .. ']') or '') end return out").await;
        println!("   sequence: {slots}");
        let source_now = lab.lua(&format!("return lines_of({p}, {t}, 16)")).await;
        lab.check("the chop (the alias's source) as it was", source_now == source_lines, format!("{source_now}"));
        let alias_now = lab.lua(&format!("return song:pattern({p2}):track({t}).alias_pattern_index")).await;
        lab.check("the source slot's track still an alias", alias_now == json!(p), alias_now.to_string());
        lab.pause().await;
        let (kept, error, _) = lab.tool("variations", json!({"action": "keep", "names": ["V2"]})).await;
        lab.check("keep V2", !error, kept.to_string());
        let (dropped, error, ms) = lab.tool("variations", json!({"action": "drop", "names": ["rest"]})).await;
        lab.check("drop the other 3 in one change", !error && dropped["dropped"].as_array().map_or(0, Vec::len) == 3, format!("{ms} ms: {dropped}"));
        let slots = lab.lua("local out = {} for k, p in ipairs(song.sequencer.pattern_sequence) do out[k] = p .. ':' .. song:pattern(p).name end return out").await;
        println!("   sequence after the drop: {slots}");
        lab.pause().await;

        // C: Kumi's undo of the kept chop take: its mark first, then its slot.
        let ids: Vec<String> = rows.borrow().iter().rev().filter(|row| row.title.contains("V2 \u{b7} K Chop alias")).map(|row| row.id.clone()).take(2).collect();
        for (k, id) in ids.iter().enumerate() {
            let (undone, error, _) = lab.tool("undo_change", json!({"change": id})).await;
            lab.check(if k == 0 { "Kumi's undo of V2's kept mark" } else { "Kumi's undo of V2's slot" }, !error, undone.to_string());
            lab.pause().await;
        }
        let rows_now = rows.borrow().len();
        println!("== {} HISTORY rows", rows_now);
        for row in rows.borrow().iter() {
            println!("   {} {:?} {}", row.id, row.state, short(&row.title, 120));
        }
        daw.close().await.ok();

        // D: Kumi again (another run, the same folder): the sets are there, "rest" reaches no kept take, numbers go on.
        let endpoint = Rc::new(InProcess { host: Arc::new(Host::new(Daw::Renoise, &home)), id: Cell::new(0) });
        let connect: Connect = Rc::new(move |_| {
            let endpoint = endpoint.clone();
            async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
        });
        let again = create_renoise_integration(OwnOptions { context: IntegrationContext::new(Rc::new(|_, _| {})), kumi_home: home.clone(), connect: Some(connect) });
        again.start(Signal::new()).await.ok();
        let observed = again.observe(Signal::new(), None).await.expect("a look at Renoise");
        let lab = Lab { host: lab.host.clone(), tools: observed.tools.clone(), results: lab.results };
        let (listed, error, _) = lab.tool("variations", json!({"action": "list", "set": "K Synth"})).await;
        let kept = listed["takes"].as_array().map_or(0, |takes| takes.iter().filter(|line| line.as_str().unwrap_or("").ends_with("(kept)")).count());
        lab.check("after a restart of Kumi, the phrase set and its 5 kept takes", !error && kept == 5, listed.to_string());
        let (refused, error, _) = lab.tool("variations", json!({"action": "drop", "set": "K Synth"})).await;
        lab.check("\"rest\" reaches no kept take", error && refused.to_string().contains("names no take"), refused.to_string());
        let e = lab.epoch().await;
        let (more, error, _) = lab
            .tool("variations", json!({"action": "make", "source": {"instrumentRef": format!("{e}:instrument:{synth}")}, "options": {"lines": 16}, "listen": false,
                "takes": [{"from": "V1", "ops": [{"op": "octave", "amount": 1, "up": true}]}], "seed": 12}))
            .await;
        lab.check("a make on the same source goes on numbering", !error && more["takes"][0]["take"] == "V10", more.to_string());
        let failed = lab.results.borrow().iter().filter(|(_, ok, _)| !ok).count();
        println!("== {} checks, {failed} failed", lab.results.borrow().len());
        again.close().await.ok();
    });
}
