//! What each DAW Kumi integrates tells the library: where it keeps the producer's sounds, presets and projects, the
//! projects it opened lately, and its manual as a tool. Each DAW's integration implements [`LibrarySources`], and the
//! app gives the library one for each DAW it registers ([`Daws`]), in its registry's order.

use std::rc::Rc;

use kumi_net::web::WebClient;
use kumi_types::contracts::KernelTool;
use kumi_types::daw::DawProfile;

use crate::sources::{Source, SourceOptions};
use crate::tools::OnEvent;

/// A DAW's places by its layout: those read before the folders the producer names (Live's User Library and Places) and
/// those after them (Splice's downloads, Live's packs and its Core Library).
#[derive(Debug, Clone, Default)]
pub struct Places {
    pub before: Vec<Source>,
    pub after: Vec<Source>,
}

/// What a DAW's manual tool is given: the library's folder, where it keeps what it read, and how to read the web.
#[derive(Clone, Default)]
pub struct ManualOptions {
    pub dir: String,
    pub client: Option<Rc<dyn WebClient>>,
    pub on_event: Option<OnEvent>,
    pub base: Option<String>,
    pub now: Option<Rc<dyn Fn() -> i64>>,
}

/// What a DAW tells the library about itself.
pub trait LibrarySources: Sync {
    /// The DAW's profile.
    fn profile(&self) -> &'static DawProfile;
    /// Its places, by its layout.
    fn places(&self, _options: &SourceOptions) -> Places {
        Places::default()
    }
    /// Its own library, read by its layout even inside a folder the producer named (REAPER's resource folder,
    /// Renoise's libraries).
    fn own_library(&self, _options: &SourceOptions) -> Vec<Source> {
        Vec::new()
    }
    /// The projects it opened lately, the producer's own.
    fn recent_projects(&self, _options: &SourceOptions) -> Vec<String> {
        Vec::new()
    }
    /// Its manual as a tool, for a DAW whose manual Kumi reads (Live's).
    fn manual_tool(&self, _options: ManualOptions) -> Option<Rc<dyn KernelTool>> {
        None
    }
}

/// The DAWs the library hears from, in the app's registry's order. None: only the folders the producer names.
#[derive(Clone, Copy, Default)]
pub struct Daws(pub &'static [&'static dyn LibrarySources]);

impl Daws {
    pub fn iter(self) -> impl Iterator<Item = &'static dyn LibrarySources> {
        self.0.iter().copied()
    }
}

impl std::fmt::Debug for Daws {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter().map(|daw| daw.profile().id)).finish()
    }
}
