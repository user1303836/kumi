//! Taking requests back: `/takeback [n]`. Kumi undoes the changes of the requests taken back, newest first, and puts
//! the earliest one's words back in the box.
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
