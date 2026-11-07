import Foundation

/// Every command the shell can send: its whole binding to §16. The parity
/// tests run each case against the exported catalog, so a §16 change that
/// breaks a binding fails at test time, never at showtime (Prompt 13 step 3).
public enum ShellCommand: Equatable, Sendable {
    // 16.1 show
    case showLoad(packagePath: String)
    case showPreflight
    case showStart
    case showStop(force: Bool)
    /// PROPOSAL (finding 1): not in SPEC v0.4 §16; the draft's control plane has it.
    case getState
    // 16.2 view/preview
    case previewSet(itemRef: String)
    case take(transition: Transition, durationFrames: Int?)
    case cut(itemRef: String)
    // 16.5 element/graphic
    case elementToggle(elementId: String, visible: Bool?)
    case breakingShow(headline: String, subhead: String?)
    case breakingHide
    // 16.8 soundboard
    case soundboardPlay(assetId: String)
    case soundboardStopAll
    // 16.14 outputs
    case recordStart(outputId: String?)
    case recordStop
    case streamStart(outputId: String?)
    case streamStop
    // 16.15 system
    case status
    case telemetrySubscribe(intervalMs: Int)

    public enum Transition: String, CaseIterable, Sendable { case cut, mix, wipe, sting, move, dve }

    public var name: String {
        switch self {
        case .showLoad: "show.load"
        case .showPreflight: "show.preflight"
        case .showStart: "show.start"
        case .showStop: "show.stop"
        case .getState: "show.getState"
        case .previewSet: "preview.set"
        case .take: "view.take"
        case .cut: "view.cut"
        case .elementToggle: "element.toggle"
        case .breakingShow: "breaking.show"
        case .breakingHide: "breaking.hide"
        case .soundboardPlay: "soundboard.play"
        case .soundboardStopAll: "soundboard.stopAll"
        case .recordStart: "record.start"
        case .recordStop: "record.stop"
        case .streamStart: "stream.start"
        case .streamStop: "stream.stop"
        case .status: "system.status"
        case .telemetrySubscribe: "system.telemetry.subscribe"
        }
    }

    public var payload: [String: JSONValue] {
        switch self {
        case .showLoad(let path): ["packagePath": .string(path)]
        case .showPreflight, .showStart, .getState, .breakingHide, .soundboardStopAll, .recordStop, .streamStop, .status: [:]
        case .showStop(let force): ["force": .bool(force)]
        case .previewSet(let ref): ["itemRef": .string(ref)]
        case .take(let t, let frames):
            frames.map { ["transition": .string(t.rawValue), "durationFrames": .number(Double($0))] }
                ?? ["transition": .string(t.rawValue)]
        case .cut(let ref): ["itemRef": .string(ref)]
        case .elementToggle(let id, let visible):
            visible.map { ["elementId": .string(id), "visible": .bool($0)] } ?? ["elementId": .string(id)]
        case .breakingShow(let headline, let subhead):
            subhead.map { ["headline": .string(headline), "subhead": .string($0)] } ?? ["headline": .string(headline)]
        case .soundboardPlay(let asset): ["assetId": .string(asset)]
        case .recordStart(let id), .streamStart(let id): id.map { ["outputId": .string($0)] } ?? [:]
        case .telemetrySubscribe(let ms): ["intervalMs": .number(Double(ms))]
        }
    }

    public func envelope(baseStateVersion: Int? = nil) -> CommandEnvelope {
        CommandEnvelope(command: name, payload: payload, baseStateVersion: baseStateVersion)
    }

    /// One instance of every case, for the parity tests.
    public static let samples: [ShellCommand] = [
        .showLoad(packagePath: "/shows/dress"), .showPreflight, .showStart, .showStop(force: false), .getState,
        .previewSet(itemRef: "B2"), .take(transition: .cut, durationFrames: nil),
        .take(transition: .mix, durationFrames: 12), .cut(itemRef: "B3"),
        .elementToggle(elementId: "lt1", visible: true), .elementToggle(elementId: "lt1", visible: nil),
        .breakingShow(headline: "BREAKING", subhead: "detail"), .breakingShow(headline: "BREAKING", subhead: nil),
        .breakingHide, .soundboardPlay(assetId: "stab"), .soundboardStopAll,
        .recordStart(outputId: "ep01"), .recordStart(outputId: nil), .recordStop,
        .streamStart(outputId: nil), .streamStop, .status, .telemetrySubscribe(intervalMs: 100),
    ]
}

/// AC-16: what a single operator must be able to do, each mapped to the
/// command that does it (SPEC Appendix AC-16).
public enum AC16Action: String, CaseIterable, Sendable {
    case loadPackage = "load a package"
    case runPreflight = "run preflight"
    case armFirstSequence = "arm first sequence"
    case startShow = "start show"
    case takeBetweenItems = "take between items"
    case triggerLowerThird = "trigger lower third"
    case triggerBreakingBanner = "trigger breaking banner"
    case playSoundboardEffect = "play soundboard effect"
    case startStopRecording = "start/stop recording"
    case startStopStream = "start/stop stream"

    public var commands: [ShellCommand] {
        switch self {
        case .loadPackage: [.showLoad(packagePath: "/shows/dress")]
        case .runPreflight: [.showPreflight]
        case .armFirstSequence: [.previewSet(itemRef: "S1")]
        case .startShow: [.showStart]
        case .takeBetweenItems: [.take(transition: .cut, durationFrames: nil)]
        case .triggerLowerThird: [.elementToggle(elementId: "lowerThird", visible: true)]
        case .triggerBreakingBanner: [.breakingShow(headline: "BREAKING", subhead: nil)]
        case .playSoundboardEffect: [.soundboardPlay(assetId: "stab")]
        case .startStopRecording: [.recordStart(outputId: nil), .recordStop]
        case .startStopStream: [.streamStart(outputId: nil), .streamStop]
        }
    }
}
