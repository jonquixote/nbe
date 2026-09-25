//! Which path a recorded frame takes from the compositor to the encoder.
//!
//! ZERO-COPY Phase 2. Phase 1 proved the chain and measured it
//! (`docs/09-measurements.md`, ZERO-COPY section): at 1080p30 the zero-copy tap
//! costs mean 4.033 ms / p95 6.044 ms against CPU readback's 12.1 / 19.3, and at
//! 4K it runs p95 17.6 ms where readback alone is ~48 ms and exceeds the
//! 33.333 ms budget by itself.
//!
//! **Selection is a published table, not a runtime dial.** The table is the
//! product decision; the code below is only its evaluator. A dial would make
//! every deployment's frame path an operational accident, and the one thing the
//! §0.1 assumption 24 allowance cannot survive is a path nobody can predict.
//!
//! The CPU path is not deprecated by any of this. It is the portability floor
//! (no IOSurface off Apple platforms), the test seam (every existing record test
//! drives it unmodified), and the fallback when the probe says no.

use std::fmt;

/// The path a recorded frame takes to the encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapPath {
    /// Compositor and encoder share one IOSurface; nothing is copied.
    ZeroCopy,
    /// The View is read back to CPU memory and handed to the encoder as bytes.
    CpuReadback,
}

impl TapPath {
    /// The stable token this path reports as, on the wire and in logs.
    pub fn as_str(self) -> &'static str {
        match self {
            TapPath::ZeroCopy => "zeroCopy",
            TapPath::CpuReadback => "cpuReadback",
        }
    }
}

impl fmt::Display for TapPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Who is consuming the frames. The axis exists because the answer differs by
/// consumer, and because streaming's answer is already fixed by the spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consumer {
    /// Recording to disk.
    Record,
    /// Streaming (Prompt 10, live). Its only lawful path is [`TapPath::ZeroCopy`]:
    /// recording's v0.4.2 readback allowance does not extend here, so a
    /// `CpuReadback` override for this consumer is refused on the
    /// `E_NO_ZEROCOPY` path (see [`select_with_override`]) — never Override-live.
    Stream,
}

/// Why a path was chosen. Carried with the choice because "which path is live"
/// is useless to an operator without "and why" — a silent fallback to CPU on a
/// machine that should have managed zero-copy is exactly the event worth seeing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The published table's cell for this capability × resolution × consumer.
    Table,
    /// The probe reported no usable zero-copy chain; CPU is the fallback.
    ProbeUnavailable,
    /// An operator override was set for this output. The escape hatch.
    Override,
}

/// A selection: the path, and the reason it was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub path: TapPath,
    pub reason: Reason,
}

/// Above this height a frame is "large" for the purposes of the table. 1080p is
/// the reference geometry (`docs/hardware-baseline.txt`); anything taller is
/// where readback's linear scaling stops fitting the budget.
pub const REFERENCE_HEIGHT: u32 = 1080;

/// Evaluate the published selection table.
///
/// | zero-copy capable | height | consumer | path |
/// |---|---|---|---|
/// | no  | any      | record | CpuReadback (ProbeUnavailable) |
/// | no  | any      | stream | **refused** — see [`select_stream`] |
/// | yes | ≤ 1080   | record | ZeroCopy (Table) |
/// | yes | > 1080   | record | ZeroCopy (Table) |
/// | yes | any      | stream | ZeroCopy (Table) |
///
/// The record rows are both ZeroCopy where the probe allows it, and they are
/// listed separately on purpose: the *reasons* differ in strength. At ≤ 1080
/// either path fits the budget and zero-copy is chosen because it is three times
/// cheaper; above 1080 readback does not fit at all, so the row is not a
/// preference but a requirement. Collapsing them would hide that.
pub fn select(zero_copy_capable: bool, height: u32, consumer: Consumer) -> Selection {
    if !zero_copy_capable {
        return Selection {
            path: TapPath::CpuReadback,
            reason: Reason::ProbeUnavailable,
        };
    }
    let _ = height; // the capable rows agree today; see the doc table above
    let _ = consumer;
    Selection {
        path: TapPath::ZeroCopy,
        reason: Reason::Table,
    }
}

/// Streaming's row, separated because its answer is not a measurement.
///
/// SPEC §0.1 assumption 24 says outputs share frames with encoders **without CPU
/// readback**. v0.4.2 granted one scoped allowance and it covers the *recording*
/// output only — "Streaming (Prompt 10) inherits no allowance from this row."
/// So a streaming consumer on a machine with no zero-copy chain has no lawful
/// path, and this returns `None` rather than quietly handing it the readback the
/// spec forbids.
///
/// Called by the `stream.start` gate (and the tests pinning the refusal row),
/// so Prompt 10 arrives to a decision already made instead of finding the tap
/// defaulted to the path it is forbidden from.
pub fn select_stream(zero_copy_capable: bool) -> Option<Selection> {
    zero_copy_capable.then_some(Selection {
        path: TapPath::ZeroCopy,
        reason: Reason::Table,
    })
}

