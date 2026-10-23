//! Kumi's agent: the kernel (the agent loop), the DAW-neutral core (sessions, HISTORY, plans, places, FOCUS views and
//! the workflows), the MCP client, and what every DAW's integration shares (with none and the fallback). It names
//! what the other layers define (kumi-types, kumi-ai, kumi-media, kumi-net, kumi-web, kumi-library, kumi-listening)
//! by their crates; its root names what other crates use most of its own.

pub mod core;
pub mod integrations;
pub mod kernel;
pub mod mcp;

pub use core::contracts::{
    ArrangementStrip, CatchUp, ChainNode, Claim, ClipView, ConnectionState, ConversationMatch, ConversationNow, ConversationStore,
    ConversationSummary, DeviceNode, DeviceTree, DisconnectCause, Integration, IntegrationFactory, Kernel, KernelCheckpoint, KernelEvent,
    KernelFactory, KernelOptions, LibraryEvent, LiveFocus, LiveTransport, Memory, MemoryEvent, MemoryNote, MemoryScope, MemoryStore,
    NoteChange, Observation, PinnedNode, RecipeEvent, RecipeSummary, SavedConversation, SessionController, SessionEvent, SessionStatus,
    SessionStrip, SetAsk, SetChoice, TechniqueEvent, TechniqueSummary, TranscriptLine, TurnResult, TurnState,
};
pub use core::conversations::{create_conversation_store, create_conversation_store_keeping, gone_words, KEEP_DAYS};
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
pub use integrations::fallback::{with_fallback, with_fallback_failure};
pub use integrations::{create_project_store, Baseline, ProjectStore};
pub use kernel::agent::{create_agent_kernel, AgentKernel, AgentKernelOptions, Checkpoint};
