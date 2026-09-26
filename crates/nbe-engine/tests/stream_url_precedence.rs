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
//!
//! Complete-target rule (PR #33's fix round): the winner must PARSE as
//! `rtmp://host[:port]/app/key` — the publisher's own parser, run at resolve
//! time. A keyless or malformed endpoint refuses `E_BAD_PAYLOAD` here, naming
//! the parser's reason, instead of opening a session with no publisher while
//! `streamState` goes live (the false-live v0.4.6's `streamTransportState`
//! surfaced). The fixtures below gained a `/key` for that reason: every
//! "valid" URL in this file was keyless — ~~`rtmp://manifest.example/live`~~ —
//! and resolved only because the old rule checked the scheme alone (§2c).
//! Their empty-string and precedence logic is unchanged.

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
        Some("rtmp://manifest.example/live/key"),
        Some("rtmp://command.example/live/key"),
    )
    .expect("both urls present must resolve");
    assert_eq!(
        got, "rtmp://command.example/live/key",
        "stream.start's url OVERRIDES outputs.stream.url for the run"
    );
}

// (b) Manifest url answers when the command is silent (absent).
#[test]
fn manifest_url_answers_when_command_silent() {
    let got = resolve_stream_url(Some("rtmp://manifest.example/live/key"), None)
        .expect("manifest url must answer a silent command");
    assert_eq!(got, "rtmp://manifest.example/live/key");
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
    let got = resolve_stream_url(Some("rtmp://manifest.example/live/key"), Some(""))
        .expect("empty command url is silent, manifest must answer");
    assert_eq!(got, "rtmp://manifest.example/live/key");
}

// Whitespace-only command url = silent: the manifest answers.
#[test]
fn whitespace_command_url_falls_back_to_manifest() {
    let got = resolve_stream_url(Some("rtmp://manifest.example/live/key"), Some("   "))
        .expect("whitespace command url is silent, manifest must answer");
    assert_eq!(got, "rtmp://manifest.example/live/key");
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
    let got = resolve_stream_url(None, Some("  rtmp://command.example/live/key  "))
        .expect("padded command url must resolve");
    assert_eq!(got, "rtmp://command.example/live/key");
}

// Manifest-side symmetry: padding and blankness behave identically on the
// manifest input — the trim rule is per-input, not per-side.
#[test]
fn manifest_whitespace_url_is_trimmed() {
    let got = resolve_stream_url(Some("  rtmp://manifest.example/live/key  "), None)
        .expect("padded manifest url must resolve");
    assert_eq!(got, "rtmp://manifest.example/live/key");
}

#[test]
fn whitespace_only_manifest_falls_back_to_command() {
    let got = resolve_stream_url(Some("   "), Some("rtmp://command.example/live/key"))
        .expect("blank manifest must fall back, not refuse");
    assert_eq!(got, "rtmp://command.example/live/key");
}

// ---------------------------------------------------------------------------
// Complete-target rule (PR #33's fix round): resolution runs the full parse.
// ---------------------------------------------------------------------------

/// The false-live, refused at the source: a keyless `rtmp://` endpoint used to
/// resolve (scheme check only), open a session with no publisher, and let
/// `streamState` go live on a stream that published nothing. Refused here, on
/// both inputs, with the missing key named.
#[test]
fn a_keyless_rtmp_url_refuses_bad_payload_and_names_the_key() {
    for (manifest, command) in [
        (Some("rtmp://manifest.example/live"), None),
        (None, Some("rtmp://command.example/live")),
    ] {
        let err = resolve_stream_url(manifest, command)
            .expect_err("a keyless rtmp:// url is not a complete publish target");
        assert!(
            is_bad_payload(&err),
            "a keyless url must be an E_BAD_PAYLOAD-shaped refusal, got: {err}"
        );
        assert!(
            err.to_string().contains("no stream key"),
            "the refusal must name what is missing, got: {err}"
        );
    }
}

/// A malformed COMMAND url refuses; it never falls back to a good manifest
/// url — that would publish somewhere the operator did not name (the rule the
/// non-string `url` already follows in `on_stream_start`).
#[test]
fn a_malformed_command_url_refuses_rather_than_falling_back() {
    let err = resolve_stream_url(
        Some("rtmp://manifest.example/live/key"),
        Some("rtmp://command.example/live"),
    )
    .expect_err("a keyless command url must refuse, not fall back");
    assert!(is_bad_payload(&err), "got: {err}");
}

/// The rest of the parser's refusals reach the operator the same way: no
/// app/key path, no host, a bad port, an empty key segment.
#[test]
fn every_parser_refusal_is_bad_payload_at_resolve_time() {
    for url in [
        "rtmp://hostonly",
        "rtmp:///live/key",
        "rtmp://host:notaport/live/key",
        "rtmp://host/live/",
    ] {
        let err = resolve_stream_url(None, Some(url)).expect_err(url);
        assert!(
            is_bad_payload(&err),
            "{url} must be an E_BAD_PAYLOAD-shaped refusal, got: {err}"
        );
    }
}

/// A well-formed target still resolves, verbatim: explicit port, uppercase
/// scheme, and a key with slashes in it (the key is everything past the app).
#[test]
fn a_complete_publish_target_still_resolves_verbatim() {
    for url in [
        "rtmp://127.0.0.1:1935/live/key",
        "RTMP://ingest.example/app/key",
        "rtmp://ingest.example/app/key/with/slashes",
    ] {
        let got = resolve_stream_url(None, Some(url)).expect(url);
        assert_eq!(got, url, "a complete target resolves unchanged");
    }
}

/// The garbage-scheme case, hardware-free and unchanged in outcome: still
/// `E_BAD_PAYLOAD` (its command-path twin is
/// `prompt10_stream_cmds::stream_start_with_garbage_url_refuses_bad_payload`).
#[test]
fn a_garbage_scheme_still_refuses_bad_payload() {
    for url in ["notaurl", "http://host/live/key", "srt://host/live/key"] {
        let err = resolve_stream_url(None, Some(url)).expect_err(url);
        assert!(is_bad_payload(&err), "{url}: got {err}");
    }
}
