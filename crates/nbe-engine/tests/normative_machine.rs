//! #84's strict helper, `tests/common/machine.rs` (the second key's ruling of
//! 2026-11-04, night): strict for every environment except
//! `GITHUB_ACTIONS=true`, and no panic. Each combination runs the helper in a
//! child process of this binary, so the real environment is read and no test
//! here changes its own.

use std::process::Command;

#[path = "common/machine.rs"]
mod machine;
use machine::on_the_normative_machine;

/// The child's half: report what the helper decided. A no-op unless the parent
/// below spawned this binary for it.
#[test]
fn child_reports_the_machine() {
    if std::env::var_os("NBE_MACHINE_CHILD").is_none() {
        return;
    }
    println!("MACHINE strict={}", on_the_normative_machine());
}

#[test]
fn every_environment_but_a_runner_is_strict_and_none_panics() {
    // (CI, GITHUB_ACTIONS, strict): `None` is unset.
    let cases: &[(Option<&str>, Option<&str>, bool)] = &[
        (None, None, true),
        (Some("1"), None, true),
        (Some("true"), None, true),
        (None, Some(""), true),
        (None, Some("false"), true),
        (None, Some("1"), true),
        (None, Some("TRUE"), true),
        (None, Some("true "), true),
        (Some("true"), Some("yes"), true),
        (None, Some("true"), false),
        (Some("true"), Some("true"), false),
        (Some("1"), Some("true"), false),
    ];
    let me = std::env::current_exe().expect("this test binary");
    for &(ci, actions, strict) in cases {
        let mut child = Command::new(&me);
        child
            .args(["--exact", "child_reports_the_machine", "--nocapture"])
            .env_remove("CI")
            .env_remove("GITHUB_ACTIONS")
            .env("NBE_MACHINE_CHILD", "1");
        if let Some(v) = ci {
            child.env("CI", v);
        }
        if let Some(v) = actions {
            child.env("GITHUB_ACTIONS", v);
        }
        let out = child.output().expect("spawn this test binary");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "CI={ci:?} GITHUB_ACTIONS={actions:?}: the helper failed or panicked:\n{stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            stdout.contains(&format!("MACHINE strict={strict}")),
            "CI={ci:?} GITHUB_ACTIONS={actions:?}: expected strict={strict}:\n{stdout}"
        );
    }
}
