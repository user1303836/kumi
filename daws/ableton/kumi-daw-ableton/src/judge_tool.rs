//! The judge, tune, groove, form and sound tools in Live's words: the model's side of a judged run (a goal in, a round's
//! log out). The requests, their parsing and the replies are Kumi's own (`listening::tools`).
pub use kumi_listening::tools::{
    form_request, groove_request, judge_reply, judge_request, sound_request, tune_request, FORM_TOOL, GROOVE_TOOL, JUDGE_TOOL, SOUND_TOOL,
    TUNE_TOOL,
};
use kumi_listening::tools::{form_tool, groove_tool, judge_tool, sound_tool, tune_tool, ToolDef};
use kumi_runtime::core::contracts::JsonObject;
use kumi_types::daw::DawProfile;
use std::sync::LazyLock;

static JUDGE: LazyLock<ToolDef> = LazyLock::new(|| judge_tool(&DawProfile::ABLETON));
static TUNE: LazyLock<ToolDef> = LazyLock::new(|| tune_tool(&DawProfile::ABLETON));
static SOUND: LazyLock<ToolDef> = LazyLock::new(|| sound_tool(&DawProfile::ABLETON));
static FORM: LazyLock<ToolDef> = LazyLock::new(|| form_tool(&DawProfile::ABLETON));
static GROOVE: LazyLock<ToolDef> = LazyLock::new(|| groove_tool(&DawProfile::ABLETON));
pub static JUDGE_DESCRIPTION: LazyLock<String> = LazyLock::new(|| JUDGE.description.clone());
pub static JUDGE_SCHEMA: LazyLock<JsonObject> = LazyLock::new(|| JUDGE.schema.clone());
pub static TUNE_DESCRIPTION: LazyLock<String> = LazyLock::new(|| TUNE.description.clone());
pub static TUNE_SCHEMA: LazyLock<JsonObject> = LazyLock::new(|| TUNE.schema.clone());
pub static SOUND_DESCRIPTION: LazyLock<String> = LazyLock::new(|| SOUND.description.clone());
pub static SOUND_SCHEMA: LazyLock<JsonObject> = LazyLock::new(|| SOUND.schema.clone());
pub static FORM_DESCRIPTION: LazyLock<String> = LazyLock::new(|| FORM.description.clone());
pub static FORM_SCHEMA: LazyLock<JsonObject> = LazyLock::new(|| FORM.schema.clone());
pub static GROOVE_DESCRIPTION: LazyLock<String> = LazyLock::new(|| GROOVE.description.clone());
pub static GROOVE_SCHEMA: LazyLock<JsonObject> = LazyLock::new(|| GROOVE.schema.clone());
