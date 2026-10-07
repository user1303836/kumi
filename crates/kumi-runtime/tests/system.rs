use kumi_runtime::system::{system_program, windows_device_name, Env, SystemProgram};

fn env(pairs: &[(&str, &str)]) -> Env {
    pairs.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect()
}

#[test]
fn the_names_windows_takes_for_its_devices_are_known_with_any_extension() {
    for name in ["CON", "aux.json", "Aux.amxd", "nul.tar.gz", "Prn ", "AUX .amxd", "com1.json", "LPT9", "COM0", "lpt\u{b9}", "COM\u{b3}.x"]
    {
        assert!(windows_device_name(name), "{name}");
    }
    for name in ["console", "auxiliary.json", "con-1", "com10", "lpt", "COM", "Aux 2", " aux", "x.aux", "", "com\u{b4}", "CO\u{b9}"] {
        assert!(!windows_device_name(name), "{name}");
    }
}

#[test]
fn on_windows_windows_own_programs_are_run_by_their_full_path_wherever_windows_is_elsewhere_by_name() {
    assert_eq!(system_program(SystemProgram::Tar, &env(&[("SystemRoot", "D:\\Windows")]), "win32"), "D:\\Windows\\System32\\tar.exe");
    assert_eq!(
        system_program(SystemProgram::Tasklist, &env(&[("SYSTEMROOT", "C:\\WINDOWS")]), "win32"),
        "C:\\WINDOWS\\System32\\tasklist.exe"
    );
    assert_eq!(
        system_program(SystemProgram::Powershell, &env(&[]), "win32"),
        "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
    );
    assert_eq!(system_program(SystemProgram::Tar, &env(&[("SystemRoot", "C:\\Windows")]), "darwin"), "tar");
    assert_eq!(system_program(SystemProgram::Tar, &env(&[]), "linux"), "tar");
}
