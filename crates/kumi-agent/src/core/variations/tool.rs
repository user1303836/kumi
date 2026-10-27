//! The variations tool (M7), the same in every DAW: make takes of a source, then keep, change and drop them by number. Core
//! draws and checks each take, keeps the sets (in Kumi's folder, by the project's own id, so a restart forgets nothing kept),
//! renders and hears takes before they're shown, and answers a line per take; the DAW's backend reads, writes, finds and renders.
use super::backend::{found, gone, Recipe, Rewritten, Set, Source, Undroppable, VariationsBackend, Words};
use super::heard::{db_text, Fault, Heard};
use super::{
    bolder_or_milder, check, kept_name, roll, seed_of, take_name, Bounds, Material, Purpose, Rng, Take, TakeSet, TakeState, TRIES,
};
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_common::js::json::stringify;
use kumi_types::contracts::{JsonObject, KernelTool, Reach, ToolResult};
use kumi_types::errors::RuntimeError;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::Rc;

/// The most takes one request makes or changes, and the sets Kumi keeps for a project.
const MOST_TAKES: usize = 16;
const SETS_KEPT: usize = 8;

/// A take drawn, before it's written: its number, material, direction, what was done (and warnings) and its distance.
pub struct Drawn<M> {
    pub number: usize,
    pub material: M,
    pub direction: String,
    pub said: Vec<String>,
    pub distance: Option<f64>,
}

/// The variations of the project the DAW has open, with the DAW's backend.
pub struct Variations<B: VariationsBackend> {
    backend: B,
    home: PathBuf,
    sets: RefCell<Vec<Set<B>>>,
    /// The highest number given to takes of each name (a set's short), kept past the sets Kumi lets go of: a take's name never
    /// comes back while one of that name may still be in the project (#400 review notes).
    numbered: RefCell<BTreeMap<String, usize>>,
    /// The project whose sets are loaded.
    loaded: RefCell<Option<String>>,
    description: String,
    schema: JsonObject,
}

