import Combine
import Foundation
import NBEProtocol
import Network
import XCTest

@testable import NBEShell

/// The operator shell's model, now importable (the NBEShell library target).
@MainActor
final class ShellModelTests: XCTestCase {
    /// #122's surviving mutant MC (`may()` ignoring the stale state), killed:
    /// a session last heard 5 s ago is past the 2 s contact limit, so no
    /// command may be sent; heard just now, the same commands may.
    func testNoCommandMayBeSentWhileContactIsStale() {
        let model = ShellModel()
        model.loadFixture(staleSeconds: 5)
        model.now = Date()
        XCTAssertTrue(model.noContact, "5 s without a frame is NO CONTACT")
        for command in ShellCommand.samples {
            XCTAssertFalse(model.may(command), "\(command.name) must be refused while there is no contact")
        }
        model.loadFixture(staleSeconds: 0)
        model.now = Date()
        XCTAssertFalse(model.noContact)
        XCTAssertTrue(
            ShellCommand.samples.allSatisfy { model.may($0) },
            "with contact, the fixture session may send every sample command"
        )
    }

    /// The fixture's sample alerts (`--alerts`): three, by id; none without.
    func testTheFixtureCarriesItsSampleAlertsOnlyWhenAsked() {
        let model = ShellModel()
        model.loadFixture(alerts: true)
        let ids = AlertFeed.current(status: model.status, telemetry: model.telemetry).map(\.id)
        XCTAssertEqual(ids, ["item.failed", "audio.underrun", "sample.unknown"])
        model.loadFixture()
        XCTAssertEqual(AlertFeed.current(status: model.status, telemetry: model.telemetry), [])
    }

    /// Only the costly stops ask for confirmation; TAKE, CUT and the rest
    /// are one press.
    func testOnlyTheCostlyStopsAskForConfirmation() {
        let model = ShellModel()
        let costly: [ShellCommand] = [.showStop(force: false), .showStop(force: true), .recordStop, .streamStop]
        for c in costly {
            XCTAssertNotNil(model.confirmation(for: c), "\(c.name) must ask first")
        }
        for c in ShellCommand.samples where !costly.contains(c) {
            XCTAssertNil(model.confirmation(for: c), "\(c.name) must be one press")
        }
    }

    /// Every fixture state gives its own condition, and only its own.
    func testEachFixtureStateGivesItsCondition() {
        for state in FixtureState.allCases {
            let model = ShellModel()
            model.loadFixture(state: state)
            model.now = Date()
            let alerts = AlertFeed.current(status: model.status, telemetry: model.telemetry)
            XCTAssertEqual(model.noContact, state == .noContact, "\(state): NO CONTACT")
            XCTAssertEqual(model.telemetry.fallbackActive == true, state == .fallback, "\(state): the fallback")
            XCTAssertEqual(alerts.count, state == .alerts ? 3 : 0, "\(state): the alerts")
            switch (state, model.connection) {
            case (.lost, .lost), (.connecting, .connecting), (.failed, .failed): break
            case (.lost, _), (.connecting, _), (.failed, _):
                XCTFail("\(state): the connection reads \(model.connection)")
            case (_, .fixture): break
            default: XCTFail("\(state): the connection reads \(model.connection)")
            }
        }
    }

    /// #120's untested part: the model's `.lost` state. A loopback control
    /// plane answers, sends a tick, then drops the first connection; the
    /// model must pass through `.lost(since:)` and note "lost contact"
    /// before its session reconnects.
    func testTheModelGoesLostWhenTheSocketDrops() async throws {
        let server = try DroppingServer()
        let port = try await server.start()
        defer { server.stop() }
        let model = ShellModel()
        var seen: [ShellModel.Connection] = []
        let watch = model.$connection.sink { seen.append($0) }
        defer { watch.cancel() }
        let url = try XCTUnwrap(URL(string: "ws://127.0.0.1:\(port)/nbe/v0.3"))
        model.connect(url: url, token: "t", role: .operator)
        let deadline = Date().addingTimeInterval(5)
        while Date() < deadline, !seen.contains(where: { if case .lost = $0 { true } else { false } }) {
            try await Task.sleep(for: .milliseconds(20))
        }
        let lost = seen.firstIndex { if case .lost = $0 { true } else { false } }
        let connected = seen.firstIndex { if case .connected = $0 { true } else { false } }
        XCTAssertNotNil(connected, "the model connected first: \(seen)")
        XCTAssertNotNil(lost, "the dropped socket must show as .lost: \(seen)")
        if let lost, let connected { XCTAssertLessThan(connected, lost) }
        XCTAssertTrue(model.log.contains { $0.contains("lost contact") }, "the log notes it: \(model.log)")
    }

