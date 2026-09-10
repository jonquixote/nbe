//! SPEC §12.4's budget table is the source; `loop_cache`'s constants mirror it,
//! and nothing outside `loop_cache` builds a budget of its own.
//!
//! The numbers were held in two places with different values — preflight had
//! §12.4's 256/512, the engine a Prompt 05 placeholder of 1024/4096 — so one
//! shared planner gave two answers about which loops are resident. Parsing the
//! table makes the spec the fixture, the same discipline `nbe-protocol`'s
//! mirror audit applies to the §16 command surface: a table edit the constants
//! do not follow fails here rather than at a customer's package.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    // crates/nbe-core -> repo root
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .to_path_buf()
}

#[test]
fn the_spec_12_4_budget_table_matches_the_constants() {
    let spec =
        std::fs::read_to_string(repo_root().join("docs/spec.v0.4.md")).expect("spec is readable");
    let section = spec
        .split("## 12.4 Budgets")
        .nth(1)
        .expect("§12.4 exists")
        .split("\n## ")
        .next()
        .expect("§12.4 ends");
    let value_of = |label: &str| -> u32 {
        let row = section
            .lines()
            .find(|l| l.starts_with('|') && l.contains(label))
            .unwrap_or_else(|| panic!("§12.4 has no row for {label:?}"));
        row.split('|')
            .nth(2)
            .expect("value cell")
            .split_whitespace()
            .next()
            .expect("value")
            .parse()
            .expect("value is a number")
    };
    assert_eq!(
        value_of("absolute short-loop frame cap"),
        nbe_core::loop_cache::ABSOLUTE_LOOP_FRAME_CAP
    );
    assert_eq!(
        value_of("default per-loop budget"),
        nbe_core::loop_cache::DEFAULT_PER_LOOP_MIB
    );
    assert_eq!(
        value_of("default total short-loop budget"),
        nbe_core::loop_cache::DEFAULT_TOTAL_LOOP_MIB
    );
}

#[test]
fn only_loop_cache_constructs_a_cache_budget_from_literals() {
    // The constants are single-source only if the struct is constructed in one
    // place. `CacheBudget::from_manifest` is that place; a struct literal
    // anywhere else is how 1024/4096 came to exist beside 256/512.
    //
    // `CacheBudget`'s fields are private now, so the compiler refuses a literal
    // in any other crate outright — and refuses the mutation this grep never
    // saw (`let mut b = …; b.per_loop_mib = 1024;`), which is why the fields
    // were closed rather than the lint widened. What remains for this test is
    // proliferation *inside* `nbe-core`, where privacy does not apply.
    let mut offenders = Vec::new();
    let mut stack = vec![repo_root().join("crates")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("crates is readable") {
            let path = entry.expect("readable entry").path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            // `loop_cache.rs` owns the struct, its constructor, and the
            // fixtures its own unit tests plan against. This file names the
            // pattern it searches for, so it matches itself.
            if path.ends_with("nbe-core/src/loop_cache.rs")
                || path.ends_with("nbe-core/tests/spec_budgets.rs")
            {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("source is readable");
            for (i, line) in text.lines().enumerate() {
                // `-> …CacheBudget {` opens a function body, not a value.
                let is_construction = line.contains("CacheBudget {")
                    && !line.contains("->")
                    && !line.trim_start().starts_with("//");
                if is_construction {
                    offenders.push(format!(
                        "{}:{}: {}",
                        path.strip_prefix(repo_root()).unwrap_or(&path).display(),
                        i + 1,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "build the budget with `CacheBudget::from_manifest`, so §12.4's numbers \
         stay in one place:\n  {}",
        offenders.join("\n  ")
    );
}
