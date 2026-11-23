import Foundation

/// What each control-plane alert means to the operator, and what to do
/// (Tier 3, the user's ruling of 2026-11-18 04:55). Keyed by the alert ids the
/// control plane raises (packages/control-plane/src/alerts.ts); a test reads
/// that file so a new id without a meaning here fails. An id this table does
/// not know is shown as it came, never hidden.
public enum AlertMeaning {
    public struct Entry: Equatable, Sendable {
        public let title: String
        public let meaning: String
        public let action: String
    }

    public static let table: [String: Entry] = [
        "engine.notReporting": Entry(
            title: "ENGINE NOT REPORTING",
            meaning: "The render engine has stopped reporting; what is on air cannot be confirmed.",
            action: "Look at the programme output; be ready to put the fallback slate up."),
        "command.unacknowledged.stream": Entry(
            title: "STREAM COMMAND NOT ACKNOWLEDGED",
            meaning: "A stream start or stop went to the engine and was not acknowledged in time.",
            action: "Check the stream's state before sending it again."),
        "command.unacknowledged.record": Entry(
            title: "RECORD COMMAND NOT ACKNOWLEDGED",
            meaning: "A record start or stop went to the engine and was not acknowledged in time.",
            action: "Check the record's state before sending it again."),
        "stream.divergence": Entry(
            title: "STREAM STATE DISAGREES",
            meaning: "The control plane and the engine disagree on whether the stream is live.",
            action: "Trust the engine's report; stop and restart the stream if it should be live."),
        "outputs.held": Entry(
            title: "SHOW STOP HELD",
            meaning: "show.stop was held because an output (record or stream) is still active.",
            action: "Stop the record and the stream first, or stop the show with force."),
        "audit.sinkFailed": Entry(
            title: "AUDIT LOG FAILING",
            meaning: "The audit log cannot be written: commands are not being recorded.",
            action: "Free disk space or fix the audit path; tell the engineer."),
        "audio.underrun": Entry(
            title: "AUDIO UNDERRUN",
            meaning: "Programme audio ran short of samples: a gap was heard.",
            action: "Watch the audio meters; tell the engineer if it repeats."),
        "audio.device": Entry(
            title: "AUDIO DEVICE",
            meaning: "The audio output device was lost or changed.",
            action: "Check the audio output device and its cable."),
        "item.failed": Entry(
            title: "ITEM FAILED",
            meaning: "A clip on air stopped decoding; the View holds its last frame.",
            action: "Take the next item or the fallback slate."),
        "ticker.notOnAir": Entry(
            title: "TICKER ITEM COUNT MISMATCH",
            meaning: "After the settling period, the engine reports a different ticker override item count from the control plane.",
            action: "Check the programme output and engine log before changing or clearing the override."),
        "controlPlane.restarted": Entry(
            title: "CONTROL PLANE RESTARTED",
            meaning: "The control plane restarted mid-show and took over the show the engine was running; the picture never stopped. The note says what was kept and what was dropped.",
            action: "Check the dropped automation and the rundown; nothing else is needed. It clears when this show ends."),
        "engine.unidentified": Entry(
            title: "ENGINE SHOW NOT IDENTIFIED",
            meaning: "After a restart, the control plane cannot identify the show the engine is running; it has not touched it, and the picture stays.",
            action: "Either choice ends what is on air now: load a show to put it in its place, or unload to clear it. Wait for a break if you can."),
        "record.failed": Entry(
            title: "RECORDING STOPPED",
            meaning: "The take stopped recording on its own; the file keeps what was written before it stopped.",
            action: "Stop the take, check the record disk, then start a new take."),
    ]

    /// The entry for `id`, or one that names an id this shell does not know.
    public static func of(_ id: String) -> Entry {
        table[id] ?? Entry(
            title: id,
            meaning: "An alert this shell does not know yet.",
            action: "Read its reason.")
    }
}
