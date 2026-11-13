import Foundation
import XCTest
@testable import NBEClient
@testable import NBEProtocol

/// AC-16, end to end: a single operator does the ten things, each through
/// the same `ShellCommand` the shell's button sends and the same client,
/// against a real control plane with the real engine attached. Each action
/// is checked in the control plane's state (`show.getState`,
/// `system.status`) and, where the engine owns the answer, in the engine's
/// own report on the telemetry tick.
///
/// The single operator holds the `admin` role. SPEC §16.0's matrix gives the
/// `operator` role no `show.load`, `show.preflight` or `show.start` (the
/// first run of this test was refused at `show.load` by the shell's own
/// catalog check), so AC-16's one person doing all ten is, under §16.0,
/// an admin session. Recorded, not resolved here.
///
/// Opt-in: skipped unless `NBE_SHELL_E2E_URL`, `NBE_SHELL_E2E_ADMIN_TOKEN`
/// and `NBE_SHELL_E2E_PACKAGE` are set. `scripts/shell-e2e.sh` with
/// `NBE_ENGINE_BIN` starts the control plane, the engine and a package.
final class AC16LiveTests: XCTestCase {
    private var url: URL!
    private var token = ""
    private var package = ""

    override func setUpWithError() throws {
        let e = ProcessInfo.processInfo.environment
        guard let u = e["NBE_SHELL_E2E_URL"], let url = URL(string: u),
              let op = e["NBE_SHELL_E2E_ADMIN_TOKEN"], let pkg = e["NBE_SHELL_E2E_PACKAGE"]
        else { throw XCTSkip("no engine-attached control plane: NBE_SHELL_E2E_URL, the admin token and NBE_SHELL_E2E_PACKAGE are unset") }
        self.url = url
        token = op
        package = pkg
    }

    /// The latest telemetry tick, kept as the pushes arrive.
    private final class Ticks: @unchecked Sendable {
        private let lock = NSLock()
        private var last: Telemetry?
        func set(_ t: Telemetry) { lock.lock(); last = t; lock.unlock() }
        func get() -> Telemetry? { lock.lock(); defer { lock.unlock() }; return last }
    }

    private func wait(_ what: String, _ ticks: Ticks, seconds: Double = 8, _ ok: (Telemetry) -> Bool) async throws {
        let deadline = Date().addingTimeInterval(seconds)
        while Date() < deadline {
            if let t = ticks.get(), ok(t) { return }
            try await Task.sleep(for: .milliseconds(100))
        }
        XCTFail("\(what): not reported within \(seconds) s; last tick \(String(describing: ticks.get()))")
    }

    private func state(_ c: ControlPlaneClient) async throws -> ShowStateView {
        let r = try await c.send(.getState)
        XCTAssertTrue(r.ok, "show.getState: \(r)")
        return try XCTUnwrap(r.data).decode(ShowStateView.self)
    }

    private func ok(_ c: ControlPlaneClient, _ cmd: ShellCommand, _ action: AC16Action) async throws {
        let r = try await c.send(cmd)
        XCTAssertTrue(r.ok, "AC-16 \(action.rawValue): \(cmd.name) refused: \(String(describing: r.error))")
    }

