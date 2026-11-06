//! Pure background candidates, persisted before any separately requested foreground apply.
use super::*;
use kumi_common::abort::SignalExt;
use kumi_store::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const BOARD_BYTES: i64 = 8 * 1024 * 1024;

pub struct PreparedMake<B: VariationsBackend> {
    project: String,
    source: Source<B::Material, B::Data>,
    input: JsonObject,
    pub board_id: String,
    pub request_key: String,
    ran: Cell<bool>,
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum State {
    Planned,
    Kept,
    Dropped,
    Applying,
    Applied,
    Unsure,
}

struct Board<B: VariationsBackend> {
    id: String,
    project: String,
    set: Set<B>,
    states: BTreeMap<usize, State>,
    stopped: Option<String>,
    revision: i64,
}

impl<B: VariationsBackend + 'static> Variations<B> {
    pub async fn prepare_make(&self, input: JsonObject, signal: &Signal) -> Result<PreparedMake<B>, String> {
        signal.check().map_err(|error| error.to_string())?;
        if input.get("action").and_then(Value::as_str) != Some("make") || input.contains_key("board") {
            return Err("A background request makes a new candidate board: action make, without board.".into());
        }
        let project = self.backend.project_key(signal).await.ok_or("Observe an identified project before requesting candidates.")?;
        let source = self.backend.prepare_source(&input, signal).await?;
        self.recipes_of(&input, &source)?;
        signal.check().map_err(|error| error.to_string())?;
        let request_key = hex::encode(Sha256::digest(
            serde_json::to_vec(&json!({"project":project,"input":input,
            "source":{"key":source.key,"data":self.backend.data_json(&source.data),
                "material":source.material.as_ref().map(|material|self.backend.material_json(material))}}))
            .map_err(|error| error.to_string())?,
        ));
        Ok(PreparedMake { project, source, input, board_id: uuid::Uuid::new_v4().to_string(), request_key, ran: Cell::new(false) })
    }

    /// No asynchronous backend method is called here: only the captured material enters the pure generator.
    pub async fn run_prepared(&self, prepared: &PreparedMake<B>, signal: &Signal, progress: &dyn Fn(&str)) -> Result<Value, String> {
        let _held = self.busy.lock().await;
        if prepared.ran.replace(true) {
            return Err("This request already ran; inspect its immutable board rather than replaying it.".into());
        }
        self.load(Some(&prepared.project));
        let recipes = self.recipes_of(&prepared.input, &prepared.source)?;
        let seed = prepared.input.get("seed").and_then(Value::as_u64).unwrap_or_else(fresh_seed);
        let mut set = TakeSet::new(0, prepared.source.clone(), seed);
        set.floor = self.numbered.borrow().get(&set.source.short).copied().unwrap_or(0);
        let mut board = Board {
            id: prepared.board_id.clone(),
            project: prepared.project.clone(),
            set,
            states: BTreeMap::new(),
            stopped: None,
            revision: 0,
        };
        self.save_board(&mut board)?;
        let bounds = bounds_of(&prepared.input);
        let mut used = recipes.iter().map(|recipe| recipe.direction.clone()).collect();
        for mut recipe in recipes {
            if signal.is_cancelled() {
                board.stopped = Some("Cancelled; completed candidates remain, and no DAW writes were made.".into());
                break;
            }
            let number = board.set.next_number();
            progress(&format!("Planning V{number}"));
            if recipe.picked && recipe.from.is_none() && board.set.source.material.is_none() {
                recipe.from = board.set.takes.first().map(|take| take.number);
            }
            let (drawn, _) = match self.draw_picking(&board.set, number, &recipe, &mut used, &bounds) {
                Ok(drawn) => drawn,
                Err(why) => {
                    board.stopped = Some(format!("V{number} wasn't planned: {why}"));
                    break;
                }
            };
            let line = take_line(&drawn, "candidate; not written or heard");
            board.states.insert(number, State::Planned);
            board.set.takes.push(Take {
                number,
                state: TakeState::Shown,
                place: Value::Null,
                change: None,
                direction: drawn.direction,
                digest: self.backend.digest(&drawn.material),
                material: drawn.material,
                previous: vec![],
                line,
            });
            self.numbered.borrow_mut().insert(board.set.source.short.clone(), number);
            self.save()?;
            self.save_board(&mut board)?;
            // Give other jobs and the foreground a chance to run between bounded pure draws.
            tokio::task::yield_now().await;
        }
        self.save_board(&mut board)?;
        Ok(self.board_view(&board))
    }