impl<B: VariationsBackend + 'static> Variations<B> {
    pub fn new(backend: B, home: PathBuf) -> Rc<Self> {
        let words = backend.words();
        Rc::new(Variations {
            description: description(words),
            schema: schema(words),
            backend,
            home,
            sets: RefCell::default(),
            numbered: RefCell::default(),
            loaded: RefCell::default(),
        })
    }

    pub fn tool(self: &Rc<Self>) -> Rc<dyn KernelTool> {
        Rc::new(VariationsTool { owner: self.clone() })
    }

    async fn run(&self, input: JsonObject, signal: Signal) -> Result<Value, String> {
        let action = input.get("action").and_then(Value::as_str).unwrap_or("");
        // (the project's sets, as Kumi kept them: a restart of Kumi forgets nothing kept)
        let project = self.backend.project_key(&signal).await;
        self.load(project.as_deref());
        let answer = match action {
            "make" => self.make(&input, &signal).await,
            "change" => self.change(&input, &signal).await,
            "keep" => self.keep(&input, &signal).await,
            "drop" => self.drop(&input, &signal).await,
            "extract" if self.backend.words().extract.is_some() => return self.backend.extract(&input, &signal).await,
            "list" => return self.list(&input),
            _ => return Err(format!("action is {}", actions(self.backend.words()).join(", "))),
        };
        self.save();
        answer
    }

    fn list(&self, input: &JsonObject) -> Result<Value, String> {
        let sets = self.sets.borrow();
        let set = pick_set(&sets, input)?;
        let mut reply = json!({"set": set.id, "source": set.source.label, "takes": set.lines()});
        if let Some(note) = self.backend.end_note(set) {
            reply["note"] = json!(note);
        }
        let others: Vec<String> =
            sets.iter().filter(|other| other.id != set.id).map(|other| format!("set {}: {}", other.id, other.source.label)).collect();
        if !others.is_empty() {
            reply["otherSets"] = json!(others);
        }
        Ok(reply)
    }

    // Keeping the sets

    fn store_path(&self, project: &str) -> PathBuf {
        let safe: String = project.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).take(80).collect();
        self.home.join("variations").join(format!("{}-{safe}.json", self.backend.words().app.to_ascii_lowercase()))
    }

    /// The sets Kumi kept for this project (another project's are put away first).
    fn load(&self, project: Option<&str>) {
        if self.loaded.borrow().as_deref() == project {
            return;
        }
        *self.loaded.borrow_mut() = project.map(str::to_owned);
        let kept = project
            .and_then(|project| std::fs::read_to_string(self.store_path(project)).ok())
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        *self.numbered.borrow_mut() = kept
            .as_ref()
            .and_then(|value| value.get("numbered"))
            .and_then(Value::as_object)
            .map(|numbered| numbered.iter().filter_map(|(name, n)| Some((name.clone(), n.as_u64()? as usize))).collect())
            .unwrap_or_default();
        *self.sets.borrow_mut() = kept.map(|value| self.sets_of(&value)).unwrap_or_default();
    }

    /// The project's sets in Kumi's folder, written whole and then moved into place.
    fn save(&self) {
        let Some(project) = self.loaded.borrow().clone() else { return };
        let path = self.store_path(&project);
        let text = stringify(&self.json_of());
        if let Some(folder) = path.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let partial = path.with_extension("json.partial");
        if std::fs::write(&partial, text).is_ok() {
            let _ = std::fs::rename(&partial, &path);
        }
    }

    fn json_of(&self) -> Value {
        let backend = &self.backend;
        let sets: Vec<Value> = self
            .sets
            .borrow()
            .iter()
            .map(|set| {
                let takes: Vec<Value> = set
                    .takes
                    .iter()
                    .map(|take| {
                        json!({"number": take.number, "state": take.state.word(), "place": take.place, "change": take.change,
                            "direction": take.direction, "digest": take.digest, "line": take.line, "material": backend.material_json(&take.material)})
                    })
                    .collect();
                json!({"id": set.id, "seed": set.seed, "floor": set.floor, "label": set.source.label, "short": set.source.short,
                    "key": set.source.key, "data": backend.data_json(&set.source.data),
                    "material": set.source.material.as_ref().map(|material| backend.material_json(material)), "takes": takes})
            })
            .collect();
        json!({"version": 1, "sets": sets, "numbered": *self.numbered.borrow()})
    }

    fn sets_of(&self, value: &Value) -> Vec<Set<B>> {
        let backend = &self.backend;
        let text = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        value
            .get("sets")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|kept| {
                let data = backend.data_of(kept.get("data")?)?;
                let material = kept.get("material").filter(|value| !value.is_null()).and_then(|value| backend.material_of(value));
                let source = Source { label: text(kept, "label"), short: text(kept, "short"), key: text(kept, "key"), material, data };
                let mut set: Set<B> = TakeSet::new(kept.get("id")?.as_u64()? as usize, source, kept.get("seed")?.as_u64()?);
                set.floor = kept.get("floor").and_then(Value::as_u64).unwrap_or(0) as usize;
                for take in kept.get("takes").and_then(Value::as_array).into_iter().flatten() {
                    let state = match take.get("state").and_then(Value::as_str) {
                        Some("kept") => TakeState::Kept,
                        Some("dropped") => TakeState::Dropped,
                        _ => TakeState::Shown,
                    };
                    let Some(material) = take.get("material").and_then(|value| backend.material_of(value)) else { continue };
                    set.takes.push(Take {
                        number: take.get("number").and_then(Value::as_u64).unwrap_or(0) as usize,
                        state,
                        place: take.get("place").cloned().unwrap_or(Value::Null),
                        change: take.get("change").and_then(Value::as_str).map(str::to_owned),
                        direction: text(take, "direction"),
                        digest: text(take, "digest"),
                        material,
                        line: text(take, "line"),
                    });
                }
                Some(set)
            })
            .collect()
    }

    // The actions

    async fn make(&self, input: &JsonObject, signal: &Signal) -> Result<Value, String> {
        let source = self.backend.source(input, signal).await?;
        let recipes = self.recipes_of(input, &source)?;
        let bounds = bounds_of(input);
        let seed = input.get("seed").and_then(Value::as_u64).unwrap_or_else(fresh_seed);
        // (the same source again, by its places now: its set goes on, V10 after V9; another source's set stays, by its number or
        // name)
        let candidates: Vec<Set<B>> = self.sets.borrow().iter().rev().filter(|set| set.source.short == source.short).cloned().collect();
        let mut same = None;
        for set in candidates {
            let key = self.backend.key_now(&set.source, signal).await.unwrap_or_else(|| set.source.key.clone());
            if key == source.key {
                same = Some(set);
                break;
            }
        }
        let (mut set, continued) = {
            let sets = self.sets.borrow();
            match same {
                Some(mut set) => {
                    // (the source as it is now, its lines, anchors and settings; its takes and their places stay)
                    set.source = source;
                    (set, true)
                }
                None => {
                    let id = sets.iter().map(|set| set.id).max().unwrap_or(0) + 1;
                    // (numbers once given to takes of the same name never come back: names stay apart in the DAW)
                    let floor = sets.iter().filter(|other| other.source.short == source.short).map(|other| other.next_number() - 1).max();
                    let given = self.numbered.borrow().get(&source.short).copied().unwrap_or(0);
                    let mut set: Set<B> = TakeSet::new(id, source, seed);
                    set.floor = floor.unwrap_or(0).max(given);
                    (set, false)
                }
            }
        };
        set.seed = seed;
        let mut refs = BTreeMap::new();
        let mut written: BTreeMap<usize, Recipe> = BTreeMap::new();
        let mut skipped = Vec::new();
        let mut used: Vec<String> = recipes.iter().map(|recipe| recipe.direction.clone()).collect();
        for mut recipe in recipes {
            let number = set.next_number();
            // (takes of a source with nothing in it that Kumi picks vary the first one written this time)
            if recipe.picked && recipe.from.is_none() && set.source.material.is_none() {
                recipe.from = written
                    .keys()
                    .next()
                    .copied()
                    .or_else(|| set.takes.iter().find(|take| take.state != TakeState::Dropped).map(|take| take.number));
            }
            let (drawn, recipe) = match self.draw_picking(&set, number, &recipe, &mut used, &bounds) {
                Ok(drawn) => drawn,
                Err(why) => {
                    skipped.push(format!("V{number} ({}) wasn't made: {why}", direction_of(&recipe)));
                    continue;
                }
            };
            match self.backend.write_new(&set, number, &drawn.material, signal).await {
                Ok((change, place, reference)) => {
                    let line = take_line(&drawn, &change);
                    refs.insert(number, self.backend.short(&reference));
                    written.insert(number, recipe.clone());
                    set.takes.push(Take {
                        number,
                        state: TakeState::Shown,
                        place,
                        change: Some(change),
                        direction: drawn.direction.clone(),
                        digest: self.backend.digest(&drawn.material),
                        material: drawn.material,
                        line,
                    });
                }
                Err((why, stops)) => {
                    skipped.push(format!("V{number} ({}) wasn't made: {why}", direction_of(&recipe)));
                    if stops {
                        break;
                    }
                }
            }
        }
        let numbers: Vec<usize> = written.keys().copied().collect();
        let heard = self.listen(&mut set, &numbers, &written, &bounds, input, signal).await;
        let answer_lines: Vec<Value> = numbers
            .iter()
            .filter_map(|n| set.take(*n))
            .map(|take| json!({"take": format!("V{}", take.number), "ref": refs[&take.number], "line": take.line}))
            .collect();
        let mut reply = json!({"set": set.id, "source": set.source.label, "takes": answer_lines, "skipped": skipped});
        if !heard.is_empty() {
            reply["heard"] = json!(heard);
        }
        if let Some(note) = self.backend.end_note(&set).filter(|_| !numbers.is_empty()) {
            reply["note"] = json!(note);
        }
        if !numbers.is_empty() || continued {
            let mut numbered = self.numbered.borrow_mut();
            let given = numbered.entry(set.source.short.clone()).or_insert(0);
            *given = (*given).max(set.next_number() - 1);
            let mut sets = self.sets.borrow_mut();
            sets.retain(|held| held.id != set.id);
            sets.push(set);
            let excess = sets.len().saturating_sub(SETS_KEPT);
            sets.drain(..excess);
        }
        Ok(reply)
    }

    async fn change(&self, input: &JsonObject, signal: &Signal) -> Result<Value, String> {
        let force = input.get("force") == Some(&Value::Bool(true));
        let names = names_of(input, &[]);
        if names.is_empty() {
            return Err("change needs names: the takes to change (V6, or rest)".into());
        }
        let (numbers, mut set) = {
            let sets = self.sets.borrow();
            let set = pick_set(&sets, input)?;
            (set.pick(&names, Purpose::Change, force)?, set.clone())
        };
        let given = self.given(input, &set.source)?;
        if given.len() > numbers.len() || numbers.len() > MOST_TAKES {
            return Err(format!(
                "change names {} takes and gives {} recipes: give one per take, or fewer and Kumi picks the rest",
                numbers.len(),
                given.len()
            ));
        }
        // (the directions the model left to Kumi: distinct from the ones it gave and from each other)
        let words = self.backend.words();
        let directions = self.backend.directions(&set.source);
        let mut used: Vec<String> = given.iter().map(|recipe| recipe.direction.clone()).collect();
        let mut recipes = given;
        while recipes.len() < numbers.len() {
            let direction = directions.iter().find(|d| !used.iter().any(|u| u == *d)).copied().unwrap_or(directions[0]);
            used.push(direction.into());
            recipes.push(self.of_direction(&set.source, direction));
        }
        let bounds = bounds_of(input);
        let rows = self.backend.find(&mut set, &numbers, signal).await?;
        let mut lines_out = Vec::new();
        let mut skipped = Vec::new();
        let mut rewritten = BTreeMap::new();
        for ((number, mut recipe), row) in numbers.iter().copied().zip(recipes).zip(rows) {
            let take = set.take(number).expect("a picked take").clone();
            if !found(&row) {
                skipped.push(format!("V{number} isn't in the {} any more", self.backend.words().app));
                continue;
            }
            // (a take that can't be read now is left: the takes rewritten before it stay told)
            let current = match self.backend.current(&set, &row, signal).await {
                Ok(current) => current,
                Err(why) => {
                    skipped.push(format!("V{number} wasn't changed: Kumi couldn't read it ({why})"));
                    continue;
                }
            };
            if self.backend.digest(&current) != take.digest {
                skipped.push(format!("V{number} was edited in {} since Kumi wrote it: left as it is", words.app));
                continue;
            }
            // (a take of a source with nothing in it varies itself)
            if recipe.from.is_none() && set.source.material.is_none() {
                recipe.from = Some(number);
            }
            let (drawn, recipe) = match self.draw_picking(&set, number, &recipe, &mut used, &bounds) {
                Ok(drawn) => drawn,
                Err(why) => {
                    skipped.push(format!("V{number} wasn't changed: {why}"));
                    continue;
                }
            };
            match self.backend.rewrite(&set, &row, &current, &drawn.material, signal).await {
                Ok(done) => {
                    rewritten.insert(number, recipe);
                    let (change, material, line) = holding(&drawn, done);
                    let reference = row.get("ref").and_then(Value::as_str).unwrap_or("");
                    lines_out.push(json!({"take": format!("V{number}"), "ref": self.backend.short(reference), "line": line}));
                    if let Some(take) = set.take_mut(number) {
                        take.digest = self.backend.digest(&material);
                        take.material = material;
                        take.direction = drawn.direction;
                        take.change = Some(change);
                        take.line = line;
                    }
                }
                Err((why, stops)) => {
                    skipped.push(format!("V{number} wasn't changed: {why}"));
                    if stops {
                        break;
                    }
                }
            }
        }
        let changed: Vec<usize> = rewritten.keys().copied().collect();
        let heard = self.listen(&mut set, &changed, &rewritten, &bounds, input, signal).await;
        for line in &mut lines_out {
            let number: Option<usize> = line["take"].as_str().and_then(|take| take.trim_start_matches('V').parse().ok());
            if let Some(take) = number.and_then(|n| set.take(n)) {
                line["line"] = json!(take.line);
            }
        }
        let mut reply = json!({"set": set.id, "changed": lines_out, "skipped": skipped, "takes": set.lines()});
        if !heard.is_empty() {
            reply["heard"] = json!(heard);
        }
        self.put_back(set);
        Ok(reply)
    }

    /// Marks takes kept: each is renamed "V3 · … · kept" in the DAW (one change each), so the mark is there for the producer and
    /// for Kumi after a restart; one the producer renamed keeps its name. `at` places one take too.
    async fn keep(&self, input: &JsonObject, signal: &Signal) -> Result<Value, String> {
        let names = names_of(input, &[]);
        if names.is_empty() {
            return Err("keep needs names: the takes the producer keeps (V3)".into());
        }
        let (numbers, mut set) = {
            let sets = self.sets.borrow();
            let set = pick_set(&sets, input)?;
            (set.pick(&names, Purpose::Keep, false)?, set.clone())
        };
        let mut placed = Value::Null;
        if let Some(at) = input.get("at").and_then(Value::as_object) {
            let [number] = numbers.as_slice() else { return Err("at places one take: keep the others without at".into()) };
            let row = self.backend.find(&mut set, &[*number], signal).await?.into_iter().next().unwrap_or(Value::Null);
            if !found(&row) {
                return Err(format!("V{number} isn't in the {} any more", self.backend.words().app));
            }
            placed = self.backend.place(&mut set, *number, &row, at, signal).await?;
        }
        let rows = self.backend.find(&mut set, &numbers, signal).await?;
        let mut renamed = Vec::new();
        let mut notes = Vec::new();
        for (number, row) in numbers.iter().copied().zip(rows) {
            if !found(&row) {
                notes.push(format!("V{number} isn't in the {} any more", self.backend.words().app));
                continue;
            }
            if let Some(take) = set.take_mut(number) {
                take.state = TakeState::Kept;
            }
            match self.backend.mark_kept(&set, number, &row, signal).await {
                Ok(Some(change)) => renamed.push(format!("V{number} ({change})")),
                Ok(None) => {}
                Err(why) => notes.push(format!("V{number} is kept, but its mark wasn't written: {why}")),
            }
        }
        let rest: Vec<String> =
            set.takes.iter().filter(|take| take.state == TakeState::Shown).map(|take| format!("V{}", take.number)).collect();
        let mut reply = json!({"set": set.id, "kept": numbers.iter().map(|n| format!("V{n}")).collect::<Vec<_>>(), "renamed": renamed,
            "placed": placed, "rest": rest, "takes": set.lines()});
        if !notes.is_empty() {
            reply["notes"] = json!(notes);
        }
        self.put_back(set);
        Ok(reply)
    }

    async fn drop(&self, input: &JsonObject, signal: &Signal) -> Result<Value, String> {
        let force = input.get("force") == Some(&Value::Bool(true));
        let names = names_of(input, &["rest"]);
        let (numbers, mut set) = {
            let sets = self.sets.borrow();
            let set = pick_set(&sets, input)?;
            (set.pick(&names, Purpose::Drop, force)?, set.clone())
        };
        let app = self.backend.words().app;
        // (the takes named one by one: an edit since Kumi wrote one doesn't keep it, as it does from "rest")
        let named: BTreeSet<usize> =
            names.iter().filter_map(|name| name.trim().to_ascii_lowercase().trim_start_matches('v').parse().ok()).collect();
        let rows = self.backend.find(&mut set, &numbers, signal).await?;
        let mut dropping = Vec::new();
        let mut skipped = Vec::new();
        let mut edited = Vec::new();
        for (number, row) in numbers.iter().copied().zip(rows) {
            if gone(&row) {
                skipped.push(format!("V{number} isn't in the {app} any more"));
                if let Some(take) = set.take_mut(number) {
                    take.state = TakeState::Dropped;
                }
                continue;
            }
            if !found(&row) {
                skipped.push(format!("Kumi can't find V{number} where it left it, nor by its name: left as it is"));
                continue;
            }
            let current = self.backend.current(&set, &row, signal).await?;
            if self.backend.digest(&current) != set.take(number).expect("a picked take").digest {
                if !named.contains(&number) {
                    skipped.push(format!("V{number} was edited in {app} since Kumi wrote it: left as it is (name it to drop it anyway)"));
                    continue;
                }
                edited.push(format!("V{number}"));
            }
            match self.backend.undroppable(&set, number, &row, signal).await {
                Some(Undroppable::InUse(why)) => {
                    skipped.push(format!("V{number} {why}: left as it is"));
                    continue;
                }
                Some(Undroppable::Edited(why)) if !named.contains(&number) => {
                    skipped.push(format!("V{number} {why}: left as it is (name it to drop it anyway)"));
                    continue;
                }
                Some(Undroppable::Edited(_)) if !edited.contains(&format!("V{number}")) => edited.push(format!("V{number}")),
                _ => {}
            }
            dropping.push((number, row));
        }
        let mut change = Value::Null;
        if !dropping.is_empty() {
            let (id, applied) = self.backend.drop(&set, &dropping, signal).await?;
            for (number, _) in &dropping {
                if let Some(take) = set.take_mut(*number) {
                    take.state = TakeState::Dropped;
                }
            }
            change = json!({"change": id, "title": applied.pointer("/display/title").cloned().unwrap_or(Value::Null),
                "note": applied.pointer("/display/note").cloned().unwrap_or(Value::Null)});
        }
        let mut reply = json!({"set": set.id, "dropped": dropping.iter().map(|(n, _)| format!("V{n}")).collect::<Vec<_>>(), "in": change,
            "skipped": skipped, "takes": set.lines()});
        if !edited.is_empty() {
            reply["edited"] = json!(edited);
        }
        self.put_back(set);
        Ok(reply)
    }

    // Drawing

    fn given(&self, input: &JsonObject, source: &Source<B::Material, B::Data>) -> Result<Vec<Recipe>, String> {
        let recipes: Vec<Recipe> =
            input.get("takes").and_then(Value::as_array).into_iter().flatten().map(parse_recipe).collect::<Result<_, _>>()?;
        for recipe in &recipes {
            self.backend.check_ops(source, &recipe.ops)?;
        }
        Ok(recipes)
    }

    fn of_direction(&self, source: &Source<B::Material, B::Data>, direction: &str) -> Recipe {
        Recipe { direction: direction.into(), ops: self.backend.direction_ops(source, direction), lines: None, from: None, picked: true }
    }

    /// The recipes a make asks for: the model's, then (to `count`) directions Kumi picks that none of them has.
    fn recipes_of(&self, input: &JsonObject, source: &Source<B::Material, B::Data>) -> Result<Vec<Recipe>, String> {
        let has_source = source.material.is_some();
        let given = self.given(input, source)?;
        let count = input.get("count").and_then(Value::as_u64).map(|n| n as usize);
        if given.is_empty() && !has_source {
            return Err("takes of a source with nothing in it need lines you write (later takes can start from: \"V1\")".into());
        }
        let wanted = count.unwrap_or(if given.is_empty() { 4 } else { given.len() });
        if wanted > MOST_TAKES {
            return Err(format!("at most {MOST_TAKES} takes at once"));
        }
        let directions = self.backend.directions(source);
        let mut recipes = given;
        let used: Vec<String> = recipes.iter().map(|recipe| recipe.direction.clone()).collect();
        let mut free = directions.iter().filter(|d| !used.iter().any(|u| u == *d));
        while recipes.len() < wanted {
            let direction = free.next().copied().unwrap_or(directions[0]);
            recipes.push(self.of_direction(source, direction));
        }
        Ok(recipes)
    }

    /// Draws take `number` by its recipe; one whose direction Kumi picked and that can't pass gives way to the next direction no
    /// take of the request has (`used`). The take and the recipe that made it.
    fn draw_picking(
        &self,
        set: &Set<B>,
        number: usize,
        recipe: &Recipe,
        used: &mut Vec<String>,
        bounds: &Bounds,
    ) -> Result<(Drawn<B::Material>, Recipe), String> {
        let first = self.draw(set, number, recipe, bounds);
        if first.is_ok() || !recipe.picked {
            return first.map(|drawn| (drawn, recipe.clone()));
        }
        let mut last = first.map(|drawn| (drawn, recipe.clone()));
        let free: Vec<&str> = self.backend.directions(&set.source).iter().copied().filter(|d| !used.iter().any(|u| u == d)).collect();
        for direction in free {
            used.push(direction.to_string());
            let mut other = self.of_direction(&set.source, direction);
            other.from = recipe.from;
            last = self.draw(set, number, &other, bounds).map(|drawn| (drawn, other));
            if last.is_ok() {
                break;
            }
        }
        last
    }

    /// Draws take `number` by its recipe and checks it: different from the source and the other takes (its old self too, for a
    /// change), the same idea, the anchors, the key and the grid kept; a draw that fails is drawn again, bolder or milder. Lines
    /// the model wrote are drawn once: close to the source or another take, they're written with a warning.
    fn draw(&self, set: &Set<B>, number: usize, recipe: &Recipe, bounds: &Bounds) -> Result<Drawn<B::Material>, String> {
        let backend = &self.backend;
        let source = set.source.material.as_ref();
        let base: B::Material = match recipe.from {
            Some(from) => set
                .take(from)
                .filter(|take| take.state != TakeState::Dropped)
                .map(|take| take.material.clone())
                .ok_or(format!("from: there is no V{from} to start from"))?,
            None => source.cloned().unwrap_or_else(|| backend.empty(set)),
        };
        let others_owned: Vec<(usize, B::Material)> =
            set.living_except(&[]).into_iter().map(|take| (take.number, take.material.clone())).collect();
        let others: Vec<(usize, &B::Material)> = others_owned.iter().map(|(n, material)| (*n, material)).collect();
        let written_only = recipe.ops.is_empty() && recipe.lines.is_some();
        let rolling = if written_only { Bounds { min_source: 0., max_source: 1., min_others: 0. } } else { *bounds };
        let tries = if recipe.ops.is_empty() { 1 } else { TRIES };
        let said: RefCell<Vec<String>> = RefCell::default();
        let warnings: RefCell<Vec<String>> = RefCell::default();
        // (a try too close to the source or a take draws bolder next time, one too far milder)
        let factor = Cell::new(1.0f64);
        let rolled = roll(set.seed, number, tries, source, &others, &rolling, |seed, before| {
            factor.set(bolder_or_milder(factor.get(), before));
            let mut take = match &recipe.lines {
                Some(lines) => backend.written(set, lines, &base)?,
                None => base.clone(),
            };
            let mut rng = Rng::new(seed);
            let mut words = Vec::new();
            if recipe.lines.is_some() {
                words.push("lines as written".to_string());
            }
            words.extend(backend.apply_ops(&recipe.ops, factor.get(), &mut take, source.unwrap_or(&base), &mut rng)?);
            if let Some(source) = source {
                backend.anchors_kept(source, &take)?;
            }
            let issues = backend.judge(source.or(Some(&base)), &take, &recipe.ops, factor.get());
            if !issues.is_empty() {
                if recipe.ops.is_empty() {
                    *warnings.borrow_mut() = issues;
                } else {
                    return Err(issues.join("; "));
                }
            }
            *said.borrow_mut() = words;
            Ok(take)
        });
        let take = rolled.take.filter(|_| rolled.verdict.is_fine()).ok_or_else(|| {
            let tried = if rolled.tries > 1 { format!(" (after {} tries)", rolled.tries) } else { String::new() };
            format!("{}{tried}", rolled.verdict.words())
        })?;
        let mut words = said.into_inner();
        if rolled.tries > 1 {
            words.push(format!("re-rolled {} times", rolled.tries - 1));
        }
        // (lines the model wrote: what the checks would have said, as a warning)
        if written_only {
            let strict = check(&take, source, &others, bounds);
            if !strict.is_fine() {
                words.push(format!("written as asked, though {}", strict.words()));
            }
        }
        words.extend(warnings.into_inner().into_iter().map(|issue| format!("warning: {issue}")));
        let distance = source.map(|source| take.distance(source));
        Ok(Drawn { number, material: take, direction: direction_of(recipe).to_string(), said: words, distance })
    }

    // Listening

    /// Renders the takes offline, with the source once, and hears them. A take that clips where the source doesn't is turned
    /// down to the source's peak, otherwise as it was (#407); one that is silent, or clips in a DAW that can't turn it down, is
    /// drawn again (lines the model wrote are only said). Either is one more change, and is heard again (up to twice). Each
    /// take's line says how it sounded and how much of the source's rhythm it keeps (its family). Never while the DAW plays: a
    /// render stops playback. Answers what to tell.
    async fn listen(
        &self,
        set: &mut Set<B>,
        numbers: &[usize],
        recipes: &BTreeMap<usize, Recipe>,
        bounds: &Bounds,
        input: &JsonObject,
        signal: &Signal,
    ) -> Vec<String> {
        let app = self.backend.words().app;
        if numbers.is_empty() || input.get("listen") == Some(&Value::Bool(false)) {
            return vec![];
        }
        if let Some(why) = self.backend.cant_render() {
            return vec![format!("The takes weren't rendered: {why}")];
        }
        match self.backend.playing(signal).await {
            Some(false) => {}
            Some(true) => {
                return vec![format!(
                    "{app} is playing, and a render would stop it: the takes weren't rendered (they're checked on their lines). Ask to hear them once it's stopped."
                )]
            }
            None => {
                return vec![format!(
                    "Kumi couldn't tell whether {app} is playing, and a render would stop it: the takes weren't rendered (they're checked on their lines). Ask to hear them."
                )]
            }
        }
        let mut notes = Vec::new();
        let mut pending: Vec<usize> = numbers.to_vec();
        let mut source: Option<Option<Heard>> = None;
        for round in 0..3u32 {
            // (a later round renders the takes drawn again or turned down: the producer may have pressed play meanwhile)
            if round > 0 && self.backend.playing(signal).await != Some(false) {
                notes.push(format!(
                    "{app} plays now (or couldn't say whether it does): the takes drawn again or turned down weren't rendered again. Ask to hear them once it's stopped."
                ));
                break;
            }
            let (heard_source, heard) = match self.backend.render(set, &pending, source.is_none(), signal).await {
                Ok(rendered) => rendered,
                Err(why) => {
                    notes.push(format!("The takes weren't rendered: {why}"));
                    break;
                }
            };
            if source.is_none() {
                source = Some(heard_source);
            }
            let source = source.clone().flatten();
            let mut again = Vec::new();
            for number in pending.iter().copied() {
                let heard = match heard.get(&number) {
                    Some(Ok(heard)) => heard,
                    Some(Err(why)) => {
                        notes.push(format!("V{number} wasn't rendered: {why}"));
                        continue;
                    }
                    None => continue,
                };
                let family = source.as_ref().map(|source| heard.family(source));
                let fault = heard.fault(source.as_ref());
                let redraw = recipes.get(&number).filter(|recipe| !recipe.ops.is_empty() && round < 2);
                // (a take that clips is turned down, the rest of it as it was: its level is no reason to draw it again, and the
                // producer lowering it by hand would leave it edited since Kumi wrote it (#407))
                let quieter = match &fault {
                    Some(Fault::Clips { over, .. }) if round < 2 => Some(
                        set.take(number)
                            .ok_or_else(|| "the take is gone".to_string())
                            .and_then(|take| self.backend.quieter(&take.material, *over)),
                    ),
                    _ => None,
                };
                match (fault, quieter, redraw) {
                    (Some(fault), Some(Ok(material)), _) => match self.turn_down(set, number, material, signal).await {
                        Ok(done) => {
                            if let Some(take) = set.take_mut(number) {
                                take.line = format!("{} · {}", take.line, turned_down(&fault, &done));
                            }
                            again.push(number);
                        }
                        Err(why) => notes.push(format!("V{number} {} in the render, and turning it down failed: {why}", fault.words())),
                    },
                    (Some(fault), _, Some(recipe)) => match self.redraw(set, number, recipe, bounds, round, signal).await {
                        Ok(()) => {
                            if let Some(take) = set.take_mut(number) {
                                take.line = format!("{} (drawn again after the render: it {})", take.line, fault.words());
                            }
                            again.push(number);
                        }
                        Err(why) => notes.push(format!("V{number} {} in the render, and drawing it again failed: {why}", fault.words())),
                    },
                    (Some(fault), quieter, None) => {
                        let why = quieter.and_then(Result::err).map(|why| format!(" (not turned down: {why})")).unwrap_or_default();
                        if let Some(take) = set.take_mut(number) {
                            take.line = format!("{} · heard: it {}{why}", take.line, fault.words());
                        }
                    }
                    (None, _, _) => {
                        if let Some(take) = set.take_mut(number) {
                            take.line = format!("{} · heard: {}", take.line, heard.words(family, source.as_ref()));
                        }
                    }
                }
            }
            if again.is_empty() {
                break;
            }
            pending = again;
        }
        notes
    }

    /// Draws take `number` again from another seed after its render failed, and rewrites it (one change).
    async fn redraw(
        &self,
        set: &mut Set<B>,
        number: usize,
        recipe: &Recipe,
        bounds: &Bounds,
        round: u32,
        signal: &Signal,
    ) -> Result<(), String> {
        let mut salted = set.clone();
        salted.seed = seed_of(set.seed, number, 100 + round);
        let before = set.take(number).map(|take| take.material.clone()).ok_or("the take is gone")?;
        let redrawn = self.draw(&salted, number, recipe, bounds)?;
        let row = self.backend.find(set, &[number], signal).await?.into_iter().next().unwrap_or(Value::Null);
        if !found(&row) {
            return Err(format!("it isn't in the {} any more", self.backend.words().app));
        }
        let done = self.backend.rewrite(set, &row, &before, &redrawn.material, signal).await.map_err(|(why, _)| why)?;
        let (change, material, line) = holding(&redrawn, done);
        if let Some(take) = set.take_mut(number) {
            take.digest = self.backend.digest(&material);
            take.material = material;
            take.change = Some(change);
            take.line = line;
        }
        Ok(())
    }

    /// Writes take `number` as `quieter` (the backend's take turned down after its render clipped; one change), and keeps it as
    /// Kumi's: a change or a drop takes it as Kumi wrote it. Its change, and why only part went in when it didn't all.
    async fn turn_down(
        &self,
        set: &mut Set<B>,
        number: usize,
        quieter: B::Material,
        signal: &Signal,
    ) -> Result<(String, Option<String>), String> {
        let before = set.take(number).map(|take| take.material.clone()).ok_or("the take is gone")?;
        let row = self.backend.find(set, &[number], signal).await?.into_iter().next().unwrap_or(Value::Null);
        if !found(&row) {
            return Err(format!("it isn't in the {} any more", self.backend.words().app));
        }
        let (change, instead) = self.backend.rewrite(set, &row, &before, &quieter, signal).await.map_err(|(why, _)| why)?;
        let (material, partly) = match instead {
            Some((holds, why)) => (holds, Some(why)),
            None => (quieter, None),
        };
        if let Some(take) = set.take_mut(number) {
            take.digest = self.backend.digest(&material);
            take.material = material;
            take.change = Some(change.clone());
        }
        Ok((change, partly))
    }

    fn put_back(&self, set: Set<B>) {
        let mut sets = self.sets.borrow_mut();
        if let Some(slot) = sets.iter_mut().find(|held| held.id == set.id) {
            *slot = set;
        }
    }
}

