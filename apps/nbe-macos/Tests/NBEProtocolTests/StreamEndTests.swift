import Foundation
import Network
import XCTest
@testable import NBEClient
@testable import NBEProtocol

/// Operator failure design, build step 1: the client's push stream ends when
/// the socket ends. Before it, a control plane that died left the shell's
/// `for await` over `pushes` waiting for ever, and the shell showed
/// "connected as operator" with frozen values.
///
/// No control plane is needed: a one-connection WebSocket server on loopback
/// answers the client's `system.status`, then drops the socket, as a dying
/// control plane would.
final class StreamEndTests: XCTestCase {
    /// Accepts one connection, answers its first command with an ok
    /// response, then cancels the connection.
    private final class OneShotServer: @unchecked Sendable {
        let listener: NWListener
        private let queue = DispatchQueue(label: "nbe-test-one-shot-ws")
        private var connection: NWConnection?

        init() throws {
            let params = NWParameters.tcp
            let ws = NWProtocolWebSocket.Options()
            params.defaultProtocolStack.applicationProtocols.insert(ws, at: 0)
            params.requiredLocalEndpoint = NWEndpoint.hostPort(host: "127.0.0.1", port: .any)
            listener = try NWListener(using: params)
        }

        func start() async throws -> UInt16 {
            listener.newConnectionHandler = { [weak self] conn in
                guard let self else { return }
                self.connection = conn
                conn.start(queue: self.queue)
                self.answerFirstCommandThenDrop(conn)
            }
            return try await withCheckedThrowingContinuation { cont in
                var resumed = false
                listener.stateUpdateHandler = { [listener] state in
                    guard !resumed else { return }
                    switch state {
                    case .ready:
                        resumed = true
                        cont.resume(returning: listener.port?.rawValue ?? 0)
                    case .failed(let e):
                        resumed = true
                        cont.resume(throwing: e)
                    default: break
                    }
                }
                listener.start(queue: queue)
            }
        }

        private func answerFirstCommandThenDrop(_ conn: NWConnection) {
            conn.receiveMessage { data, _, _, _ in
                guard let data,
                      let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                      let id = obj["id"] as? String
                else { conn.cancel(); return }
                let reply = #"{"v":"0.3","requestId":"\#(id)","status":"ok","stateVersion":1,"data":{}}"#
                let meta = NWProtocolWebSocket.Metadata(opcode: .text)
                let ctx = NWConnection.ContentContext(identifier: "reply", metadata: [meta])
                conn.send(content: Data(reply.utf8), contentContext: ctx, isComplete: true,
                          completion: .contentProcessed { _ in
                    // The control plane dies a moment after answering.
                    self.queue.asyncAfter(deadline: .now() + 0.3) { conn.cancel() }
                })
            }
        }

        func stop() {
            connection?.cancel()
            listener.cancel()
        }
    }

    func testThePushStreamEndsWhenTheSocketEnds() async throws {
        let server = try OneShotServer()
        let port = try await server.start()
        defer { server.stop() }
        let url = try XCTUnwrap(URL(string: "ws://127.0.0.1:\(port)/nbe/v0.3"))
        let client = ControlPlaneClient(
            config: .init(url: url, token: "t", role: .operator, responseGuard: .seconds(5)),
            catalog: nil
        )
        let hello = try await client.connect()
        XCTAssertTrue(hello.ok, "precondition: the server answered system.status: \(hello)")

        // A hang guard, not a measurement: the stream must end once the
        // server drops the socket, well inside 5 s.
        let ended = await withTaskGroup(of: Bool.self) { group -> Bool in
            group.addTask {
                for await _ in client.pushes {}
                return true
            }
            group.addTask {
                try? await Task.sleep(for: .seconds(5))
                return false
            }
            let first = await group.next() ?? false
            group.cancelAll()
            return first
        }
        XCTAssertTrue(ended, "the push stream must end when the socket ends (it waited 5 s)")

        // And a command after the end is refused at once, not left waiting.
        do {
            _ = try await client.send(.status)
            XCTFail("a command on an ended session must fail")
        } catch {
            XCTAssertEqual(error as? ControlPlaneClient.ClientError, .notConnected)
        }
    }

    func testDisconnectEndsThePushStream() async throws {
        let server = try OneShotServer()
        let port = try await server.start()
        defer { server.stop() }
        let url = try XCTUnwrap(URL(string: "ws://127.0.0.1:\(port)/nbe/v0.3"))
        let client = ControlPlaneClient(
            config: .init(url: url, token: "t", role: .operator, responseGuard: .seconds(5)),
            catalog: nil
        )
        _ = try await client.connect()
        await client.disconnect()
        let ended = await withTaskGroup(of: Bool.self) { group -> Bool in
            group.addTask {
                for await _ in client.pushes {}
                return true
            }
            group.addTask {
                try? await Task.sleep(for: .seconds(5))
                return false
            }
            let first = await group.next() ?? false
            group.cancelAll()
            return first
        }
        XCTAssertTrue(ended, "disconnect() must end the push stream")
    }
}
