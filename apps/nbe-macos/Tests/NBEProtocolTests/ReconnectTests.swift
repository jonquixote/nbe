import Foundation
import Network
import XCTest
@testable import NBEClient
@testable import NBEProtocol

/// Operator failure design, build step 3: the shell reconnects by itself.
/// A loopback WebSocket server stands in for the control plane. It answers
/// every command, and it pushes a telemetry tick only after a
/// `system.telemetry.subscribe` on that connection, as the real one does:
/// a subscription belongs to a connection. Its first connection is dropped
/// after the first tick, as a dying control plane would drop it.
final class ReconnectTests: XCTestCase {
    private final class ScriptedServer: @unchecked Sendable {
        let listener: NWListener
        private let queue = DispatchQueue(label: "nbe-test-reconnect-ws")
        private var connections: [NWConnection] = []
        /// Connections accepted so far (1-based index of the next one).
        private(set) var accepted = 0
        /// Drop the first connection this long after its first tick.
        let dropFirstAfter: TimeInterval

        init(dropFirstAfter: TimeInterval = 0.3) throws {
            self.dropFirstAfter = dropFirstAfter
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
            let box = ResumeOnce()
            return try await withCheckedThrowingContinuation { cont in
                listener.stateUpdateHandler = { [listener] state in
                    switch state {
                    case .ready: box.once { cont.resume(returning: listener.port?.rawValue ?? 0) }
                    case .failed(let e): box.once { cont.resume(throwing: e) }
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
                        // The tick names its connection in masterClockFrame.
                        let tick = #"{"v":"0.3","kind":"telemetry","data":{"masterClockFrame":\#(index)}}"#
                        self.send(conn, tick) {
                            if index == 1 {
                                self.queue.asyncAfter(deadline: .now() + self.dropFirstAfter) { conn.cancel() }
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

    private final class ResumeOnce: @unchecked Sendable {
        private let lock = NSLock()
        private var done = false
        func once(_ body: () -> Void) {
            lock.lock()
            defer { lock.unlock() }
            guard !done else { return }
            done = true
            body()
        }
    }

    private enum Seen: Equatable {
        case connected, tick(Int), lost, retrying(Int)
    }

    /// Every event the session reports, collected by one consumer for the
    /// whole test (an AsyncStream has one consumer; ending its iterator ends
    /// the stream).
    private actor EventLog {
        private(set) var seen: [Seen] = []
        func add(_ s: Seen) { seen.append(s) }
    }

    private func record(_ session: ReconnectingSession) -> EventLog {
        let log = EventLog()
        Task {
            for await e in session.events {
                switch e {
                case .connected: await log.add(.connected)
                case .frame(.telemetry(let t)): await log.add(.tick(t.data.masterClockFrame ?? -1))
                case .frame: break
                case .lost: await log.add(.lost)
                case .retrying(let n, _): await log.add(.retrying(n))
                }
            }
        }
        return log
    }

    /// Wait until `done` holds for what has been seen, or `seconds` pass (a
    /// hang guard, not a measurement). Returns what was seen.
    private func wait(_ log: EventLog, seconds: Double, until done: ([Seen]) -> Bool) async -> [Seen] {
        let deadline = Date().addingTimeInterval(seconds)
        while Date() < deadline {
            let seen = await log.seen
            if done(seen) { return seen }
            try? await Task.sleep(for: .milliseconds(20))
        }
        return await log.seen
    }

    func testTheBackoffDoublesFromAQuarterSecondToTwo() {
        XCTAssertEqual((1...6).map(Backoff.delay), [
            .milliseconds(250), .milliseconds(500), .milliseconds(1000),
            .milliseconds(2000), .milliseconds(2000), .milliseconds(2000),
        ])
    }

    func testASessionComesBackAndHearsTicksAgain() async throws {
        let server = try ScriptedServer()
        let port = try await server.start()
        defer { server.stop() }
        let url = try XCTUnwrap(URL(string: "ws://127.0.0.1:\(port)/nbe/v0.3"))
        let session = ReconnectingSession(
            config: .init(url: url, token: "t", role: .operator, responseGuard: .seconds(5)),
            catalog: nil
        )
        let log = record(session)
        await session.start()
        let seen = await wait(log, seconds: 8) { $0.contains(.tick(2)) }
        await session.stop()
        print("reconnect: \(seen)")
        XCTAssertEqual(Array(seen.prefix(4)), [.connected, .tick(1), .lost, .retrying(1)],
                       "connected, heard a tick, lost it, and scheduled attempt 1")
        XCTAssertTrue(seen.contains(.tick(2)),
                      "after the reconnect the session subscribed again and a tick arrived: \(seen)")
        XCTAssertEqual(seen.filter { $0 == .connected }.count, 2)
    }

    func testRetryNowCutsTheWait() async throws {
        let server = try ScriptedServer()
        let port = try await server.start()
        defer { server.stop() }
        let url = try XCTUnwrap(URL(string: "ws://127.0.0.1:\(port)/nbe/v0.3"))
        // A backoff far longer than the hang guard: only retryNow() can bring
        // the session back in time.
        let session = ReconnectingSession(
            config: .init(url: url, token: "t", role: .operator, responseGuard: .seconds(5)),
            catalog: nil,
            backoff: { _ in .seconds(60) }
        )
        let log = record(session)
        await session.start()
        let lost = await wait(log, seconds: 5) { $0.contains(.retrying(1)) }
        XCTAssertTrue(lost.contains(.retrying(1)), "precondition: the first connection was lost: \(lost)")
        let started = Date()
        await session.retryNow()
        let back = await wait(log, seconds: 5) { $0.contains(.tick(2)) }
        let took = Date().timeIntervalSince(started)
        await session.stop()
        print("retryNow: back in \(String(format: "%.2f", took)) s: \(back)")
        XCTAssertTrue(back.contains(.tick(2)), "retryNow reconnected well inside the 60 s backoff: \(back)")
    }
}