    fn board_file(&self, project: &str, id: &str) -> Result<PathBuf, String> {
        if !uuid::Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id) {
            return Err("A board id must be its canonical UUID.".into());
        }
        let project = hex::encode(Sha256::digest(project.as_bytes()));
        Ok(self
            .home
            .join("variations")
            .join("boards")
            .join(self.backend.words().app.to_ascii_lowercase())
            .join(project)
            .join(format!("{id}.sqlite3")))
    }

    fn board_json(&self, board: &Board<B>) -> Value {
        let set = &board.set;
        let takes = set.takes.iter().map(|take| json!({
            "number":take.number,"state":"shown","place":take.place,"change":take.change,
            "direction":take.direction,"digest":take.digest,"line":take.line,"material":self.backend.material_json(&take.material),"previous":[]
        })).collect::<Vec<_>>();
        json!({"version":1,"id":board.id,"project":board.project,"states":board.states,"stopped":board.stopped,
            "set":{"id":0,"seed":set.seed,"floor":set.floor,"label":set.source.label,"short":set.source.short,
                "key":set.source.key,"data":self.backend.data_json(&set.source.data),
                "material":set.source.material.as_ref().map(|material|self.backend.material_json(material)),"takes":takes}})
    }

    fn save_board(&self, board: &mut Board<B>) -> Result<(), String> {
        let path = self.board_file(&board.project, &board.id)?;
        let text = serde_json::to_string(&self.board_json(board)).map_err(|error| error.to_string())?;
        if i64::try_from(text.len()).map_err(|_| "The board size cannot be represented.")? > BOARD_BYTES {
            return Err("The candidate board exceeds its 8 MiB storage bound.".into());
        }
        std::fs::create_dir_all(path.parent().ok_or("The board folder is unavailable.")?).map_err(|error| error.to_string())?;
        let db = Connection::open(path).map_err(|error| error.to_string())?;
        db.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=EXTRA; CREATE TABLE IF NOT EXISTS board (id INTEGER PRIMARY KEY CHECK(id=1), revision INTEGER NOT NULL CHECK(revision>0), payload TEXT NOT NULL);")
            .map_err(|error|error.to_string())?;
        // EXTRA in rollback-journal mode syncs the directory after journal deletion. CAS refuses a stale writer.
        let next_revision =
            board.revision.checked_add(1).filter(|revision| *revision > 0).ok_or("The board revision is invalid or exhausted.")?;
        let changed = if board.revision == 0 {
            db.execute("INSERT OR IGNORE INTO board (id,revision,payload) VALUES (1,1,?1)", params![text])
        } else {
            db.execute("UPDATE board SET revision=?2,payload=?1 WHERE id=1 AND revision=?3", params![text, next_revision, board.revision])
        }
        .map_err(|error| error.to_string())?;
        if changed != 1 {
            return Err("The board changed in another request; inspect it before doing more. No further candidate was applied.".into());
        }
        board.revision = next_revision;
        Ok(())
    }

    fn read_board(&self, project: &str, id: &str) -> Result<Board<B>, String> {
        let path = self.board_file(project, id)?;
        if !path.is_file() {
            return Err("That board isn't in this project.".into());
        }
        let db = Connection::open(path).map_err(|error| error.to_string())?;
        let (revision, text): (i64, String) = db
            .query_row("SELECT revision,payload FROM board WHERE id=1 AND length(CAST(payload AS BLOB))<=?1", params![BOARD_BYTES], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()
            .map_err(|error| error.to_string())?
            .ok_or("The board is absent or exceeds its storage bound.")?;
        if revision < 1 || revision.checked_add(1).is_none() {
            return Err("The board revision is invalid or exhausted.".into());
        }
        let value: Value = serde_json::from_str(&text).map_err(|_| "The board file is unreadable.".to_string())?;
        if value["version"] != 1 || value["id"] != id || value["project"] != project {
            return Err("The board identity doesn't match this project.".into());
        }
        let set = self.sets_of(&json!({"sets":[value["set"].clone()]})).into_iter().next().ok_or("The board material is incomplete.")?;
        let mut states: BTreeMap<usize, State> =
            serde_json::from_value(value["states"].clone()).map_err(|_| "The board states are incomplete.")?;
        if set.takes.len() > MOST_TAKES
            || states.len() != set.takes.len()
            || set
                .takes
                .iter()
                .any(|take| take.number == 0 || !states.contains_key(&take.number) || self.backend.digest(&take.material) != take.digest)
        {
            return Err("The board's candidate identities or material are incomplete.".into());
        }
        // An app crash may have interrupted the native reply. Never re-apply an unconfirmed write.
        for state in states.values_mut() {
            if *state == State::Applying {
                *state = State::Unsure;
            }
        }
        Ok(Board { id: id.into(), project: project.into(), set, states, stopped: value["stopped"].as_str().map(str::to_owned), revision })
    }

    fn board_view(&self, board: &Board<B>) -> Value {
        let takes = board.set.takes.iter().map(|take| json!({
            "id":format!("{}:V{}",board.id,take.number),"name":format!("V{}",take.number),"state":board.states[&take.number],
            "digest":take.digest,"material":self.backend.material_json(&take.material),"line":take.line,"place":take.place,"change":take.change
        })).collect::<Vec<_>>();
        let actions = if self.backend.can_apply_candidates() {
            "list; keep/drop unwritten candidates locally; explicitly apply named candidates with atomic source and destination guards. Applied or unsure candidates are never replayed or automatically deleted."
        } else {
            "list; keep/drop candidates locally; review note material manually. Applying candidate boards in this DAW is unavailable."
        };
        json!({"board":board.id,"source":board.set.source.label,"takes":takes,"stopped":board.stopped,
            "audio":null,"actions":actions})
    }

    pub(super) async fn board_action(&self, input: &JsonObject, signal: &Signal) -> Result<Value, String> {
        let project = self.loaded.borrow().clone().ok_or("Observe the original project before inspecting a board.")?;
        let id = input.get("board").and_then(Value::as_str).ok_or("board must be its immutable UUID.")?;
        let mut board = self.read_board(&project, id)?;
        let action = input.get("action").and_then(Value::as_str).unwrap_or("");
        if action == "apply" && !self.backend.can_apply_candidates() {
            return Err("Applying candidate boards in this DAW is unavailable: it has no atomic native source and destination guard. Review the candidate note material manually; nothing was applied.".into());
        }
        if action == "list" {
            return Ok(self.board_view(&board));
        }
        if !["keep", "drop", "apply"].contains(&action) {
            return Err("A candidate board supports list, keep, drop or explicit foreground apply.".into());
        }
        let names = names_of(input, &[]);
        if names.is_empty()
            || (action == "apply" && names.iter().any(|name| name.trim().trim_start_matches(['V', 'v']).parse::<usize>().is_err()))
        {
            return Err("Name candidates explicitly, such as names: [V1, V3]; apply never means all/rest.".into());
        }
        let numbers = board.set.pick(&names, Purpose::Keep, false)?;
        if numbers.iter().any(|number| !matches!(board.states[number], State::Planned | State::Kept)) {
            return Err("Only unwritten planned/kept candidates can be selected, discarded or applied. Inspect Live for applied/unsure entries; they are never replayed or deleted by this board.".into());
        }
        if action != "apply" {
            for number in numbers {
                board.states.insert(number, if action == "keep" { State::Kept } else { State::Dropped });
            }
            self.save_board(&mut board)?;
            return Ok(self.board_view(&board));
        }
        signal.check().map_err(|error| error.to_string())?;
        self.backend.verify_source(&board.set.source, signal).await?;
        board.stopped = None;
        for number in numbers {
            if signal.is_cancelled() {
                board.stopped = Some(format!("Cancelled before V{number}; completed writes remain on this board."));
                self.save_board(&mut board)?;
                break;
            }
            if let Err(why) = self.backend.verify_source(&board.set.source, signal).await {
                board.stopped = Some(format!("Stopped before V{number}: {why}. Completed writes remain on this board."));
                self.save_board(&mut board)?;
                break;
            }
            board.states.insert(number, State::Applying);
            self.save_board(&mut board)?; // The pending marker must exist before a native write can start.
            let material = board.set.take(number).ok_or("The candidate is missing.")?.material.clone();
            match self.backend.write_candidate(&board.set, number, &material, signal).await {
                Ok((change, place, _)) => {
                    let take = board.set.take_mut(number).ok_or("The candidate is missing.")?;
                    take.place = place;
                    take.change = Some(change);
                    board.states.insert(number, State::Applied);
                }
                Err((why, _)) => {
                    board.states.insert(number, State::Unsure);
                    board.stopped =
                        Some(format!("V{number}: {why}. Its write was not confirmed; inspect Live and history, never replay it."));
                    self.save_board(&mut board)?;
                    break;
                }
            }
            self.save_board(&mut board)?;
        }
        Ok(self.board_view(&board))
    }
}
