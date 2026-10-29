//! The command table, as the full-screen app read lines before it had one: what each line asks for (the order of its old
//! `submit()`, rows C1–C41 of the review's inventory), and what the `/` menu finds for what's typed.
use kumi_agent::core::{
    commands::{is_command, matching, named, run, By, Effect, Found, Fronts, Now, Why, COMMANDS},
    protocol::Capabilities,
};
use std::collections::BTreeSet;

/// A session that can do everything, with a front end that has everything, idle.
fn everything() -> Now {
    let can = Capabilities {
        attachments: true,
        steer: true,
        aside: true,
        reconnect: true,
        conversations: true,
        resume_conversation: true,
        watch: true,
        memory: true,
        library: true,
        taste: true,
        forget_taste: true,
        forget: true,
        change_note: true,
        recipes: true,
        instant: true,
        run_recipe: true,
        forget_recipe: true,
        techniques: true,
        forget_technique: true,
        answer_technique: true,
        goal: true,
        stop_goal: true,
        stop_loop: true,
        goal_status: true,
        lessons: true,
        forget_lesson: true,
        stop_daw: true,
        daw_message: true,
        jobs: true,
        palette: true,
        device_tree: true,
        clip_view: true,
        session_strip: true,
        arrangement_strip: true,
        reconfigure: true,
        music: true,
        map: true,
        slots: true,
        models: true,
    };
    let fronts = Fronts { voice: true, updates: true, switch_daw: true, willington: true };
    Now { busy: false, goal_running: false, can, fronts }
}

fn busy() -> Now {
    Now { busy: true, ..everything() }
}

/// A session that can do nothing, with a front end that has nothing, idle.
fn nothing() -> Now {
    Now { busy: false, goal_running: false, can: Capabilities::default(), fronts: Fronts::default() }
}

#[test]
fn every_command_has_a_name_of_its_own() {
    let mut names = BTreeSet::new();
    for command in COMMANDS {
        for name in std::iter::once(&command.name).chain(command.aliases) {
            assert!(is_command(name), "{name}");
            assert!(names.insert(*name), "{name} twice");
        }
        assert_eq!(named(command.name).map(|found| found.name), Some(command.name));
    }
    assert_eq!(COMMANDS.len(), 37);
}

#[test]
fn a_message_is_not_a_command_and_a_path_is_a_message() {
    for line in ["make the bass darker", "/Users/me/kick.wav", "/", "/2x louder", "/goal/x"] {
        assert_eq!(run(line, &everything()), Effect::Message, "{line}");
        assert_eq!(run(line, &busy()), Effect::Message, "{line}");
    }
}

#[test]
fn each_command_reads_as_its_own_with_its_words() {
    let now = everything();
    let cases = [
        ("/quit", Effect::Quit),
        ("/help", Effect::Help),
        ("/changelog", Effect::Changelog),
        ("/jobs", Effect::Jobs),
        ("/voice", Effect::Voice),
        ("/fast", Effect::Fast),
        ("/model", Effect::Model),
        ("/effort", Effect::Effort),
        ("/login", Effect::Login),
        ("/logout", Effect::Logout),
        ("/recipes", Effect::Recipes),
        ("/memory", Effect::Memory),
        ("/resume", Effect::Resume { word: String::new() }),
        ("/resume  night drive ", Effect::Resume { word: "night drive".into() }),
        ("/conversations c3f", Effect::Resume { word: "c3f".into() }),
        ("/rename", Effect::Rename { words: String::new() }),
        ("/rename c3f Night drive", Effect::Rename { words: "c3f Night drive".into() }),
        ("/fork take two", Effect::Fork { name: "take two".into() }),
        ("/recipe", Effect::Recipe),
        ("/recipe tidy level=-6", Effect::Recipe),
        ("/note p3 Prefers short reverbs", Effect::Note),
        ("/stop", Effect::Stop),
        ("/daw reaper", Effect::Daw),
        ("/update", Effect::Update),
        ("/slots listening gemini", Effect::Slots),
        ("/music add ~/Music", Effect::Music),
        ("/map open", Effect::Map),
        ("/palette use Drums", Effect::Palette),
        ("/status", Effect::Status),
        ("/btw why the low shelf?", Effect::Btw),
        ("/goal", Effect::Goal),
        ("/goal a punchier kick", Effect::Goal),
        ("/loop", Effect::Loop),
        ("/loop match the reference", Effect::Loop),
        ("/undo", Effect::Undo),
        ("/copy", Effect::Copy),
        ("/export", Effect::Export { words: String::new() }),
        ("/export md", Effect::Export { words: "md".into() }),
        ("/share", Effect::Share),
        ("/willington", Effect::Willington),
        ("/refresh", Effect::Refresh),
        ("/new", Effect::New { name: String::new() }),
        ("/new Bassline ideas", Effect::New { name: "Bassline ideas".into() }),
        ("/reconnect", Effect::Reconnect),
    ];
    for (line, effect) in cases {
        assert_eq!(run(line, &now), effect, "{line}");
    }
}