    /// A command in flight is pending: may() refuses it, why() says so, and a
    /// second press sends nothing. The server answers show.start after 0.5 s.
    func testACommandInFlightIsPendingAndIsNotSentTwice() async throws {
        let server = try DelayingServer(slow: "show.start", delay: 0.5)
        let port = try await server.start()
        defer { server.stop() }
        let model = ShellModel()
        let url = try XCTUnwrap(URL(string: "ws://127.0.0.1:\(port)/nbe/v0.3"))
        model.connect(url: url, token: "t", role: .admin)
        let deadline = Date().addingTimeInterval(5)
        while Date() < deadline {
            if case .connected = model.connection { break }
            try await Task.sleep(for: .milliseconds(20))
        }
        guard case .connected = model.connection else { return XCTFail("never connected: \(model.connection)") }
        model.now = Date()
        XCTAssertTrue(model.may(.showStart), "admin, in contact, nothing pending: \(model.why(.showStart))")
        model.run(.showStart)
        XCTAssertFalse(model.may(.showStart), "show.start is in flight")
        XCTAssertTrue(model.why(.showStart).contains("pending"), model.why(.showStart))
        model.run(.showStart)
        let until = Date().addingTimeInterval(3)
        while Date() < until, !model.inFlight.isEmpty { try await Task.sleep(for: .milliseconds(20)) }
        XCTAssertTrue(model.inFlight.isEmpty, "the answer came: \(model.inFlight)")
        XCTAssertEqual(server.count("show.start"), 1, "the second press sent nothing")
        model.now = Date()
        XCTAssertTrue(model.may(.showStart), "answered, it may be sent again: \(model.why(.showStart))")
    }

    /// Answers every command ok with a tick beside it (so contact stays
    /// fresh), the `slow` one after `delay`; counts each command it receives.
    private final class DelayingServer: @unchecked Sendable {
        let listener: NWListener
        private let queue = DispatchQueue(label: "nbe-test-model-pending")
        private var connections: [NWConnection] = []
        private var counts: [String: Int] = [:]
        private let slow: String
        private let delay: TimeInterval

        init(slow: String, delay: TimeInterval) throws {
            self.slow = slow
            self.delay = delay
            let params = NWParameters.tcp
            params.defaultProtocolStack.applicationProtocols.insert(NWProtocolWebSocket.Options(), at: 0)
            params.requiredLocalEndpoint = NWEndpoint.hostPort(host: "127.0.0.1", port: .any)
            listener = try NWListener(using: params)
        }

        func count(_ command: String) -> Int { queue.sync { counts[command] ?? 0 } }

        func start() async throws -> UInt16 {
            listener.newConnectionHandler = { [weak self] conn in
                guard let self else { return }
                self.connections.append(conn)
                conn.start(queue: self.queue)
                self.serve(conn)
            }
            let box = Once()
            return try await withCheckedThrowingContinuation { cont in
                listener.stateUpdateHandler = { [listener] state in
                    switch state {
                    case .ready: box.run { cont.resume(returning: listener.port?.rawValue ?? 0) }
                    case .failed(let e): box.run { cont.resume(throwing: e) }
                    default: break
                    }
                }
                listener.start(queue: queue)
            }
        }

        private func send(_ conn: NWConnection, _ text: String) {
            let meta = NWProtocolWebSocket.Metadata(opcode: .text)
            let ctx = NWConnection.ContentContext(identifier: "msg", metadata: [meta])
            conn.send(content: Data(text.utf8), contentContext: ctx, isComplete: true, completion: .idempotent)
        }