fn actions(words: &Words) -> Vec<&'static str> {
    let mut actions = vec!["make", "change", "keep", "drop", "list"];
    if words.extract.is_some() {
        actions.push("extract");
    }
    actions
}

/// The tool's description in the DAW's words.
pub fn description(words: &Words) -> String {
    let app = words.app;
    let mut text = format!(
        "Variations in {app}: numbered takes V1, V2… of a source, each its own object, never written over the source. make: source is {}; anchors are {}. \
Each of takes is a recipe: ops ({}), lines ({}), and/or from: \"V2\" to start from another take; direction names it in a few words. count without takes \
picks distinct directions. place: {}. options: {}. Kumi draws each take and checks it differs from the source and the other takes yet keeps the anchors, the \
idea, the key and the grid (re-rolling up to 8 times; novelty subtle, clear or bold, or minDistance 0 to 1; lines you write that come close are written \
with a warning), writes each as one change, {hears}, and answers a line per take. A make on the same source goes on numbering (V10 after V9); another \
source starts another set, still there by its number or name (set). change: names (\"V6\", or \"rest\": every take not kept) and takes (a recipe each, in \
order; left out, Kumi picks distinct directions) rewrite only those takes, {rewrites}: kept takes and the source are never touched (force: true only when the \
producer named a kept take), and a take the producer edited is left as it is. keep: names; each kept take is renamed “V3 · … · kept” in {app} (a change \
each), so the mark is there after a restart too; at places one take: {}. drop: names (\"rest\" by default) {drops}; rest leaves a take edited since \
Kumi wrote it, one named goes all the same. list: the set's takes.",
        words.sources,
        words.anchors,
        words.ops,
        words.lines,
        words.places,
        words.options,
        words.at,
        hears = words.hears.map_or_else(
            || format!(
                "renders and hears the takes before they're shown (never while {app} plays; listen: false skips it), turning one that peaks at full scale where the source doesn't down to the source's peak"
            ),
            str::to_owned
        ),
        rewrites = words.rewrites.unwrap_or("one change each"),
        drops = words.drops.unwrap_or("go in one change"),
    );
    if let Some(extract) = words.extract {
        text.push_str(" extract: ");
        text.push_str(extract);
    }
    text.push(' ');
    text.push_str(words.idioms);
    text
}

