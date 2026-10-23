//! Where a conversation, its notes and its jobs are kept: a saved project by its id, a DAW's project not saved yet, or
//! conversations imported from files. Live's places are as they always were (a project id of 32 lowercase hex digits,
//! "unsaved"); another DAW's carry its name ("reaper-<id>", "reaper-unsaved"), without ":" since a place names a
//! folder. A store takes only the places of the DAWs it's told this Kumi integrates: Live's unless it's told more.
use crate::daw::DawId;

/// The DAWs whose places a store takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Places(pub &'static [DawId]);

impl Places {
    /// Live's, as every Kumi before other DAWs.
    pub const LIVE: Places = Places(&[DawId::Ableton]);

    pub fn takes(&self, daw: DawId) -> bool {
        self.0.contains(&daw)
    }

    /// The place `text` names, when it's one of these DAWs' or imported conversations'.
    pub fn parse(&self, text: &str) -> Option<Place> {
        let place = Place::parse(text)?;
        match place.daw() {
            Some(daw) if !self.takes(daw) => None,
            _ => Some(place),
        }
    }

    /// Whether `text` is a saved project's place for one of these DAWs.
    pub fn is_project(&self, text: &str) -> bool {
        matches!(self.parse(text), Some(Place::Saved { .. }))
    }
}

impl Default for Places {
    fn default() -> Self {
        Places::LIVE
    }
}

/// A place, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// A saved project: its DAW and the id kept in it.
    Saved { daw: DawId, id: String },
    /// The DAW's project not saved yet (one for every unsaved project of that DAW).
    Unsaved(DawId),
    /// Conversations imported from files.
    Imported,
}

/// Where `kumi import` keeps conversations from files.
pub const IMPORTED: &str = "imported";
/// Live's place for a Set not saved yet.
pub const UNSAVED: &str = "unsaved";

/// Whether `id` is a project id: 32 lowercase hex digits.
pub fn is_project_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The name a DAW's places start with; Live's have none.
fn prefix(daw: DawId) -> Option<&'static str> {
    match daw {
        DawId::Ableton => None,
        DawId::Reaper => Some("reaper"),
        DawId::Renoise => Some("renoise"),
    }
}

impl Place {
    /// Any DAW's place, as written.
    pub fn parse(text: &str) -> Option<Place> {
        if text == IMPORTED {
            return Some(Place::Imported);
        }
        if text == UNSAVED {
            return Some(Place::Unsaved(DawId::Ableton));
        }
        if is_project_id(text) {
            return Some(Place::Saved { daw: DawId::Ableton, id: text.into() });
        }
        let (name, rest) = text.split_once('-')?;
        let daw = [DawId::Reaper, DawId::Renoise].into_iter().find(|daw| prefix(*daw) == Some(name))?;
        if rest == UNSAVED {
            Some(Place::Unsaved(daw))
        } else if is_project_id(rest) {
            Some(Place::Saved { daw, id: rest.into() })
        } else {
            None
        }
    }

    /// The DAW it's a place of; None for imported conversations.
    pub fn daw(&self) -> Option<DawId> {
        match self {
            Place::Saved { daw, .. } | Place::Unsaved(daw) => Some(*daw),
            Place::Imported => None,
        }
    }

    /// The place as written: the folder it names.
    pub fn text(&self) -> String {
        match self {
            Place::Imported => IMPORTED.into(),
            Place::Unsaved(daw) => unsaved(*daw).into(),
            Place::Saved { daw, id } => match prefix(*daw) {
                Some(name) => format!("{name}-{id}"),
                None => id.clone(),
            },
        }
    }
}

/// Whether a conversation kept in `place` belongs with sessions in `daw`: one of that DAW's places, or imported from a
/// file (which carries on in a copy anywhere). Another DAW's goes on only with that DAW.
pub fn kept_for(place: &str, daw: DawId) -> bool {
    Place::parse(place).and_then(|place| place.daw()).is_none_or(|own| own == daw)
}

/// A DAW's place for its projects not saved yet: "unsaved" for Live's.
pub fn unsaved(daw: DawId) -> &'static str {
    match daw {
        DawId::Ableton => UNSAVED,
        DawId::Reaper => "reaper-unsaved",
        DawId::Renoise => "renoise-unsaved",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "a2dfd5f2cf329c593c5fdc7448524087";

    #[test]
    fn lives_places_are_as_they_were_and_another_dafs_carry_its_name() {
        for text in [ID, "unsaved", "imported", &format!("reaper-{ID}"), "reaper-unsaved", &format!("renoise-{ID}"), "renoise-unsaved"] {
            assert_eq!(Place::parse(text).map(|place| place.text()).as_deref(), Some(text), "{text}");
        }
        for text in ["A2DFD5F2CF329C593C5FDC7448524087", "unsaved:x", "../unsaved", "reaper-", "reaper-x", "ableton-unsaved", "", &ID[..31]]
        {
            assert_eq!(Place::parse(text), None, "{text}");
        }
        assert_eq!(unsaved(DawId::Ableton), "unsaved");
        assert_eq!(unsaved(DawId::Renoise), "renoise-unsaved");
        for daw in [DawId::Ableton, DawId::Reaper, DawId::Renoise] {
            assert_eq!(Place::parse(unsaved(daw)), Some(Place::Unsaved(daw)));
        }
    }

    #[test]
    fn a_conversation_belongs_with_its_places_daw_and_an_imported_one_anywhere() {
        assert!(kept_for(ID, DawId::Ableton) && kept_for("unsaved", DawId::Ableton) && kept_for("imported", DawId::Reaper));
        assert!(kept_for(&format!("reaper-{ID}"), DawId::Reaper) && kept_for("renoise-unsaved", DawId::Renoise));
        assert!(!kept_for(&format!("reaper-{ID}"), DawId::Ableton) && !kept_for(ID, DawId::Renoise));
    }

    #[test]
    fn a_store_takes_only_the_places_of_the_daws_it_knows() {
        assert!(Places::LIVE.parse(ID).is_some() && Places::LIVE.parse("unsaved").is_some() && Places::LIVE.parse("imported").is_some());
        assert_eq!(Places::LIVE.parse(&format!("reaper-{ID}")), None);
        let both = Places(&[DawId::Ableton, DawId::Reaper]);
        assert!(both.is_project(&format!("reaper-{ID}")) && !both.is_project("reaper-unsaved") && both.parse("renoise-unsaved").is_none());
    }
}