        private func serve(_ conn: NWConnection) {
            conn.receiveMessage { [weak self] data, _, _, error in
                guard let self else { return }
                guard error == nil, let data,
                      let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                      let id = obj["id"] as? String, let command = obj["command"] as? String
                else { return }
                self.counts[command, default: 0] += 1
                let answer = {
                    self.send(conn, #"{"v":"0.3","requestId":"\#(id)","status":"ok","stateVersion":1,"data":{}}"#)
                    self.send(conn, #"{"v":"0.3","kind":"telemetry","data":{"masterClockFrame":1}}"#)
                }
                if command == self.slow {
                    self.queue.asyncAfter(deadline: .now() + self.delay, execute: answer)
                } else {
                    answer()
                }
                self.serve(conn)
            }
        }

        func stop() {
            queue.sync { for c in connections { c.cancel() } }
            listener.cancel()
        }
    }

    /// Answers every command ok, sends one tick after the subscription, and
    /// drops the FIRST connection 0.3 s after that tick (ReconnectTests'
    /// scripted server, cut down).
    private final class DroppingServer: @unchecked Sendable {
        let listener: NWListener
        private let queue = DispatchQueue(label: "nbe-test-model-lost")
        private var connections: [NWConnection] = []
        private var accepted = 0

        init() throws {
            let params = NWParameters.tcp
            params.defaultProtocolStack.applicationProtocols.insert(NWProtocolWebSocket.Options(), at: 0)
            params.requiredLocalEndpoint = NWEndpoint.hostPort(host: "127.0.0.1", port: .any)
            listener = try NWListener(using: params)
        }

        func start() async throws -> UInt16 {
            listener.newConnectionHandler = { [weak self] conn in
                guard let self else { return }
                self.accepted += 1
                let index = self.accepted
                self.connections.append(conn)
                conn.start(queue: self.queue)
                self.serve(conn, index: index)
            }
            let box = Once()
            return try await withCheckedThrowingContinuation { cont in
                listener.stateUpdateHandler = { [listener] state in
                    switch state {
                    case .ready: box.run { cont.resume(returning: listener.port?.rawValue ?? 0) }
                    case .failed(let e): box.run { cont.resume(throwing: e) }
                    default: break
                    }
                }
                listener.start(queue: queue)
            }
        }

        private func send(_ conn: NWConnection, _ text: String, then: (() -> Void)? = nil) {
            let meta = NWProtocolWebSocket.Metadata(opcode: .text)
            let ctx = NWConnection.ContentContext(identifier: "msg", metadata: [meta])
            conn.send(content: Data(text.utf8), contentContext: ctx, isComplete: true,
                      completion: .contentProcessed { _ in then?() })
        }

        private func serve(_ conn: NWConnection, index: Int) {
            conn.receiveMessage { [weak self] data, _, _, error in
                guard let self else { return }
                guard error == nil, let data,
                      let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                      let id = obj["id"] as? String, let command = obj["command"] as? String
                else { return }
                let reply = #"{"v":"0.3","requestId":"\#(id)","status":"ok","stateVersion":\#(index),"data":{}}"#
                self.send(conn, reply) {
                    if command == "system.telemetry.subscribe" {
                        let tick = #"{"v":"0.3","kind":"telemetry","data":{"masterClockFrame":\#(index)}}"#
                        self.send(conn, tick) {
                            if index == 1 {
                                self.queue.asyncAfter(deadline: .now() + 0.3) { conn.cancel() }
                            }
                        }
                    }
                }
                self.serve(conn, index: index)
            }
        }

        func stop() {
            for c in connections { c.cancel() }
            listener.cancel()
        }
    }

    /// Resumes a continuation once, whichever listener state comes first.
    private final class Once: @unchecked Sendable {
        private let lock = NSLock()
        private var done = false
        func run(_ body: () -> Void) {
            lock.lock()
            defer { lock.unlock() }
            guard !done else { return }
            done = true
            body()
        }
    }
}