    func testASingleOperatorDoesTheTenThings() async throws {
        let c = ControlPlaneClient(config: .init(url: url, token: token, role: .admin, responseGuard: .seconds(30)))
        _ = try await c.connect()
        let ticks = Ticks()
        let pump = Task {
            for await frame in c.pushes {
                if case .telemetry(let t) = frame { ticks.set(t.data) }
            }
        }
        defer { pump.cancel() }
        try await ok(c, .telemetrySubscribe(intervalMs: 250), .loadPackage)

        // 1. Load a package.
        try await ok(c, .showLoad(packagePath: package), .loadPackage)
        var s = try await state(c)
        let items = try XCTUnwrap(s.rundown?.items, "the rundown")
        XCTAssertGreaterThanOrEqual(items.count, 2, "two items to take between")

        // 2. Run preflight.
        try await ok(c, .showPreflight, .runPreflight)
        let statusReply = try await c.send(.status)
        let status = try XCTUnwrap(statusReply.data).decode(StatusSnapshot.self)
        XCTAssertEqual(status.preflightPassed, true, "preflight passed")

        // 3. Arm the first item (the rundown's first sequence item).
        try await ok(c, .previewSet(itemRef: items[0].id), .armFirstSequence)
        s = try await state(c)
        XCTAssertEqual(s.rundown?.items.first?.state, "ARMED", "the first item is armed")

        // 4. Start the show.
        try await ok(c, .showStart, .startShow)
        try await wait("the show running", ticks) { $0.showState == "RUNNING" && $0.masterClockFrame ?? 0 > 0 }

        // 5. Take, then take between items.
        try await ok(c, .take(transition: .cut, durationFrames: nil), .takeBetweenItems)
        try await wait("the first item on air", ticks) { $0.viewItem == items[0].id }
        try await ok(c, .previewSet(itemRef: items[1].id), .takeBetweenItems)
        try await ok(c, .take(transition: .mix, durationFrames: 15), .takeBetweenItems)
        try await wait("the second item on air", ticks) { $0.viewItem == items[1].id }

        // 6. A lower third: the package's first lower-third template, on and off.
        let lt = try XCTUnwrap(s.templates?.first { $0.kind.hasPrefix("lowerThird") }, "a lower-third template")
        try await ok(c, .graphicShow(templateId: lt.id, fields: ["headline": "AC-16 lower third"]), .triggerLowerThird)
        try await wait("the engine drawing the lower third", ticks) {
            ($0.engineOverlayState?.graphics ?? []).contains("\(lt.id)-graphic")
        }
        try await ok(c, .graphicHide(templateId: lt.id), .triggerLowerThird)
        try await wait("the lower third gone", ticks) { $0.engineOverlayState?.graphics?.isEmpty == true }

        // 7. The breaking banner.
        try await ok(c, .breakingShow(headline: "AC-16 BREAKING", subhead: nil), .triggerBreakingBanner)
        try await wait("the engine drawing the banner", ticks) { $0.engineOverlayState?.breaking == true }
        try await ok(c, .breakingHide, .triggerBreakingBanner)
        try await wait("the banner gone", ticks) { $0.engineOverlayState?.breaking == false }

        // 8. A soundboard effect: the sfx bus meters it.
        let sfx = ProcessInfo.processInfo.environment["NBE_SHELL_E2E_SFX"] ?? "stab_sfx"
        try await ok(c, .soundboardPlay(assetId: sfx), .playSoundboardEffect)
        try await wait("the sfx bus sounding", ticks) { (($0.busPeakDbfs?["sfx"] ?? nil) ?? -120) > -60 }

        // 9. Recording, started and stopped.
        try await ok(c, .recordStart(outputId: nil), .startStopRecording)
        try await wait("recording", ticks) { $0.recordState == "recording" || $0.recordState == "RECORDING" }
        try await Task.sleep(for: .seconds(2))
        try await ok(c, .recordStop, .startStopRecording)
        try await wait("recording stopped", ticks, seconds: 30) { $0.recordState != "recording" && $0.recordState != "RECORDING" }

        // 10. The stream, started and stopped.
        try await ok(c, .streamStart(outputId: nil), .startStopStream)
        try await wait("streaming", ticks) { ($0.streamState ?? "").lowercased() == "live" }
        try await Task.sleep(for: .seconds(2))
        try await ok(c, .streamStop, .startStopStream)
        try await wait("the stream stopped", ticks, seconds: 30) { ($0.streamState ?? "").lowercased() != "live" }

        // Leave the control plane as it was found: the show stopped and the
        // package unloaded, for the live tests that run after this one.
        try await ok(c, .showStop(force: false), .startShow)
        let unload = try await c.send(CommandEnvelope(command: "show.unload", payload: [:], baseStateVersion: nil))
        XCTAssertTrue(unload.ok, "show.unload: \(String(describing: unload.error))")
        await c.disconnect()
    }
}
