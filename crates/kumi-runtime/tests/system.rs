use kumi_runtime::system::{system_program, Env, SystemProgram};

fn env(pairs: &[(&str, &str)]) -> Env {
    pairs.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect()
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
