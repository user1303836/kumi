use kumi_agent::core::context::ListeningPolicy;
use kumi_types::contracts::HearRequest;
use kumi_types::errors::HearFailure;
use std::path::Path;

fn request(path: &Path) -> HearRequest {
    HearRequest { scratch_dir: Some(path.into()), ..HearRequest::default() }
}

fn private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[tokio::test]
async fn restricted_capture_accepts_only_its_private_per_call_directory() {
    let root = tempfile::tempdir().unwrap();
    let call = tempfile::tempdir_in(root.path()).unwrap();
    let outside = tempfile::tempdir().unwrap();
    private(root.path());
    private(call.path());
    let policy = ListeningPolicy::Restricted { scratch_root: root.path().into(), quarantine_root: outside.path().join("quarantine") };
    assert_eq!(policy.scratch_dir(&request(call.path())).await.unwrap(), call.path().canonicalize().unwrap());
    for path in [root.path(), outside.path(), Path::new("relative-capture")] {
        assert!(matches!(policy.scratch_dir(&request(path)).await, Err(HearFailure::Refused { .. })));
    }
    assert!(matches!(policy.scratch_dir(&HearRequest::default()).await, Err(HearFailure::Refused { .. })));
    assert!(!root.path().join("relative-capture").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn restricted_capture_does_not_follow_or_repair_an_unsafe_scratch_directory() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let call = tempfile::tempdir_in(root.path()).unwrap();
    private(root.path());
    private(call.path());
    let linked = root.path().join("linked");
    symlink(call.path(), &linked).unwrap();
    let policy = ListeningPolicy::Restricted { scratch_root: root.path().into(), quarantine_root: root.path().join("quarantine") };
    assert!(matches!(policy.scratch_dir(&request(&linked)).await, Err(HearFailure::Refused { .. })));
    std::fs::set_permissions(call.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(policy.scratch_dir(&request(call.path())).await, Err(HearFailure::Refused { .. })));
    assert_eq!(std::fs::metadata(call.path()).unwrap().permissions().mode() & 0o777, 0o755);
    assert!(std::fs::symlink_metadata(&linked).unwrap().file_type().is_symlink());
}
