//! What a session answers a request it has nothing for: no part for it, or none set up. The tests' sessions answer what
//! their tests ask, and leave every other request to [`answer`] (what the session's old trait answered by default).
use kumi_agent::core::{
    contracts::ConversationMatch,
    protocol::{op, Op, Reply},
};
use kumi_types::errors::RuntimeError;

fn absent() -> RuntimeError {
    RuntimeError::plain("Not available")
}

/// `op`, answered by a session that has nothing for it.
pub fn answer(op: Op) -> Result<Reply, RuntimeError> {
    let background = || Err("Kumi can't run background jobs here.".to_string());
    Ok(match op {
        Op::Start(_) => Reply::Start(()),
        Op::Submit(_) => Reply::Submit(()),
        Op::References(_) => Reply::References(vec![]),
        Op::Steer(_) => Reply::Steer(false),
        Op::Aside(_) => return Err(absent()),
        Op::StopAside(_) => Reply::StopAside(()),
        Op::Cancel(_) => Reply::Cancel(()),
        Op::Close(_) => Reply::Close(()),
        Op::Undo(_) => Reply::Undo(None),
        Op::TakeBackPlan(_) => Reply::TakeBackPlan(vec![]),
        Op::TakeBack(_) => return Err(RuntimeError::plain("Nothing can be taken back here.")),
        Op::Refresh(_) => Reply::Refresh(()),
        Op::NewConversation(_) => Reply::NewConversation(()),
        Op::Reconnect(_) => return Err(absent()),
        Op::Reconfigure(_) => Reply::Reconfigure(()),
        Op::Instant(_) => Reply::Instant(None),
        Op::Pick(_) => Reply::Pick(()),
        Op::Conversations(_) => Reply::Conversations(vec![]),
        Op::AllConversations(_) => Reply::AllConversations(vec![]),
        Op::FindConversation(_) => Reply::FindConversation(ConversationMatch::None),
        Op::ResumeConversation(_) => Reply::ResumeConversation(false),
        Op::RenameConversation(_) => Reply::RenameConversation(false),
        Op::PinConversation(_) => Reply::PinConversation(false),
        Op::DeleteConversation(_) => Reply::DeleteConversation(false),
        Op::ForkConversation(_) => Reply::ForkConversation(None),
        Op::AnswerSet(_) => Reply::AnswerSet(()),
        Op::ConversationNow(_) => Reply::ConversationNow(None),
        Op::ConversationLines(_) => Reply::ConversationLines(vec![]),
        Op::ConversationToExport(_) => Reply::ConversationToExport(None),
        Op::Memory(_) => Reply::Memory(None),
        Op::Remembered(_) => Reply::Remembered(None),
        Op::Taste(_) => Reply::Taste(vec![]),
        Op::ForgetTaste(_) => Reply::ForgetTaste(false),
        Op::Forget(_) => Reply::Forget(None),
        Op::ChangeNote(_) => Reply::ChangeNote(None),
        Op::Recipes(_) => Reply::Recipes(vec![]),
        Op::PinForRecipe(op::PinForRecipe { pin }) => Reply::PinForRecipe(Some(pin)),
        Op::RunRecipe(_) => return Err(absent()),
        Op::ForgetRecipe(_) => Reply::ForgetRecipe(false),
        Op::Techniques(_) => Reply::Techniques(vec![]),
        Op::ForgetTechnique(_) => Reply::ForgetTechnique(false),
        Op::AnswerTechnique(_) => Reply::AnswerTechnique(false),
        Op::Lessons(_) => Reply::Lessons(vec![]),
        Op::ForgetLesson(_) => Reply::ForgetLesson(false),
        Op::Goal(_) => return Err(absent()),
        Op::StopGoal(_) => Reply::StopGoal(false),
        Op::StopLoop(_) => Reply::StopLoop(false),
        Op::StopDaw(_) => Reply::StopDaw(false),
        Op::DawMessage(_) => Reply::DawMessage(false),
        Op::DawRecording(_) => Reply::DawRecording(None),
        Op::StopJob(_) => Reply::StopJob(false),
        Op::RunJobAgain(_) => Reply::RunJobAgain(background()),
        Op::Background(_) => Reply::Background(background()),
        Op::CarryOn(_) => Reply::CarryOn(false),
        Op::Palette(_) => Reply::Palette(None),
        Op::PaletteGather(_) => return Err(absent()),
        Op::PaletteRemove(_) => Reply::PaletteRemove(false),
        Op::PaletteCommand(_) => return Err(absent()),
        Op::PaletteNames(_) => Reply::PaletteNames(vec![]),
        // What the session's parts answer (its music, its map, its slots, its models): none, so none of them.
        Op::Music(_)
        | Op::MusicFolders(_)
        | Op::LearnMusic(_)
        | Op::SetMusicFolders(_)
        | Op::HoldMusic(_)
        | Op::ForgetMusic(_)
        | Op::Map(_)
        | Op::Slots(_)
        | Op::Providers(_)
        | Op::LocalModels(_)
        | Op::ChooseDefault(_)
        | Op::Models(_)
        | Op::ChooseModel(_)
        | Op::SetEffort(_)
        | Op::SetFast(_)
        | Op::SetFallback(_)
        | Op::SaveKey(_)
        | Op::SignOut(_)
        | Op::SignInChatgpt(_)
        | Op::MissingModel(_) => return Err(absent()),
        Op::StopSlots(_) => Reply::StopSlots(()),
        Op::StopSignIn(_) => Reply::StopSignIn(()),
        Op::FocusBody(_) => Reply::FocusBody(None),
    })
}