/// Apply an operator override on top of the table.
///
/// The escape hatch for the day the probe is wrong — a machine where the chain
/// links but produces garbage, say. It is documented as an escape hatch and not
/// a feature: an override that becomes routine means the table is wrong, and
/// the fix is the table.
///
/// Stream contract: `CpuReadback` for [`Consumer::Stream`] is unlawful (no
/// readback allowance covers streaming), so the `stream.start` call site refuses
/// that combination on the `E_NO_ZEROCOPY` path BEFORE calling here — this
/// function must never produce an Override-live stream selection, and no caller
/// hands it that combination.
pub fn select_with_override(
    zero_copy_capable: bool,
    height: u32,
    consumer: Consumer,
    override_path: Option<TapPath>,
) -> Selection {
    match override_path {
        // An override to ZeroCopy on a machine whose probe failed is refused:
        // the override can restrict, never conjure a capability that is absent.
        Some(TapPath::ZeroCopy) if !zero_copy_capable => Selection {
            path: TapPath::CpuReadback,
            reason: Reason::ProbeUnavailable,
        },
        Some(path) => Selection {
            path,
            reason: Reason::Override,
        },
        None => select(zero_copy_capable, height, consumer),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_probe_falls_back_to_cpu_with_the_reason_recorded() {
        let s = select(false, 1080, Consumer::Record);
        assert_eq!(s.path, TapPath::CpuReadback);
        assert_eq!(s.reason, Reason::ProbeUnavailable);
    }

    #[test]
    fn streaming_has_no_lawful_path_without_zero_copy() {
        // §0.1 assumption 24 + v0.4.2: the allowance is recording-only.
        assert!(select_stream(false).is_none());
        assert_eq!(select_stream(true).unwrap().path, TapPath::ZeroCopy);
    }

    #[test]
    fn an_override_cannot_conjure_a_capability_the_probe_denied() {
        let s = select_with_override(false, 1080, Consumer::Record, Some(TapPath::ZeroCopy));
        assert_eq!(s.path, TapPath::CpuReadback);
        assert_eq!(s.reason, Reason::ProbeUnavailable);
    }

    #[test]
    fn an_override_to_cpu_is_honoured_and_says_so() {
        let s = select_with_override(true, 2160, Consumer::Record, Some(TapPath::CpuReadback));
        assert_eq!(s.path, TapPath::CpuReadback);
        assert_eq!(s.reason, Reason::Override);
    }

    #[test]
    fn path_tokens_are_stable() {
        assert_eq!(TapPath::ZeroCopy.as_str(), "zeroCopy");
        assert_eq!(TapPath::CpuReadback.as_str(), "cpuReadback");
    }

    /// The sibling of `path_tokens_are_stable`, for the reason.
    ///
    /// **`"Table"`, `"ProbeUnavailable"` and `"Override"` are normative wire
    /// tokens as of SPEC v0.4.4** (§10.1's record-tap note names all three).
    /// `TapPath` has an explicit [`TapPath::as_str`] and the test above;
    /// `Reason` has no such map — `telemetry::build_tick` renders it with
    /// `format!("{:?}", reason)`, so **renaming a variant is a silent wire
    /// change**. PR #27's two-key pass found `"Override"` named by the spec and
    /// asserted as a string nowhere.
    ///
    /// Shape chosen deliberately: this pins the `Debug` rendering rather than
    /// introducing a `Reason::as_str()`. An `as_str` telemetry does not call
    /// would be a second spelling of the same token and a guard standing beside
    /// the path instead of on it — §2a rule 8's shape. What goes on the wire is
    /// `format!("{:?}")`, so that is what is pinned, and the reasons come from
    /// the real evaluators rather than from named variants, so the test also
    /// proves each one is reachable.
    #[test]
    fn reason_tokens_are_stable() {
        let table = select(true, 1080, Consumer::Record).reason;
        let unavailable = select(false, 1080, Consumer::Record).reason;
        let overridden =
            select_with_override(true, 2160, Consumer::Record, Some(TapPath::CpuReadback)).reason;

        assert_eq!(format!("{table:?}"), "Table");
        assert_eq!(format!("{unavailable:?}"), "ProbeUnavailable");
        assert_eq!(format!("{overridden:?}"), "Override");

        // And the token that reaches the WIRE is this one. `Override` is the
        // only reason no production path can produce today — the escape hatch
        // is wired to no config surface — so this is the one place the spec's
        // third token is checked against a real tick. `Table` and
        // `ProbeUnavailable` are additionally covered end to end through
        // `record.start` by the `zerocopy_migration` suite.
        let state = crate::state::EngineState::new(30);
        *state.record_tap_selection.lock().unwrap() = Some(Selection {
            path: TapPath::CpuReadback,
            reason: overridden,
        });
        let frame = crate::telemetry::build_tick(&state);
        let nbe_protocol::EngineFrame::EngineTelemetry { fields, .. } = frame else {
            panic!("build_tick must produce telemetry");
        };
        assert_eq!(
            fields.record_tap_reason, "Override",
            "the spec names `\"Override\"` as a §10.1 token; the wire must spell it that way"
        );
    }
}
