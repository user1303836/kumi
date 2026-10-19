//! Kumi's database rows as the runtime knows them, both ways: a note, a technique, a lesson.

use super::{
    contracts::MemoryNote,
    goal::Best,
    playbook::{Lesson, Reaction},
    techniques::{Technique, TechniqueBody, TechniqueSource},
};
use kumi_store::{lessons, notes, techniques as stored};
use serde_json::Value;

pub fn note_row(n: &MemoryNote) -> notes::Note {
    notes::Note { label: n.id.clone(), text: n.text.clone(), pinned: n.pinned, at: n.at }
}
pub fn note_of(n: notes::Note) -> MemoryNote {
    MemoryNote { id: n.label, text: n.text, at: n.at, pinned: n.pinned }
}

pub fn technique_row(t: &Technique) -> stored::Technique {
    let source = t.body.source.as_ref();
    stored::Technique {
        label: t.id.clone(),
        name: t.body.name.clone(),
        fits: t.body.fits.clone(),
        idea: t.body.idea.clone(),
        settings: t.body.settings.clone(),
        substitutes: t.body.substitutes.clone(),
        recipe: t.body.recipe.clone(),
        source_title: source.and_then(|s| s.title.clone()),
        source_url: source.and_then(|s| s.url.clone()),
        request: t.request.clone(),
        used: t.used,
        undone: t.undone,
        at: t.at as i64,
        updated: t.updated.map(|at| at as i64),
        last_used: t.last_used.map(|at| at as i64),
    }
}
pub fn technique_of(t: stored::Technique) -> Technique {
    let source = (t.source_title.is_some() || t.source_url.is_some()).then(|| TechniqueSource { title: t.source_title, url: t.source_url });
    Technique {
        body: TechniqueBody {
            name: t.name,
            fits: t.fits,
            idea: t.idea,
            settings: t.settings,
            substitutes: t.substitutes,
            recipe: t.recipe,
            source,
        },
        id: t.label,
        at: t.at as f64,
        used: t.used,
        updated: t.updated.map(|at| at as f64),
        last_used: t.last_used.map(|at| at as f64),
        request: t.request,
        undone: t.undone,
    }
}

pub fn lesson_row(l: &Lesson) -> lessons::Lesson {
    lessons::Lesson {
        label: l.id.clone(),
        matched: l.matched.clone(),
        winner: l.winner.clone(),
        from: l.from,
        to: l.to,
        moves: serde_json::to_value(&l.moves).unwrap_or(Value::Array(vec![])),
        reaction: l.reaction.map(|r| match r {
            Reaction::Liked => "liked".into(),
            Reaction::Disliked => "disliked".into(),
        }),
        at: l.at as i64,
    }
}
pub fn lesson_of(l: lessons::Lesson) -> Lesson {
    Lesson {
        id: l.label,
        at: l.at as f64,
        matched: l.matched,
        winner: l.winner,
        from: l.from,
        to: l.to,
        moves: serde_json::from_value::<Vec<Best>>(l.moves).unwrap_or_default(),
        reaction: match l.reaction.as_deref() {
            Some("liked") => Some(Reaction::Liked),
            Some("disliked") => Some(Reaction::Disliked),
            _ => None,
        },
    }
}