/// The tool's schema, with the DAW's parts.
pub fn schema(words: &Words) -> JsonObject {
    let part = |text: &str| serde_json::from_str::<Value>(text).expect("a backend's schema is JSON");
    let value = json!({"type": "object", "additionalProperties": false, "required": ["action"], "properties": {
        "action": {"type": "string", "enum": actions(words)},
        "set": {"type": ["integer", "string"], "description": "Which set of takes: its number, or its source's name (the latest by default)"},
        "source": part(words.source_schema),
        "anchors": {"type": "array", "maxItems": 512, "items": {"type": "integer", "minimum": 0}, "description": words.anchors},
        "place": {"type": "string", "enum": words.place_names},
        "count": {"type": "integer", "minimum": 1, "maximum": MOST_TAKES},
        "takes": {"type": "array", "maxItems": MOST_TAKES, "items": {"type": "object", "additionalProperties": false, "properties": {
            "direction": {"type": "string", "maxLength": 80},
            "from": {"type": "string", "description": "Another take to start from, such as V2"},
            "ops": {"type": "array", "maxItems": 12, "items": part(words.op_schema)},
            "lines": {"type": "array", "maxItems": 512, "items": {"type": "object"}, "description": words.lines}}}},
        "names": {"type": "array", "maxItems": 32, "items": {"type": "string"}, "description": "Takes: V3, or rest"},
        "force": {"type": "boolean"},
        "novelty": {"type": "string", "enum": ["subtle", "clear", "bold"]},
        "minDistance": {"type": "number", "minimum": 0, "maximum": 1, "description": "How far each take must be from the source and the others (instead of novelty)"},
        "seed": {"type": "integer", "minimum": 0},
        "listen": {"type": "boolean", "description": "false skips rendering the takes to hear them (on by default)"},
        "options": part(words.options_schema),
        "at": part(words.at_schema),
    }});
    value.as_object().cloned().expect("an object")
}

