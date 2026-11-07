//! What a test logged through `tracing`, captured one way for every
//! integration test binary that reads its own log lines (R52; the user's
//! brief of 2026-11-07, step B). Included with
//! `#[path = "common/capture.rs"] mod capture;`.
//!
//! The model is the lib tests' `captured()` in `src/tick.rs` (a22c998): one
//! global subscriber, installed once, whose writer appends each event to the
//! buffer of the thread that emitted it while [`captured`] runs there, and
//! drops it otherwise. A scoped subscriber per test
//! (`tracing::subscriber::set_default`) lost lines while the binary's other
//! tests ran beside it: tick.rs's burst test lost every R33 line in 6 to 9 of
//! 40 parallel runs of the lib tests, and r33_cadence's late-submit test lost
//! its R21 line on main's run 42246047760 (attempt 2), with the engine's
//! counts right (R52). The mechanism inside tracing is not proven. With one
//! global subscriber no scoped dispatcher comes and goes during a run, so the
//! pattern is gone.
//!
//! Only the capturing thread's events are kept, as with the scoped
//! subscriber: a `#[tokio::test]` body is polled by the runtime's `block_on`
//! on the test's own thread, so awaiting [`captured`] there keeps what the
//! body and everything it awaits log, and nothing a spawned task logs.

use std::cell::RefCell;
use std::future::Future;
use std::sync::Once;

thread_local! {
    /// This thread's capture, while [`captured`] runs on it.
    static CAPTURE: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
}

/// The one subscriber's writer: each event goes to the buffer of the thread
/// that emitted it while [`captured`] runs there, and is dropped otherwise.
struct ThreadBuffer;

impl std::io::Write for ThreadBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        CAPTURE.with(|capture| {
            if let Some(log) = capture.borrow_mut().as_mut() {
                log.extend_from_slice(buf);
            }
        });
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Install this binary's one subscriber, once. The interest cache is rebuilt
/// after the install, so a callsite registered before it sees the subscriber.
fn install() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let subscriber = tracing_subscriber::fmt()
            .with_writer(|| ThreadBuffer)
            .with_ansi(false)
            .finish();
        tracing::subscriber::set_global_default(subscriber)
            .expect("this test binary's only subscriber");
        tracing::callsite::rebuild_interest_cache();
    });
}

/// Run `f` to completion; return its output and what it logged on this
/// thread, as text. Fails loudly if `f` ends on another thread than it
/// started on, since its log would then be split between two buffers.
pub async fn captured<T>(f: impl Future<Output = T>) -> (T, String) {
    install();
    let thread = std::thread::current().id();
    CAPTURE.with(|capture| {
        let open = capture.borrow_mut().replace(Vec::new());
        assert!(open.is_none(), "a capture is already open on this thread");
    });
    let out = f.await;
    assert_eq!(
        std::thread::current().id(),
        thread,
        "the capture moved threads, so its log is split"
    );
    let bytes = CAPTURE.with(|capture| capture.borrow_mut().take());
    (
        out,
        String::from_utf8(bytes.unwrap_or_default()).expect("a UTF-8 log"),
    )
}