#[test]
fn commands_that_take_no_words_with_words_are_none_of_kumis() {
    // As before: only the commands that take words read them; /undo please is no command Kumi has.
    for line in ["/undo please", "/quit now", "/status of things", "/recipesx"] {
        let name = line.split_whitespace().next().unwrap();
        assert_eq!(run(line, &everything()), Effect::Unknown { name: name.into() }, "{line}");
    }
}

#[test]
fn goal_and_loop_control_words_run_in_any_case_with_punctuation() {
    for (line, effect) in [
        ("/goal pause", Effect::PauseGoal),
        ("/goal Hold.", Effect::PauseGoal),
        ("/loop pause!", Effect::PauseLoop),
        ("/goal stop", Effect::StopGoal),
        ("/goal Done.", Effect::StopGoal),
        ("/goal clear", Effect::StopGoal),
        ("/loop stop", Effect::StopLoop),
        ("/loop END", Effect::StopLoop),
    ] {
        assert_eq!(run(line, &everything()), effect, "{line}");
        // They run while Kumi works too.
        assert_eq!(run(line, &busy()), effect, "{line}, busy");
    }
    // Without a way to stop a goal, its stop words are a goal like any other.
    let no_stop = Now { can: Capabilities { stop_goal: false, ..everything().can }, ..everything() };
    assert_eq!(run("/goal stop", &no_stop), Effect::Goal);
}

#[test]
fn while_kumi_works_the_rest_wait_for_it_and_these_run() {
    let now = busy();
    for line in [
        "/quit",
        "/help",
        "/jobs",
        "/model",
        "/memory",
        "/resume",
        "/rename x",
        "/fork",
        "/recipe tidy",
        "/note p3 x",
        "/stop",
        "/daw",
        "/update",
        "/slots",
        "/music",
        "/map",
        "/palette",
        "/status",
        "/btw why",
    ] {
        assert!(!matches!(run(line, &now), Effect::Refused(_)), "{line} runs while Kumi works");
    }
    for line in
        ["/goal a punchier kick", "/loop again", "/undo", "/copy", "/export md", "/share", "/willington", "/refresh", "/new", "/reconnect"]
    {
        let verb = line.split_whitespace().next().unwrap();
        assert_eq!(run(line, &now), Effect::Refused(Why::Busy { verb: verb.into() }), "{line}");
    }
    // /goal alone shows the goal at work; with none, it waits like the rest.
    assert_eq!(run("/goal", &Now { goal_running: true, ..busy() }), Effect::ShowGoal);
    assert_eq!(run("/goal", &now), Effect::Refused(Why::Busy { verb: "/goal".into() }));
}

#[test]
fn a_command_this_session_has_not_reads_as_unknown_idle_and_as_waiting_busy() {
    // Today's way, kept: what isn't offered falls through to the end, so it's unknown while idle, and while Kumi works
    // it's one to run once it's done. A chat that keeps no conversations says so either way.
    for line in ["/jobs", "/model", "/memory", "/stop", "/slots", "/goal fix it", "/reconnect", "/willington", "/daw"] {
        let verb = line.split_whitespace().next().unwrap();
        assert_eq!(run(line, &nothing()), Effect::Unknown { name: verb.into() }, "{line}");
        assert_eq!(run(line, &Now { busy: true, ..nothing() }), Effect::Refused(Why::Busy { verb: verb.into() }), "{line}, busy");
    }
    for line in ["/resume", "/conversations x", "/rename x", "/fork"] {
        assert_eq!(run(line, &nothing()), Effect::Refused(Why::NotKept), "{line}");
        assert_eq!(run(line, &Now { busy: true, ..nothing() }), Effect::Refused(Why::NotKept), "{line}, busy");
    }
}