fn parse_recipe(value: &Value) -> Result<Recipe, String> {
    let ops = value.get("ops").and_then(Value::as_array).cloned().unwrap_or_default();
    let from = match value.get("from").and_then(Value::as_str) {
        Some(name) => {
            Some(name.trim().trim_start_matches(['V', 'v']).parse::<usize>().map_err(|_| format!("from: {name} isn't a take such as V2"))?)
        }
        None => None,
    };
    let lines = value.get("lines").and_then(Value::as_array).cloned();
    if ops.is_empty() && lines.is_none() && from.is_none() {
        return Err("each take needs ops, lines or from".into());
    }
    let direction = value.get("direction").and_then(Value::as_str).unwrap_or("").trim().to_string();
    Ok(Recipe { direction, ops, lines, from, picked: false })
}

/// The set a request names: by its number, or its source's name (the latest that has it); the latest by default.
fn pick_set<'a, M, D>(
    sets: &'a [TakeSet<Source<M, D>, Value, M>],
    input: &JsonObject,
) -> Result<&'a TakeSet<Source<M, D>, Value, M>, String> {
    match input.get("set") {
        Some(Value::Number(id)) => {
            let id = id.as_u64().unwrap_or(0) as usize;
            sets.iter().find(|set| set.id == id).ok_or(format!("there is no set {id} (make one first)"))
        }
        Some(Value::String(name)) => {
            let name = name.trim().to_lowercase();
            sets.iter()
                .rev()
                .find(|set| set.source.short.to_lowercase() == name)
                .or_else(|| sets.iter().rev().find(|set| set.source.label.to_lowercase().contains(&name)))
                .ok_or(format!("no set's source is named “{name}”"))
        }
        _ => sets.last().ok_or("there are no takes yet: make some first".into()),
    }
}

