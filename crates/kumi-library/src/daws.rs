//! What each DAW Kumi integrates tells the library: where it keeps the producer's sounds, presets and projects, the
//! projects it opened lately, how its project files read, and its manual as a tool. Each DAW's integration implements
//! [`LibrarySources`], and the app gives the library one for each DAW it registers ([`Daws`]), in its registry's order.

use std::path::Path;
use std::rc::Rc;

use kumi_common::abort::Signal;
use kumi_net::web::WebClient;
use kumi_types::contracts::KernelTool;
use kumi_types::daw::{DawId, DawProfile};
use kumi_types::errors::RuntimeError;

use crate::sets::SetSummary;
use crate::sources::{Source, SourceKind, SourceOptions};
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

/// How a DAW's own library is laid out, which a walk of it reads by: its top folders Kumi reads and the file types each
/// holds ("" is a file with no extension, as a JSFX is), its folders whose audio files are sounds, and its folders whose
/// presets keep a format of their own (REAPER's "effects": "jsfx").
#[derive(Debug)]
pub struct Layout {
    pub folders: &'static [(&'static str, &'static [&'static str])],
    pub sounds: &'static [&'static str],
    pub formats: &'static [(&'static str, &'static str)],
}

impl Layout {
    /// No library of its own.
    pub const NONE: Layout = Layout { folders: &[], sounds: &[], formats: &[] };
}

/// A DAW's own project files, which the library learns as the producer's projects beside Live's Sets (whose reading is
/// the library's own): their extension, which of them are copies or backups it leaves out, and how one reads.
#[derive(Debug)]
pub struct ProjectFormat {
    /// The extension, lower case with its dot (".rpp").
    pub extension: &'static str,
    /// Whether a file of it (its whole path: a DAW may keep backups in a folder of their own) is a copy or backup
    /// rather than the producer's project.
    pub skip: fn(&Path) -> bool,
    /// Reads one, off Kumi's thread: what `my_sets` shows of it. Its `daw` is set by the library.
    pub read: fn(&Path, &Signal) -> Result<SetSummary, RuntimeError>,
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
    /// How its own library is laid out (`own_library`'s sources are read by it).
    fn layout(&self) -> &'static Layout {
        &Layout::NONE
    }
    /// Its project files, for a DAW whose projects Kumi reads (Live's Sets are the library's own).
    fn projects(&self) -> Option<&'static ProjectFormat> {
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

    /// How a source is laid out: its DAW's layout for a DAW's own library, else none.
    pub fn layout(self, source: &Source) -> &'static Layout {
        let SourceKind::Daw(id) = source.kind else { return &Layout::NONE };
        self.iter().find(|daw| daw.profile().id == Some(id)).map_or(&Layout::NONE, |daw| daw.layout())
    }

    /// The project files of `daw`, when Kumi reads them.
    pub fn projects_of(self, daw: DawId) -> Option<&'static ProjectFormat> {
        self.iter().find(|own| own.profile().id == Some(daw)).and_then(|own| own.projects())
    }

    /// The DAW whose project files have this extension (lower case, with its dot), and how they read.
    pub fn project_format(self, extension: &str) -> Option<(DawId, &'static ProjectFormat)> {
        self.iter().find_map(|own| Some((own.profile().id?, own.projects().filter(|format| format.extension == extension)?)))
    }
}

impl std::fmt::Debug for Daws {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter().map(|daw| daw.profile().id)).finish()
    }
}
