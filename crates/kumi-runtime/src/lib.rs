//! The crate root.

pub mod ai;
pub mod audio;
pub mod auth;
pub mod command;
pub mod core;
pub mod devices;
pub mod ears;
pub mod hands;
pub mod integrations;
pub mod kernel;
pub mod library;
pub mod listening;
pub mod mcp;
pub mod models;
pub mod notation;
pub mod plugins;
pub mod providers;
pub mod references;
pub mod slots;
pub mod system;
pub mod version;
pub mod video;
pub mod voice;
pub mod web;

pub use audio::tools::{listening_tools, LISTEN_TOOL};
pub use audio::{analyze_file, closeness, compare, hear, Analysis, Closeness, Comparison};
pub use auth::openai_codex::{
    login_codex_browser, login_codex_device, login_hint, read_pi_codex_login, DEVICE_VERIFICATION_URL, LOGIN_HINT, OPENAI_CODEX,
};
pub use auth::store::{open_credential_store, valid_api_key, ApiKeyCredential, Credential, CredentialStore, OAuthCredential};
pub use command::{INSTALLED, KUMI, KUMI_REPAIR, KUMI_START};
pub use core::contracts::{
    ArrangementStrip, AuditionEvent, AuditionRequest, AuditionResult, CatchUp, ChainNode, ChangeFamily, ChangeRecord, ClipNote, ClipView,
    ConnectionState, ConversationStore, ConversationSummary, DeviceNode, DevicePlacement, DeviceTree, DisconnectCause, HeardEvent,
    Integration, IntegrationFactory, JsonObject, Kernel, KernelCheckpoint, KernelEvent, KernelFactory, KernelOptions, KernelTool,
    LibraryEvent, LibraryStatus, LiveFocus, LiveTransport, Memory, MemoryEvent, MemoryNote, MemoryScope, MemoryStore, NoteChange,
    Observation, PinnedNode, RecipeEvent, RecipeSummary, SavedConversation, SessionController, SessionEvent, SessionStatus, SessionStrip,
    StreamingCall, TechniqueEvent, TechniqueSummary, ToolImage, ToolResult, TranscriptLine, TurnResult, TurnState, Usage, WatchedEvent,
    WebEvent,
};
pub use core::errors::{FailureKind, KumiError, RuntimeError};
pub use core::gaps::{gap_tools, GAP_GUIDANCE, GAP_TOOL};
pub use core::goal::{create_goal_store, GoalBudget, GoalState, GoalStatus, GoalStore, GOAL_BUDGET};
pub use core::goal_mode::{create_objective_store, Objective, ObjectiveBudget, ObjectiveStatus, ObjectiveStore, OBJECTIVE_BUDGET};
pub use core::match_run::{starts_match, MatchBudget, MatchRun, MatchStatus, MatchStop, MATCH_BUDGET};
pub use core::memory::{create_memory_store, memory_instructions, FORGET_TOOL, MAX_NOTE, MAX_NOTES, REMEMBER_TOOL};
pub use core::playbook::{create_playbook_store, lesson_line, playbook_brief, Lesson, PlaybookStore};
pub use core::recipes::{
    create_recipe_store, recipe_instructions, recipe_tools, slug, Recipe, RecipeStore, FORGET_RECIPE_TOOL, RUN_RECIPE_TOOL,
    SAVE_RECIPE_TOOL,
};
pub use core::session::{create_session, Session, SessionOptions};
pub use core::techniques::{
    check_technique, create_technique_store, technique_instructions, technique_tools, Technique, TechniqueBody, TechniqueDrafts,
    TechniqueSource, TechniqueStore, MAX_TECHNIQUES, TECHNIQUE_GUIDANCE, TECHNIQUE_TOOL,
};
pub use ears::device::{install_ears, EARS_NAME};
pub use hands::{can_build_hands, open_hands, Hands};
pub use integrations::ableton::focus::parse_focus;
pub use integrations::ableton::project::{create_conversation_store, create_project_store, since, Baseline, ProjectStore};
pub use integrations::ableton::{create_ableton_integration, create_inference_only_integration, BRIDGE_TOOLS};
pub use integrations::fallback::with_fallback;
pub use kernel::agent::{create_agent_kernel, AgentKernel, AgentKernelOptions, Checkpoint, ModelBinding, ModelRequest};
pub use kernel::budget::{budget_for, ContextBudget, DEFAULT_BUDGET};
pub use library::{
    create_library, library_logs, library_sources, read_state as read_library_state, LearnProgress, Library, LibraryOptions, LibraryState,
    Source as LibrarySource, TasteLine, FIND_PRESETS_TOOL, FIND_SOUNDS_TOOL, MANUAL_TOOL, MY_SETS_TOOL,
};
pub use providers::local::{
    context_for, list_local_models, local_installed, local_servers, parse_local_model_id, probe_local, resolve_local_model, start_hint,
    LocalBinding, LocalKind, LocalServer, ServerSetting, LOCAL_PROVIDERS,
};
pub use providers::models::{check_api_key, list_models, ModelInfo};
pub use providers::{
    api_key_for, parse_model_id, resolve_model, Effort, ProviderId, ProviderInfo, ResolveModelOptions, API_KEY_ENV, EFFORTS, PROVIDERS,
    PROVIDER_INFO, USER_AGENT,
};
pub use system::system_program;
pub use version::KUMI_VERSION;
pub use video::programs::{configure_programs, ffmpeg_hint, whisper_hint};
pub use video::tool::{video_tools, WATCH_VIDEO_TOOL};
pub use video::{find_ffmpeg, find_whisper, find_yt_dlp, watch_video, youtube_id, WatchRequest, Watched};
pub use voice::{
    list_microphones, listen, microphone_allowed, prepare_voice, terminal_app, voice_prompt, voice_readiness, write_down, Heard, Listening,
    VoiceError, VoiceReadiness, VoiceTrouble,
};
pub use web::net::{create_web_client, WebClient};
pub use web::tool::{web_tools, READ_WEB_TOOL, SEARCH_WEB_TOOL};