fn names_of(input: &JsonObject, default: &[&str]) -> Vec<String> {
    let names: Vec<String> =
        input.get("names").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
    if names.is_empty() {
        default.iter().map(|name| name.to_string()).collect()
    } else {
        names
    }
}

fn bounds_of(input: &JsonObject) -> Bounds {
    if let Some(least) = input.get("minDistance").and_then(Value::as_f64) {
        let least = least.clamp(0., 1.);
        return Bounds {
            min_source: least,
            max_source: Bounds::default().max_source.max(least),
            min_others: least.min(Bounds::default().min_others),
        };
    }
    match input.get("novelty").and_then(Value::as_str) {
        Some("subtle") => Bounds { min_source: 0.03, max_source: 0.6, min_others: 0.03 },
        Some("bold") => Bounds { min_source: 0.2, max_source: 0.95, min_others: 0.1 },
        _ => Bounds::default(),
    }
}

/// A rewrite's change, what the take holds now and its line: what was drawn, or (when only part of it went in) what the backend
/// says it holds, the line saying why.
fn holding<M: Clone>(drawn: &Drawn<M>, done: Rewritten<M>) -> (String, M, String) {
    let (change, instead) = done;
    let mut line = take_line(drawn, &change);
    match instead {
        Some((holds, why)) => {
            line.push_str(&format!(" (only part went in: {why})"));
            (change, holds, line)
        }
        None => (change, drawn.material.clone(), line),
    }
}