#[test]
fn goal_is_offered_only_where_kumi_keeps_goals() {
    // #636: the menu offered /goal where a line said there was no such command.
    let goal = named("/goal").unwrap();
    assert!(goal.offered(&everything().can, &everything().fronts));
    assert!(!goal.offered(&Capabilities::default(), &Fronts::default()));
    assert_eq!(run("/goal a punchier kick", &nothing()), Effect::Unknown { name: "/goal".into() });
}

fn names(found: Vec<Found>) -> Vec<&'static str> {
    found.into_iter().map(|found| found.command.name).collect()
}

#[test]
fn the_menu_finds_a_command_by_its_name_then_by_the_words_of_what_it_does() {
    let all = |_: &kumi_agent::core::commands::Command| true;
    // A name first, from its first letter; the words of what one does from the third.
    assert_eq!(names(matching("/s", &[], all)), ["/stop", "/share", "/slots", "/status"]);
    assert_eq!(names(matching("/hist", &[], all)), ["/undo"]);
    assert_eq!(names(matching("/sign", &[], all)), ["/login", "/logout"]);
    // Names before words: /res names /resume, and "restart"-like words come after.
    assert_eq!(names(matching("/res", &[], all))[0], "/resume");
    // One typed in full comes first, even when it begins a longer one.
    assert_eq!(names(matching("/recipe", &[], all)), ["/recipe", "/recipes"]);
    // Every command for a bare /, in the table's order.
    assert_eq!(names(matching("/", &[], all)), COMMANDS.iter().map(|command| command.name).collect::<Vec<_>>());
}

#[test]
fn the_menu_lists_the_commands_used_lately_first() {
    let all = |_: &kumi_agent::core::commands::Command| true;
    let found = names(matching("/", &["/status", "/undo"], all));
    assert_eq!(found[..2], ["/status", "/undo"]);
    assert_eq!(found.len(), COMMANDS.len());
    // Within what's found: /re lists /refresh, used lately, before /resume.
    assert_eq!(names(matching("/re", &["/refresh"], all))[0], "/refresh");
    // A name typed in full still comes first.
    assert_eq!(names(matching("/recipe", &["/recipes"], all))[0], "/recipe");
}

#[test]
fn the_menu_offers_only_what_this_session_and_front_end_have() {
    let now = nothing();
    let found = names(matching("/", &[], |command| command.offered(&now.can, &now.fronts)));
    assert!(found.contains(&"/undo") && found.contains(&"/help"));
    for absent in ["/jobs", "/model", "/memory", "/goal", "/slots", "/voice", "/resume", "/conversations"] {
        assert!(!found.contains(&absent), "{absent}");
    }
}

#[test]
fn the_menu_says_how_it_found_each_command_so_enter_runs_only_what_was_named() {
    let all = |_: &kumi_agent::core::commands::Command| true;
    let by =
        |typed: &str| matching(typed, &[], all).into_iter().map(|found| (found.command.name, found.by, found.named())).collect::<Vec<_>>();
    assert_eq!(by("/undo"), [("/undo", By::Typed, true)]);
    assert_eq!(by("/und"), [("/undo", By::Name, true)]);
    // Found by what it does, not by what was typed: Enter mustn't run it unchosen (/history isn't /undo).
    assert_eq!(by("/history"), [("/undo", By::Words, false)]);
    assert_eq!(by("/speed"), [("/fast", By::Words, false)]);
}

#[test]
fn common_words_find_no_command() {
    let all = |_: &kumi_agent::core::commands::Command| true;
    for typed in ["/the", "/and", "/your", "/this", "/with", "/kumi", "/what", "/then"] {
        assert!(matching(typed, &[], all).is_empty(), "{typed}: {:?}", names(matching(typed, &[], all)));
    }
    // A word that starts like a common one still finds what says it: /thi finds /effort ("thinks").
    assert_eq!(names(matching("/thi", &[], all)), ["/effort"]);
}
