use kumi_library::plugins::formats::{fabfilter, structure, vstpreset::VstPreset};
use kumi_library::plugins::templates;

#[test]
fn every_fabfilter_initialized_state_and_controller_has_a_verified_layout() {
    let mut checked = 0;
    for format in structure::builtins().iter().filter(|format| format.info.family == "fabfilter") {
        let plugin = &format.info.plugin;
        let bytes = templates::initial(plugin).unwrap_or_else(|| panic!("{plugin}: missing initialized VST3"));
        let container = VstPreset::read(bytes).unwrap();
        assert!(format.has_id(&container.class_id), "{plugin}: wrong initialized class");
        let component = container.component().unwrap();
        assert!(fabfilter::is_component(component), "{plugin}: component envelope");
        let preset = fabfilter::FabFilterPreset::read(plugin, component).unwrap();
        assert_eq!(preset.write().unwrap(), component, "{plugin}: read changed bytes");
        assert_eq!(preset.to_state(component).unwrap(), component, "{plugin}: conversion changed bytes");
        if let Some(controller) = container.controller() {
            fabfilter::check_controller(plugin, controller, controller).unwrap();
            let renamed = fabfilter::rename_controller(plugin, controller, controller, "Kumi 测试").unwrap();
            fabfilter::check_controller(plugin, &renamed, controller).unwrap();
        }
        checked += 1;
    }
    assert!(checked >= 25, "all current and captured legacy FabFilter products must be represented");
}

#[test]
fn every_fabfilter_component_rejects_corrupt_headers_before_writing() {
    for format in structure::builtins().iter().filter(|format| format.info.family == "fabfilter") {
        let plugin = &format.info.plugin;
        let container = VstPreset::read(templates::initial(plugin).unwrap()).unwrap();
        let component = container.component().unwrap();
        let mut wrong_schema = component.to_vec();
        wrong_schema[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(fabfilter::FabFilterPreset::read(plugin, &wrong_schema).is_err(), "{plugin}: unknown schema accepted");
        let mut enormous_length = component.to_vec();
        enormous_length[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(fabfilter::FabFilterPreset::read(plugin, &enormous_length).is_err(), "{plugin}: oversized length accepted");
        for size in 0..12 {
            assert!(fabfilter::FabFilterPreset::read(plugin, &component[..size]).is_err(), "{plugin}: truncated header accepted");
        }
    }
}