fn fresh_seed() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|elapsed| elapsed.as_nanos() as u64).unwrap_or(1)
}

fn direction_of(recipe: &Recipe) -> &str {
    if recipe.direction.is_empty() {
        "a variation"
    } else {
        &recipe.direction
    }
}

/// What a take turned down after its render says (#407): "turned down 1.4 dB to the source's peak (c13): its render clipped
/// (peak 1.1 dBFS)".
fn turned_down(fault: &Fault, (change, partly): &(String, Option<String>)) -> String {
    let Fault::Clips { peak, over, to_source } = fault else { return String::new() };
    let to = if *to_source { "the source's peak".to_string() } else { format!("{} dBFS", db_text(super::heard::UNDER_FULL_SCALE_DB)) };
    let partly = partly.as_ref().map(|why| format!("; only part went in: {why}")).unwrap_or_default();
    format!("turned down {over:.1} dB to {to} ({change}{partly}): its render clipped (peak {} dBFS)", db_text(*peak))
}

/// A take's line: "V4 (more reverses): reversed lines 06, 14 · 0.24 from the source (c12)".
pub fn take_line<M>(drawn: &Drawn<M>, change: &str) -> String {
    let what = if drawn.said.is_empty() { "as drawn".to_string() } else { drawn.said.join("; ") };
    let distance = drawn.distance.map(|d| format!(" \u{b7} {d:.2} from the source")).unwrap_or_default();
    let change = if change.is_empty() { String::new() } else { format!(" ({change})") };
    format!("V{} ({}): {what}{distance}{change}", drawn.number, drawn.direction)
}

