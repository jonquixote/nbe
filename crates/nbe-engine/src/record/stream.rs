//! Stream session glue (Prompt 10 WU4, SPEC §16.14).
//!
//! Lifecycle owner between the directive path and the (WU5) publisher:
//! `stream.start` opens a [`StreamSession`], `stream.stop` / `show.stop`
//! quiescence closes it BEFORE `apply()` emits the ack (SPEC §5.9.5: the ack
//! is honest only after the effect is real) — the `record.stop`
//! `stop_and_finish` shape, mirrored.
//!
//! ## Scope (WU4)
//!
//! Session bookkeeping ONLY — no transport, no publisher, no encoder session.
//! The live streaming objects arrive in WU5; this side holds only the publish
//! target + the published selection, all `Send`, so engine state can hold it.
//! The stream holds no surface pool: pool ownership (one pool or two, who
//! sizes it) is WU5's G1 decision, and a pool built here with nothing to draw
//! into it would be ~25 MiB of VRAM held for no take.
//!
//! ## Defined behavior
//!
//! * [`set_force_no_chain`] forces the chain-less path exactly as a machine
//!   with no zero-copy chain behaves — the mirror of record's
//!   `set_force_no_encoder`. There is deliberately no force-*available* seam:
//!   a test that needs a chain uses a machine with one (or skips loudly).
//! * [`set_force_close_error`] injects a teardown failure so the
//!   stop-withholds-ack path is testable without a real transport (record's
//!   equivalent sabotages the sidecar on disk; a stub session has no disk
//!   surface to sabotage).
//! * A second `stream.start` while `Live` is refused upstream with
//!   `E_FORBIDDEN_STATE` and preserves the live session (§9.1: exactly one
//!   live stream).
//! * Closing with the force seam armed fails loudly (`E_NETWORK`) and still
//!   ends the live state — the record shape (no pipeline remains to continue
//!   with) — but the ack is withheld: `apply()` only acks on `Ok`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::record::tap_path::Selection;

/// Forced-unavailable seam (tests only): when set, [`chain_available`]
/// reports no chain without touching the GPU — the mirror of record's
/// `FORCE_NO_ENCODER`.
static FORCE_NO_CHAIN: AtomicBool = AtomicBool::new(false);

/// Force (or release) the chain-less path. Test seam only.
pub fn set_force_no_chain(force: bool) {
    FORCE_NO_CHAIN.store(force, Ordering::SeqCst);
}

/// The seam state: true while forced chain-less.
pub fn force_no_chain() -> bool {
    FORCE_NO_CHAIN.load(Ordering::SeqCst)
}

/// Forced-teardown-failure seam (tests only): when set, [`StreamSession`]'s
/// close reports [`StreamError::Teardown`] instead of closing.
static FORCE_CLOSE_ERROR: AtomicBool = AtomicBool::new(false);

/// Force (or release) the teardown-failure path. Test seam only.
pub fn set_force_close_error(force: bool) {
    FORCE_CLOSE_ERROR.store(force, Ordering::SeqCst);
}

/// Whether this machine has a lawful streaming chain (SPEC §0.1 assumption 24
/// as rescoped by v0.4.2: recording alone holds the readback allowance, so a
/// streaming consumer with no zero-copy chain has no lawful path and
/// [`crate::record::tap_path::select_stream`] answers `None`).
///
/// The probe is honest: it builds the take geometry's pool against the device
/// the render loop published and keeps nothing (WU5 owns the take's pool —
/// see the module docs). A `None` device — a headless engine, or a build
/// where the render loop has not run — is a machine with no chain.
pub fn chain_available(device: &Option<Arc<wgpu::Device>>) -> bool {
    if FORCE_NO_CHAIN.load(Ordering::SeqCst) {
        return false;
    }
    let Some(d) = device else {
        return false;
    };
    crate::record::zerocopy_pool(d, crate::render::VIEW_W, crate::render::VIEW_H).is_ok()
}

/// Opening or closing a stream fails loudly, with stable tokens.
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    /// No zero-copy chain (seam-forced or genuinely absent): `E_NO_ZEROCOPY`.
    #[error("E_NO_ZEROCOPY: {0}")]
    NoChain(String),
    /// Teardown failed (seam-injected until the WU5 transport lands):
    /// `E_NETWORK`.
    #[error("E_NETWORK: {0}")]
    Teardown(String),
}

/// A live stream: the publish target and the published frame-path selection.
/// Deliberately `Send` (plain data) so engine state can hold it; the `!Send`
/// half (when WU5 lands it) lives on the publisher task.
pub struct StreamSession {
    endpoint: String,
    selection: Selection,
    closed: bool,
}

impl StreamSession {
    /// Open a session on `endpoint` with the probed `selection`. No I/O: the
    /// transport dials in WU5; WU4 owns the bookkeeping the ack must wait for.
    pub fn open(endpoint: impl Into<String>, selection: Selection) -> Self {
        Self {
            endpoint: endpoint.into(),
            selection,
            closed: false,
        }
    }

    /// The publish target this session was opened on.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// The frame-path selection probed at start.
    pub fn selection(&self) -> Selection {
        self.selection
    }

    /// True once the engine closed the session (graceful close ran, or the
    /// force path dropped it).
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Close the session gracefully: the transport is gone BEFORE this returns
    /// (WU5) — so the ack that follows is honest. With the close-error seam
    /// armed this reports [`StreamError::Teardown`] and closes nothing.
    pub fn stop_and_close(&mut self) -> Result<(), StreamError> {
        if FORCE_CLOSE_ERROR.load(Ordering::SeqCst) {
            return Err(StreamError::Teardown(
                "stream teardown failed (injected): transport did not confirm shutdown".into(),
            ));
        }
        self.closed = true;
        Ok(())
    }

    /// Drop the session WITHOUT a graceful close (`force=true` immediate
    /// stop): the transport is abandoned as-is, the session over.
    pub fn abandon(&mut self) {
        self.closed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_seam_reports_chain_less_without_touching_hardware() {
        set_force_no_chain(true);
        assert!(!chain_available(&None));
        set_force_no_chain(false);
    }

    #[test]
    fn close_error_seam_fails_loudly_with_the_network_token() {
        let sel = crate::record::tap_path::select_stream(true).unwrap();
        let mut s = StreamSession::open("rtmp://example/live", sel);
        set_force_close_error(true);
        let err = s.stop_and_close().expect_err("armed seam must fail");
        assert!(err.to_string().contains("E_NETWORK"));
        assert!(!s.is_closed(), "failed close closes nothing");
        set_force_close_error(false);
        s.stop_and_close().expect("released seam must close");
        assert!(s.is_closed());
    }
}
