use lynx_kumi::native::{proof, PROTOCOL};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::{JoinHandle, JoinSet},
};

pub struct State {
    pub session: Value,
    pub notes: Vec<Value>,
    pub calls: Vec<(String, Value)>,
    pub authenticated: usize,
    pub tokens_on_wire: usize,
    pub after_bad_challenge: Option<String>,
    pub gui_value: f64,
    pub opaque_device_state: String,
    pub drop_mutation_response: bool,
    pattern_notes: HashMap<String, Vec<Value>>,
    history: Vec<Saved>,
}
struct Saved {
    session: Value,
    notes: Vec<Value>,
    pattern_notes: HashMap<String, Vec<Value>>,
}
pub struct Fake {
    directory: tempfile::TempDir,
    pub state: Arc<Mutex<State>>,
    stop: Option<oneshot::Sender<()>>,
    running: JoinHandle<()>,
}
impl Fake {
    pub async fn start(bad_proof: bool) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("native.json");
        daw_bridge::owner::write_owner_only(&path,json!({"protocol":PROTOCOL,"host":"127.0.0.1","port":listener.local_addr().unwrap().port(),"token":"a".repeat(64),"pid":std::process::id(),"appVersion":"0.1.23-test"}).to_string().as_bytes()).unwrap();
        let state = Arc::new(Mutex::new(State {
            session: json!({"project":{"id":"1234567890abcdef1234567890abcdef","name":"Instrument test","path":"/tmp/fixture.lynx","modified":false},"controlEpoch":"b".repeat(64),"revision":1,"appVersion":"0.1.23-test","bpm":120,"beatsPerBar":4,"playing":false,"recording":false,"songMode":false,"positionTicks":0,"currentPattern":0,"channels":[{"index":0,"channelId":"01","name":"Synth","instrument":"native:4","volume":0.78,"pan":0,"muted":false,"mixer":1},{"index":1,"channelId":"02","name":"Kick","instrument":"native:0","volume":0.6,"pan":0.1,"muted":false,"mixer":2}],"mixer":[{"index":0,"name":"Master","volume":1,"pan":0,"muted":false,"solo":false}],"patterns":[{"index":0,"patternId":"03","name":"Pattern 1","noteCount":3,"lengthTicks":0,"colour":9}],"playlist":[],"capabilities":{"patterns":true,"instruments":true,"effects":true,"playlist":true,"pluginParameters":true},"canUndo":false,"canRedo":false}),
            notes: vec![json!({"key":60,"channel":0}), json!({"key":64,"channel":1}), json!({"key":67,"channel":0})],
            calls: vec![],
            authenticated: 0,
            tokens_on_wire: 0,
            after_bad_challenge: None,
            gui_value: 0.5,
            opaque_device_state: "fixture-state".into(),
            drop_mutation_response: false,
            pattern_notes: HashMap::new(),
            history: vec![],
        }));
        let (stop, mut stopped) = oneshot::channel();
        let shared = state.clone();
        let running = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    _=&mut stopped=>break,
                    accepted=listener.accept()=>{let(stream,_)=accepted.unwrap();let state=shared.clone();connections.spawn(async move{handle(stream,state,bad_proof).await;});}
                    Some(result)=connections.join_next(),if !connections.is_empty()=>{result.unwrap();}
                }
            }
            while let Some(result) = connections.join_next().await {
                result.unwrap();
            }
        });
        Self { directory, state, stop: Some(stop), running }
    }
    pub fn path(&self) -> PathBuf {
        self.directory.path().join("native.json")
    }
    pub async fn finish(mut self) -> State {
        let _ = self.stop.take().unwrap().send(());
        self.running.await.unwrap();
        Arc::try_unwrap(self.state).ok().unwrap().into_inner().unwrap()
    }
}
async fn handle(stream: TcpStream, state: Arc<Mutex<State>>, bad_proof: bool) {
    let mut stream = BufReader::new(stream);
    let Some(hello) = read(&mut stream).await else { return };
    let nonce = hello["nonce"].as_str().unwrap();
    assert_eq!(hello["protocol"], PROTOCOL);
    let server = "c".repeat(32);
    let server_proof = if bad_proof { "0".repeat(64) } else { proof(&"a".repeat(64), "server", nonce, &server) };
    if !send(&mut stream, &json!({"type":"challenge","protocol":PROTOCOL,"nonce":server,"proof":server_proof})).await {
        return;
    }
    if bad_proof {
        let mut line = String::new();
        stream.read_line(&mut line).await.unwrap();
        state.lock().unwrap().after_bad_challenge = Some(line);
        return;
    }
    let Some(auth) = read(&mut stream).await else { return };
    assert_eq!(auth["type"], "auth");
    assert_eq!(auth["proof"], proof(&"a".repeat(64), "client", nonce, &server));
    state.lock().unwrap().authenticated += 1;
    if !send(&mut stream, &json!({"type":"ready","protocol":PROTOCOL})).await {
        return;
    }
    let Some(request) = read(&mut stream).await else { return };
    if request.get("token").is_some() {
        state.lock().unwrap().tokens_on_wire += 1;
    }
    let op = request["op"].as_str().unwrap().to_owned();
    let args = request["args"].clone();
    let response = {
        let mut state = state.lock().unwrap();
        state.calls.push((op.clone(), args.clone()));
        execute(&mut state, &op, &args)
    };
    if op == "tempo.set" {
        let mut state = state.lock().unwrap();
        if state.drop_mutation_response {
            state.drop_mutation_response = false;
            return;
        }
    }
    let _ = send(&mut stream, &response).await;
}
fn execute(state: &mut State, op: &str, args: &Value) -> Value {
    let metadata = |state: &State, mut result: Value| {
        result["projectId"] = state.session["project"]["id"].clone();
        result["controlEpoch"] = state.session["controlEpoch"].clone();
        result["revision"] = state.session["revision"].clone();
        json!({"ok":true,"result":result})
    };
    match op {
        "session.inspect" | "status" => json!({"ok":true,"result":state.session}),
        "api.describe" => metadata(state, json!({"protocol":PROTOCOL,"operations":["project.read","project.edit"]})),
        "device.state" => metadata(state, json!({"deviceStateHash":device_hash(state)})),
        "project.read" => {
            use base64::{engine::general_purpose::STANDARD, Engine};
            let bytes = document(state).to_string().into_bytes();
            let hash = daw_bridge::auth::sha256_hex(&bytes);
            let offset = args["offset"].as_u64().unwrap_or(0) as usize;
            let limit = args["limit"].as_u64().unwrap_or(32768) as usize;
            if offset > bytes.len() || (offset > 0 && (args["snapshotId"] != "e".repeat(32) || args["expectedDocumentHash"] != hash)) {
                return json!({"ok":false,"code":"changed","error":"stale snapshot","uncertain":false});
            }
            let end = (offset + limit).min(bytes.len());
            let mut result = json!({"encoding":"base64","data":STANDARD.encode(&bytes[offset..end]),"snapshotId":"e".repeat(32),"documentHash":hash,"offset":offset,"totalBytes":bytes.len()});
            if end < bytes.len() {
                result["nextOffset"] = end.into();
            }
            metadata(state, result)
        }
        "pattern.notes" => {
            let offset = args["offset"].as_u64().unwrap_or(0) as usize;
            let limit = args["limit"].as_u64().unwrap_or(256) as usize;
            let index = args["index"].as_u64().unwrap() as usize;
            let source = notes_for(state, index);
            let notes = source.iter().skip(offset).take(limit).cloned().collect::<Vec<_>>();
            let mut result = json!({"index":index,"offset":offset,"notes":notes,"totalNotes":source.len()});
            if offset + limit < source.len() {
                result["nextOffset"] = (offset + limit).into();
            }
            metadata(state, result)
        }
        "instrument.catalog" => metadata(
            state,
            json!({"instruments":[{"instrumentId":"lynx:gen:Synth","name":"Synth","type":"native","available":true}],"count":1,"offset":0}),
        ),
        "plugin.inspect" => {
            if args.get("target").is_some() {
                metadata(
                    state,
                    json!({"target":args["target"],"available":true,"parameterCount":1,"parameters":[{"index":0,"parameterId":"gain","value":state.gui_value,"readOnly":false}]}),
                )
            } else {
                metadata(
                    state,
                    json!({"devices":[{"target":{"kind":"effect","mixerIndex":0,"effectIndex":0},"name":"Limiter","pluginId":"native:1","type":"native","available":true,"parameterCount":1}]}),
                )
            }
        }
        "tempo.set" | "pattern.replace_notes" | "pattern.create" | "pattern.clone" | "pattern.set" | "project.edit" | "history.undo" => {
            if args["expectedProjectId"] != state.session["project"]["id"]
                || args["expectedEpoch"] != state.session["controlEpoch"]
                || args["expectedRevision"].as_u64() != state.session["revision"].as_u64()
            {
                return json!({"ok":false,"error":"stale native guard"});
            }
            let revision = state.session["revision"].as_u64().unwrap() + 1;
            let mut pattern_index = args["index"].as_u64().unwrap_or(state.session["patterns"].as_array().unwrap().len() as u64) as usize;
            if op == "pattern.set" {
                let old = &state.session["patterns"][pattern_index];
                let changed = ["name", "lengthTicks"].iter().any(|key| args.get(*key).is_some_and(|value| value != &old[*key]));
                if !changed {
                    return metadata(state, pattern_result(state, pattern_index, false));
                }
            }
            if op == "history.undo" {
                let Some(saved) = state.history.pop() else { return json!({"ok":false,"error":"no undo"}) };
                state.session = saved.session;
                state.notes = saved.notes;
                state.pattern_notes = saved.pattern_notes;
            } else {
                state.history.push(Saved {
                    session: state.session.clone(),
                    notes: state.notes.clone(),
                    pattern_notes: state.pattern_notes.clone(),
                });
                if op == "tempo.set" {
                    state.session["bpm"] = args["bpm"].clone();
                } else if op == "project.edit" {
                    if args["expectedDocumentHash"] != daw_bridge::auth::sha256_hex(document(state).to_string().as_bytes()) {
                        state.history.pop();
                        return json!({"ok":false,"code":"changed","error":"document changed","uncertain":false});
                    }
                    for patch in args["patch"].as_array().unwrap() {
                        match patch["path"].as_str().unwrap() {
                            "/tempo" => state.session["bpm"] = patch["value"].clone(),
                            "/channels/0/name" => state.session["channels"][0]["name"] = patch["value"].clone(),
                            _ => panic!("test patch path not implemented"),
                        }
                    }
                } else if op == "pattern.replace_notes" {
                    let notes = args["notes"].as_array().unwrap().clone();
                    let id = state.session["patterns"][pattern_index]["patternId"].as_str().unwrap().to_owned();
                    state.session["patterns"][pattern_index]["noteCount"] = notes.len().into();
                    if id == "03" {
                        state.notes = notes;
                    } else {
                        state.pattern_notes.insert(id, notes);
                    }
                } else if op == "pattern.set" {
                    for field in ["name", "lengthTicks"] {
                        if let Some(value) = args.get(field) {
                            state.session["patterns"][pattern_index][field] = value.clone();
                        }
                    }
                } else if op == "pattern.create" || op == "pattern.clone" {
                    let id = format!("{:016x}", 0x100 + revision);
                    let mut row = if op == "pattern.clone" {
                        let notes = notes_for(state, pattern_index).to_vec();
                        state.pattern_notes.insert(id.clone(), notes);
                        let source = state.session["patterns"][pattern_index].clone();
                        pattern_index += 1;
                        source
                    } else {
                        state.pattern_notes.insert(id.clone(), vec![]);
                        json!({"name":format!("Pattern {}", state.session["patterns"].as_array().unwrap().len()+1),"noteCount":0,"lengthTicks":args["lengthTicks"].as_u64().unwrap_or(0),"colour":0})
                    };
                    row["patternId"] = id.into();
                    if let Some(name) = args.get("name") {
                        row["name"] = name.clone();
                    } else if op == "pattern.clone" {
                        row["name"] = format!("{} #2", row["name"].as_str().unwrap()).into();
                    }
                    let patterns = state.session["patterns"].as_array_mut().unwrap();
                    patterns.insert(pattern_index, row);
                    for (index, row) in patterns.iter_mut().enumerate() {
                        row["index"] = index.into();
                    }
                    for clip in state.session["playlist"].as_array_mut().unwrap() {
                        if clip["kind"] == "pattern" && clip["pattern"].as_u64().is_some_and(|p| p >= pattern_index as u64) {
                            clip["pattern"] = (clip["pattern"].as_u64().unwrap() + 1).into();
                        }
                    }
                    state.session["currentPattern"] = pattern_index.into();
                }
            }
            state.session["revision"] = revision.into();
            state.session["canUndo"] = (!state.history.is_empty()).into();
            state.session["canRedo"] = (op == "history.undo").into();
            if matches!(op, "pattern.create" | "pattern.clone" | "pattern.set") {
                metadata(state, pattern_result(state, pattern_index, true))
            } else {
                metadata(state, json!({"changed":true,"bpm":state.session["bpm"]}))
            }
        }
        "transport.stop" => {
            state.session["playing"] = false.into();
            metadata(state, json!({"playing":false}))
        }
        _ => json!({"ok":false,"error":"unsupported fake operation"}),
    }
}
fn notes_for(state: &State, index: usize) -> &[Value] {
    let id = state.session["patterns"][index]["patternId"].as_str().unwrap();
    if id == "03" {
        &state.notes
    } else {
        &state.pattern_notes[id]
    }
}
fn pattern_result(state: &State, index: usize, changed: bool) -> Value {
    let row = &state.session["patterns"][index];
    json!({"changed":changed,"index":index,"patternId":row["patternId"],"name":row["name"],"lengthTicks":row["lengthTicks"],"patternCount":state.session["patterns"].as_array().unwrap().len()})
}
async fn read(stream: &mut BufReader<TcpStream>) -> Option<Value> {
    let mut line = String::new();
    if stream.read_line(&mut line).await.unwrap() == 0 {
        return None;
    }
    Some(serde_json::from_str(&line).unwrap())
}
async fn send(stream: &mut BufReader<TcpStream>, value: &Value) -> bool {
    stream.get_mut().write_all(format!("{value}\n").as_bytes()).await.is_ok()
}

fn document(state: &State) -> Value {
    let patterns = state.session["patterns"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let mut row = row.clone();
            row["notes"] = json!(notes_for(state, index));
            row
        })
        .collect::<Vec<_>>();
    json!({"format":"lynx","projectId":state.session["project"]["id"],"tempo":state.session["bpm"],"channels":state.session["channels"],"mixer":state.session["mixer"],"patterns":patterns,"playlist":state.session["playlist"]})
}
fn device_hash(state: &State) -> String {
    daw_bridge::auth::sha256_hex(format!("{}:{}", state.gui_value, state.opaque_device_state).as_bytes())
}