/// (Names, for backends.)
pub fn names(number: usize, source: &str) -> [String; 2] {
    [take_name(number, source), kept_name(number, source)]
}

/// The names a backend finds a take by when its place is lost, in the order to try them: a kept take's kept name first (a
/// copy the producer made of it before it was kept holds the name it was made with).
pub fn names_to_find(number: usize, source: &str, state: TakeState) -> [String; 2] {
    let [made, kept] = names(number, source);
    if state == TakeState::Kept {
        [kept, made]
    } else {
        [made, kept]
    }
}

struct VariationsTool<B: VariationsBackend + 'static> {
    owner: Rc<Variations<B>>,
}

#[async_trait(?Send)]
impl<B: VariationsBackend + 'static> KernelTool for VariationsTool<B> {
    fn name(&self) -> &str {
        "variations"
    }

    fn description(&self) -> &str {
        &self.owner.description
    }

    fn input_schema(&self) -> JsonObject {
        self.owner.schema.clone()
    }

    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        Ok(match self.owner.run(input, signal).await {
            Ok(answer) => ToolResult::text(stringify(&answer)),
            Err(why) => ToolResult::error(why),
        })
    }

    fn reach(&self) -> Reach {
        Reach::DawWrite
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_lines_say_what_was_done_how_far_from_the_source_and_the_change() {
        let drawn = Drawn {
            number: 4,
            material: (),
            direction: "more reverses".into(),
            said: vec!["reversed lines 06, 14".into()],
            distance: Some(0.24),
        };
        assert_eq!(take_line(&drawn, "c12"), "V4 (more reverses): reversed lines 06, 14 \u{b7} 0.24 from the source (c12)");
    }

    #[test]
    fn a_min_distance_sets_the_floors_and_novelty_moves_them() {
        let input = |value: Value| value.as_object().cloned().unwrap();
        let least = bounds_of(&input(json!({"minDistance": 0.01})));
        assert_eq!((least.min_source, least.min_others), (0.01, 0.01));
        assert_eq!(bounds_of(&input(json!({}))), Bounds::default());
        assert!(bounds_of(&input(json!({"novelty": "bold"}))).min_source > Bounds::default().min_source);
    }

    #[test]
    fn a_recipe_needs_ops_lines_or_a_take_to_start_from() {
        assert!(parse_recipe(&json!({"direction": "x"})).is_err());
        let recipe = parse_recipe(&json!({"from": "v2", "direction": " denser "})).unwrap();
        assert_eq!((recipe.from, recipe.direction.as_str(), recipe.picked), (Some(2), "denser", false));
        assert!(parse_recipe(&json!({"from": "two"})).unwrap_err().contains("isn't a take"));
    }
}
