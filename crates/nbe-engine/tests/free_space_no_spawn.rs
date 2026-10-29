//! R36's follow-up (the user's work order of 2026-10-29): the record target's
//! free space is read with one statvfs-style call, never by spawning a process.
//! Forking `df` from inside the engine measured 1.9-3.1 s, and `df` does not
//! exist on Windows. The proof: the read succeeds with PATH empty, where any
//! spawned command by name cannot be found. The test re-runs itself as a child
//! with PATH cleared, so nothing else in this process sees the change.

const CHILD: &str = "NBE_FREE_SPACE_NO_PATH_CHILD";

#[test]
fn free_space_is_read_without_spawning_a_process() {
    if std::env::var_os(CHILD).is_some() {
        // The child: no PATH. A `df` (or any command by name) cannot be found.
        let dir = tempfile::tempdir().expect("tempdir");
        let mib = nbe_engine::record::available_space_mib(dir.path())
            .expect("free space must be readable with no PATH");
        assert!(
            mib > 0.0,
            "free space on a real volume must be > 0, got {mib}"
        );
        return;
    }
    let out = std::process::Command::new(std::env::current_exe().expect("this test's binary"))
        .args([
            "--exact",
            "free_space_is_read_without_spawning_a_process",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("PATH", "")
        .output()
        .expect("re-run this test as a child");
    assert!(
        out.status.success(),
        "with PATH empty the free-space read failed, so it spawns a process:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
