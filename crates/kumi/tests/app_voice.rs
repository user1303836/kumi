//! Voice settings and controls.
use kumi::{
    config::{read_settings, write_settings},
    voice::*,
};
use kumi_runtime::system::Env;
use serde_json::json;
use std::{cell::RefCell, rc::Rc};
#[test]
fn voice_preferences_stay_beside_the_model_and_invalid_settings_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("settings.json");
    let file = file.to_str().unwrap();
    write_settings(
        file,
        &json!({"model":"anthropic/claude-sonnet-5","voice":{"send":true,"language":"ja","microphone":"Scarlett 2i2 USB"}}),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(read_settings(file).voice).unwrap(),
        json!({"send":true,"language":"ja","microphone":"Scarlett 2i2 USB"})
    );
    write_settings(file, &json!({"model":"openai/gpt-6-luna"})).unwrap();
    assert_eq!(
        serde_json::to_value(read_settings(file)).unwrap(),
        json!({"model":"openai/gpt-6-luna","voice":{"send":true,"language":"ja","microphone":"Scarlett 2i2 USB"}})
    );
    std::fs::write(file, json!({"voice":{"send":"yes","language":"Klingon","microphone":"Mic\u{7}"}}).to_string()).unwrap();
    assert_eq!(serde_json::to_value(read_settings(file)).unwrap(), json!({}));
}
#[test]
fn computer_language_is_used_until_the_producer_chooses_then_choices_persist() {
    for (env, want) in [
        (Env::from([("LANG".into(), "ja_JP.UTF-8".into())]), "ja"),
        (Env::from([("LC_ALL".into(), "de_DE.UTF-8".into()), ("LANG".into(), "en_US.UTF-8".into())]), "de"),
        (Env::from([("LANG".into(), "C".into())]), "en"),
    ] {
        assert_eq!(system_language(&env), want)
    }
    for (tag, want) in
        [("ENG-US", "eng"), ("a_US", "en"), ("abcd_US", "en"), ("zh-Hant-TW", "zh"), ("ja!", "en"), ("en@variant", "en"), ("EN", "en")]
    {
        assert_eq!(system_language(&Env::from([("LANG".into(), tag.into())])), want)
    }
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("settings.json").display().to_string();
    let opened = Rc::new(RefCell::new(vec![]));
    let voice = create_voice_control(VoiceControlOptions {
        env: Some(Env::from([("LANG".into(), "ja_JP.UTF-8".into())])),
        tools_dir: dir.path().join("tools").display().to_string(),
        settings_file: file.clone(),
        open: Some(Rc::new({
            let opened = opened.clone();
            move |url| opened.borrow_mut().push(url.to_string())
        })),
        platform: Some("darwin".into()),
    });
    assert_eq!(voice.system_language(), "ja");
    assert_eq!(serde_json::to_value(voice.choices()).unwrap(), json!({"send":false,"language":"ja"}));
    voice
        .choose(VoiceChange { send: Some(true), language: Some("en".into()), microphone: Some(Some("MacBook Pro Microphone".into())) })
        .unwrap();
    assert_eq!(serde_json::to_value(voice.choices()).unwrap(), json!({"send":true,"language":"en","microphone":"MacBook Pro Microphone"}));
    voice.choose(VoiceChange { send: Some(false), microphone: Some(None), ..Default::default() }).unwrap();
    assert_eq!(serde_json::to_value(voice.choices()).unwrap(), json!({"send":false,"language":"en"}));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&file).unwrap()).unwrap(),
        json!({"voice":{"language":"en"}})
    );
    voice.open_privacy();
    assert_eq!(*opened.borrow(), ["x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"]);
    for (platform, want) in [("win32", true), ("linux", false)] {
        let control = create_voice_control(VoiceControlOptions {
            settings_file: file.clone(),
            open: Some(Rc::new(|_| {})),
            platform: Some(platform.into()),
            ..Default::default()
        });
        assert_eq!(control.has_privacy(), want)
    }
}
