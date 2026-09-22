//! WU3 — `stream.start` url precedence (SPEC v0.4.5 §9.4, §16.14).
//!
//! The rule, resolved in code and guarded both directions:
//! * `stream.start`'s `url` OVERRIDES `outputs.stream.url` for the run;
//! * the manifest answers when the command is silent;
//! * NEITHER present is an `E_BAD_PAYLOAD`-shaped refusal.
//!
//! Pure-logic tests over [`nbe_engine::directive::resolve_stream_url`] —
//! hardware-free: no engine state, no encoder, no package on disk.
//!
//! Empty-string edge (decided here, pinned below): a missing, empty, or
//! whitespace-only `url` counts as SILENT, never as an endpoint — an empty
//! string must not become a valid publish target. So an empty command `url`
//! falls back to the manifest, and empty on both sides is a refusal. This
//! matches the record path's leniency (`outputId` filters empties and falls
//! back to `"episode"` in `on_record_start`).

use nbe_engine::directive::{resolve_stream_url, DirectiveError};

/// The refusal is `E_BAD_PAYLOAD`-shaped: the engine has no dedicated
/// `BadPayload` variant, so payload refusals surface as
/// `DirectiveError::Invalid` — the same convention as `marker.add` with a
/// missing name and `record.start` with no record directory. The message
/// carries the `E_BAD_PAYLOAD` token so the refusal is greppable.
fn is_bad_payload(err: &DirectiveError) -> bool {
    matches!(err, DirectiveError::Invalid(msg) if msg.contains("E_BAD_PAYLOAD"))
}

// (a) Command url wins when both present.
#[test]
fn command_url_overrides_manifest_url() {
    let got = resolve_stream_url(
        Some("rtmp://manifest.example/live"),
        Some("rtmp://command.example/live"),
    )
    .expect("both urls present must resolve");
    assert_eq!(
        got, "rtmp://command.example/live",
        "stream.start's url OVERRIDES outputs.stream.url for the run"
    );
}

// (b) Manifest url answers when the command is silent (absent).
#[test]
fn manifest_url_answers_when_command_silent() {
    let got = resolve_stream_url(Some("rtmp://manifest.example/live"), None)
        .expect("manifest url must answer a silent command");
    assert_eq!(got, "rtmp://manifest.example/live");
}

// (c) Neither present → E_BAD_PAYLOAD-shaped refusal.
#[test]
fn neither_url_refuses_bad_payload() {
    let err = resolve_stream_url(None, None).expect_err("neither url must refuse");
    assert!(
        is_bad_payload(&err),
        "neither url must be an E_BAD_PAYLOAD-shaped refusal, got: {err}"
    );
}

// Empty command url = silent: the manifest answers.
#[test]
fn empty_command_url_falls_back_to_manifest() {
    let got = resolve_stream_url(Some("rtmp://manifest.example/live"), Some(""))
        .expect("empty command url is silent, manifest must answer");
    assert_eq!(got, "rtmp://manifest.example/live");
}

// Whitespace-only command url = silent: the manifest answers.
#[test]
fn whitespace_command_url_falls_back_to_manifest() {
    let got = resolve_stream_url(Some("rtmp://manifest.example/live"), Some("   "))
        .expect("whitespace command url is silent, manifest must answer");
    assert_eq!(got, "rtmp://manifest.example/live");
}

// Empty manifest url never counts: silent command + empty manifest = refusal.
#[test]
fn empty_manifest_url_with_silent_command_refuses() {
    let err = resolve_stream_url(Some(""), None).expect_err("empty manifest url is no endpoint");
    assert!(
        is_bad_payload(&err),
        "empty manifest url must be an E_BAD_PAYLOAD-shaped refusal, got: {err}"
    );
}

// Both empty = refusal: an empty string must not become a valid endpoint.
#[test]
fn both_empty_refuses() {
    let err = resolve_stream_url(Some(""), Some("")).expect_err("empty urls must refuse");
    assert!(
        is_bad_payload(&err),
        "empty urls must be an E_BAD_PAYLOAD-shaped refusal, got: {err}"
    );
}

// Surrounding whitespace is trimmed: it is never meaningful in a URL.
#[test]
fn surrounding_whitespace_is_trimmed() {
    let got = resolve_stream_url(None, Some("  rtmp://command.example/live  "))
        .expect("padded command url must resolve");
    assert_eq!(got, "rtmp://command.example/live");
}

// Manifest-side symmetry: padding and blankness behave identically on the
// manifest input — the trim rule is per-input, not per-side.
#[test]
fn manifest_whitespace_url_is_trimmed() {
    let got = resolve_stream_url(Some("  rtmp://manifest.example/live  "), None)
        .expect("padded manifest url must resolve");
    assert_eq!(got, "rtmp://manifest.example/live");
}

#[test]
fn whitespace_only_manifest_falls_back_to_command() {
    let got = resolve_stream_url(Some("   "), Some("rtmp://command.example/live"))
        .expect("blank manifest must fall back, not refuse");
    assert_eq!(got, "rtmp://command.example/live");
}
