//! Taking requests back: `/takeback [n]`, and `esc esc`'s list of the producer's requests, newest first, each with how
//! many changes it made. Kumi undoes the changes of the request chosen and of each one after it, newest first, and puts
//! its words back in the box; before it does, the list says which of those changes Kumi can't undo.
use super::super::picker::{NoteTone, PickerOptions};
use super::panels::{inert, item, noted};
use super::*;

impl TuiApp {
    /// /takeback: the producer's last request taken back, or their last `words` of them. Kumi undoes their changes newest
    /// first and puts the earliest one's words back in the box.
    pub(super) async fn take_back(&self, words: &str) {
        if !self.may_take_back() {
            return;
        }
        let Some(count) = (if words.is_empty() { Some(1) } else { words.parse::<usize>().ok().filter(|n| *n > 0) }) else {
            self.notice("/takeback takes back your last request, and /takeback 3 your last three.", NoticeTone::Info);
            return;
        };
        // Newest first: the earliest of the last `count`, or the first of all when there are fewer.
        let earliest = move |requests: &[Requested]| requests.get(count.min(requests.len()).saturating_sub(1)).map(|asked| asked.at);
        self.take_back_where(earliest, "There's no request of yours to take back yet.").await;
    }
    /// Whether requests can be taken back now. When they can't, the producer hears why, unless Kumi is closing or already
    /// taking some back.
    fn may_take_back(&self) -> bool {
        if self.0.state.borrow().closing || self.0.state.borrow().undoing {
            return false;
        }
        if !self.can().take_back {
            self.notice("Kumi can't take requests back here: this conversation isn't kept.", NoticeTone::Info);
            return false;
        }
        if self.busy() {
            self.notice("Kumi is still working. Press esc to stop it first, then take it back.", NoticeTone::Info);
            return false;
        }
        true
    }
    /// Takes back the request `earliest` finds among those that stand (newest first, as the session has them now), and
    /// each one after it; `none` says why when it finds none.
    async fn take_back_where(&self, earliest: impl FnOnce(&[Requested]) -> Option<i64>, none: &str) {
        {
            let mut state = self.0.state.borrow_mut();
            state.undoing = true;
            state.activity = "taking it back".into();
        }
        self.0.scheduler.request();
        let taken = match self.session().op(op::TakeBackPlan {}).await {
            Ok(requests) => match earliest(&requests) {
                Some(to) => self.session().op(op::TakeBack { to }).await.map(|taken| Some((taken, requests))),
                None => Ok(None),
            },
            Err(error) => Err(error),
        };
        match taken {
            Ok(Some((taken, requests))) => {
                let left = left_as_they_are(&requests[..(taken.requests as usize).min(requests.len())]);
                self.took_back(taken, left);
            }
            Ok(None) => self.notice(none, NoticeTone::Info),
            Err(error) => {
                if !self.0.state.borrow().closing {
                    self.error(&error);
                }
            }
        }
        self.0.state.borrow_mut().undoing = false;
        self.0.scheduler.request();
    }
    /// What taking requests back did, in words, with the earliest one's words back in the box.
    fn took_back(&self, taken: TakenBack, left: Vec<ChangeRecord>) {
        let undid = (!taken.undone.is_empty()).then(|| changes(taken.undone.len()));
        let mut said = vec![];
        match (taken.requests, undid) {
            (0, None) => {}
            (0, Some(undid)) => said.push(format!("Undid {undid}.")),
            (n, undid) => said.push(format!(
                "Took back {}{}.",
                if n == 1 { "your last request".to_string() } else { format!("your last {n} requests") },
                undid.map(|undid| format!(" and undid {undid}")).unwrap_or_default()
            )),
        }
        if let Some(failed) = &taken.failed {
            said.push(
                string::trim(&format!("Kumi couldn't undo {} and stopped there. {}", failed.title, failed.note.as_deref().unwrap_or("")))
                    .to_string(),
            );
        }
        if !left.is_empty() {
            let named: Vec<String> = left.iter().map(|change| format!("{} ({})", change.title, why_left(change))).collect();
            said.push(format!("Left as {}: {}.", if left.len() == 1 { "it is" } else { "they are" }, named.join("; ")));
        }
        if let Some(words) = taken.words.filter(|words| !words.is_empty()) {
            self.0.state.borrow_mut().editor.set(&words);
            said.push("Its words are back in the box to change and send again.".into());
        }
        let tone = if taken.failed.is_some() { NoticeTone::Warn } else { NoticeTone::Info };
        self.notice(&said.join(" "), tone);
    }
    /// esc esc (app.take-back): the producer's requests that stand, newest first, each with how many changes it made and how many of them
    /// Kumi can't undo. Choosing one asks before it's taken back, with each one after it.
    pub(super) async fn pick_take_back(&self) {
        if !self.may_take_back() {
            return;
        }
        let requests = match self.session().op(op::TakeBackPlan {}).await {
            Ok(requests) => requests,
            Err(error) => return self.error(&error),
        };
        // Something may have begun while the log was read (a list over Kumi at work would take the esc that stops it).
        if self.busy() || self.0.state.borrow().closing {
            return;
        }
        if requests.is_empty() {
            self.notice("There's no request of yours to take back yet.", NoticeTone::Info);
            return;
        }
        let items = requests
            .iter()
            .map(|asked| {
                // What it changed that still stands: one undone since, or a sound only heard, isn't counted.
                let standing =
                    asked.changes.iter().filter(|change| !matches!(change.state, ChangeState::Undone | ChangeState::Heard)).count();
                let row = item(self.clean(&asked.words.replace('\n', " "), 70), asked.at.to_string(), changes(standing));
                match left_as_they_are(std::slice::from_ref(asked)).len() {
                    0 => row,
                    stuck => noted(row, format!("{stuck} can't be undone"), NoteTone::Warn),
                }
            })
            .collect();
        let requests = Rc::new(requests);
        let options = PickerOptions { filterable: true, ..Default::default() };
        self.pick(Picker::with_options("Take a request back", items, options), move |app, chosen| {
            let requests = requests.clone();
            async move {
                if let Some(at) = chosen.value.and_then(|value| value.parse::<i64>().ok()) {
                    app.confirm_take_back(&requests, at);
                }
                Ok(())
            }
        });
    }
    /// Before the request asked at `at` is taken back, with each one after it: what Kumi undoes, and each change it can't
    /// undo, which stays as it is.
    fn confirm_take_back(&self, requests: &[Requested], at: i64) {
        let Some(index) = requests.iter().position(|asked| asked.at == at) else { return };
        let going = &requests[..=index];
        let undoes = going.iter().flat_map(|asked| &asked.changes).filter(|change| change.state == ChangeState::Applied).count();
        let mut items: Vec<PickerItem> = left_as_they_are(going)
            .iter()
            .map(|change| inert(format!("Stays as it is: {} ({})", self.clean(&change.title, 60), why_left(change))))
            .collect();
        // (beside the choices, in the 40 columns a list leaves beside its longest)
        let words = if undoes == 0 {
            "Nothing to undo; its words come back".to_string()
        } else {
            format!("Undoes {}; its words come back", changes(undoes))
        };
        items.push(item(if index == 0 { "Take it back" } else { "Take them back" }, "take", words));
        // A copy carries on from the requests before it, so the first has none.
        if index + 1 < requests.len() {
            items.push(item("Carry on from here in a copy", "copy", "Leaves your DAW and the original alone"));
        }
        items.push(PickerItem::new("Not now", "later"));
        let asked = self.clean(&going[index].words.replace('\n', " "), 50);
        let title = match index {
            0 => format!("Take back “{asked}”?"),
            1 => format!("Take back “{asked}” and the request after it?"),
            n => format!("Take back “{asked}” and the {n} requests after it?"),
        };
        let words = going[index].words.clone();
        self.pick(Picker::new(title, items), move |app, answer| {
            let words = words.clone();
            async move {
                app.close_panel();
                match answer.value.as_deref() {
                    Some("take") if app.may_take_back() => {
                        let still = move |requests: &[Requested]| requests.iter().any(|asked| asked.at == at).then_some(at);
                        app.take_back_where(still, "That request isn't there to take back any more.").await;
                    }
                    Some("copy") if app.may_take_back() => app.carry_on_before(at, &words).await,
                    _ => {}
                }
                Ok(())
            }
        });
    }
    /// A copy of the conversation as it stood just before the request asked at `at`, to carry on in, with that request's
    /// `words` in the box. The session says what it did, and the screen shows the copy as it stands.
    async fn carry_on_before(&self, at: i64, words: &str) {
        {
            let mut state = self.0.state.borrow_mut();
            state.undoing = true;
            state.activity = "making the copy".into();
        }
        self.0.scheduler.request();
        match self.session().op(op::ForkConversation { name: None, before: Some(at) }).await {
            Ok(Some(_)) => self.0.state.borrow_mut().editor.set(words),
            Ok(None) => self.notice("There's nothing to carry on from yet.", NoticeTone::Info),
            Err(error) => {
                if !self.0.state.borrow().closing {
                    self.error(&error);
                }
            }
        }
        self.0.state.borrow_mut().undoing = false;
        self.0.scheduler.request();
    }
}

/// "1 change", "3 changes", "no changes".
fn changes(n: usize) -> String {
    match n {
        0 => "no changes".into(),
        1 => "1 change".into(),
        n => format!("{n} changes"),
    }
}

/// What `requests` changed that Kumi can't undo, as the plan has them: when they're taken back, these stay as they are.
fn left_as_they_are(requests: &[Requested]) -> Vec<ChangeRecord> {
    requests
        .iter()
        .flat_map(|asked| &asked.changes)
        .filter(|change| !matches!(change.state, ChangeState::Applied | ChangeState::Undone | ChangeState::Heard))
        .cloned()
        .collect()
}

/// Why Kumi can't undo a change it leaves as it is.
fn why_left(change: &ChangeRecord) -> &'static str {
    match change.state {
        ChangeState::Expired => "from an earlier session",
        ChangeState::Unsure => "never confirmed",
        _ => "Kumi can't undo it",
    }
}
