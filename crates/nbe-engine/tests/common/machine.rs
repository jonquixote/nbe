//! Which bound a test that measures the machine asserts (#84; the second key's
//! ruling of 2026-11-04, night). One copy, included by each test binary that
//! re-scopes a bound with `#[path = "common/machine.rs"] mod machine;`, and
//! pinned by `tests/normative_machine.rs`.

/// `true`: assert the strict bound, the normative machine's. Every environment
/// is strict except `GITHUB_ACTIONS` set to exactly `true`, which GitHub
/// Actions sets on its runners (the dress rehearsal's `whichMachine` reads the
/// same variable the same way). Both unset is strict; `CI` alone, `CI=1`, or any
/// other combination is strict too: an unidentified machine gets the strict
/// bound, never the runner's relief and never a panic. `CI` is not read, since
/// it identifies no runner.
pub fn on_the_normative_machine() -> bool {
    std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
}
