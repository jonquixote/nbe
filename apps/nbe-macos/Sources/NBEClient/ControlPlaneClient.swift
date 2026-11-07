import Foundation
import NBEProtocol

/// A session on the control plane's one endpoint (SPEC §5.3): the handshake
/// headers a browser cannot send, a command matched to its response by
/// `requestId` (§5.4), and the server's pushes (§5.4.1) as a stream.
///
/// The client keeps no state of the show. After every connect it asks
/// `system.status` (§5.4.1 rule 3: a client that missed a `stateChange`
/// recovers with it), and the shell's model folds what arrives.
public actor ControlPlaneClient {
    public struct Config: Sendable {
        public var url: URL
        public var token: String
        public var role: Role
        /// Past this a command with no response is reported as unanswered:
        /// a hang guard, not a measurement (R39); the command may still apply.
        public var responseGuard: Duration

        public init(url: URL, token: String, role: Role, responseGuard: Duration = .seconds(10)) {
            self.url = url
            self.token = token
            self.role = role
            self.responseGuard = responseGuard
        }

        /// `ws://127.0.0.1:8462/nbe/v0.3`, the default local endpoint.
        public static func local(token: String, role: Role, port: Int = defaultPort) -> Config {
            Config(url: URL(string: "ws://127.0.0.1:\(port)\(wsPath)")!, token: token, role: role)
        }
    }

    public enum ClientError: Error, Equatable {
        /// A bearer token over `ws://` to a host that is not this machine:
        /// refused. SPEC §5.3 puts remote use on the TLS endpoint (`wss://`).
        case cleartextToRemoteHost(String)
        case notConnected
        case unanswered(command: String)
        case refusedByCatalog([String])
        case closed
    }

    public let config: Config
    private let catalog: CommandCatalog?
    private var task: URLSessionWebSocketTask?
    private var pending: [String: CheckedContinuation<CommandResponse, Error>] = [:]
    private var pushContinuation: AsyncStream<InboundFrame>.Continuation?
    public nonisolated let pushes: AsyncStream<InboundFrame>

    public init(config: Config, catalog: CommandCatalog? = try? .bundled()) {
        self.config = config
        self.catalog = catalog
        var cont: AsyncStream<InboundFrame>.Continuation!
        self.pushes = AsyncStream { cont = $0 }
        self.pushContinuation = cont
    }

    /// The upgrade request, with §5.3's two headers.
    public nonisolated static func handshake(_ config: Config) -> URLRequest {
        var req = URLRequest(url: config.url)
        req.setValue("Bearer \(config.token)", forHTTPHeaderField: "Authorization")
        req.setValue(config.role.rawValue, forHTTPHeaderField: "X-NBE-Role")
        return req
    }

    /// Whether a token may travel to `url`: `wss://` anywhere, `ws://` only
    /// to a literal loopback address (SPEC §5.3's default local endpoint is
    /// `127.0.0.1`). A name such as `localhost` is refused: what it resolves
    /// to is the resolver's, not this check's.
    public nonisolated static func checkTransport(_ url: URL) throws {
        guard url.scheme?.lowercased() == "ws" else { return }
        let host = (url.host ?? "").lowercased()
        let loopback = ["127.0.0.1", "::1", "[::1]"]
        if !loopback.contains(host) { throw ClientError.cleartextToRemoteHost(host) }
    }

    public func connect() async throws -> CommandResponse {
        try Self.checkTransport(config.url)
        let task = URLSession.shared.webSocketTask(with: Self.handshake(config))
        self.task = task
        task.resume()
        Task { await self.receiveLoop(task) }
        return try await send(.status)
    }

    public func disconnect() {
        task?.cancel(with: .normalClosure, reason: nil)
        task = nil
        failAll(ClientError.closed)
    }

    /// Send one bound command and await its response. The catalog check runs
    /// first: a command §16 would refuse by shape or by role never leaves.
    public func send(_ command: ShellCommand, baseStateVersion: Int? = nil) async throws -> CommandResponse {
        if let catalog {
            let problems = catalog.problems(command: command.name, payload: command.payload, role: config.role)
            if !problems.isEmpty { throw ClientError.refusedByCatalog(problems) }
        }
        return try await send(command.envelope(baseStateVersion: baseStateVersion))
    }

    public func send(_ envelope: CommandEnvelope) async throws -> CommandResponse {
        guard let task else { throw ClientError.notConnected }
        let text = String(decoding: try JSONEncoder().encode(envelope), as: UTF8.self)
        let id = envelope.id
        let command = envelope.command
        return try await withCheckedThrowingContinuation { cont in
            // Registered on the actor before the frame leaves, so a response
            // can never arrive ahead of its continuation.
            Task { await self.begin(id: id, command: command, text: text, task: task, cont: cont) }
        }
    }

    private func begin(id: String, command: String, text: String, task: URLSessionWebSocketTask,
                       cont: CheckedContinuation<CommandResponse, Error>) {
        pending[id] = cont
        task.send(.string(text)) { error in
            if let error { Task { await self.resolve(id, with: .failure(error)) } }
        }
        let guardTime = config.responseGuard
        Task {
            try? await Task.sleep(for: guardTime)
            await self.resolve(id, with: .failure(ClientError.unanswered(command: command)))
        }
    }

    private func resolve(_ id: String, with result: Result<CommandResponse, Error>) {
        guard let cont = pending.removeValue(forKey: id) else { return }
        cont.resume(with: result)
    }

    private func failAll(_ error: Error) {
        let all = pending
        pending = [:]
        for (_, cont) in all { cont.resume(throwing: error) }
    }

    private func receiveLoop(_ task: URLSessionWebSocketTask) async {
        while true {
            let message: URLSessionWebSocketTask.Message
            do { message = try await task.receive() } catch {
                failAll(error)
                return
            }
            let data: Data
            switch message {
            case .string(let s): data = Data(s.utf8)
            case .data(let d): data = d
            @unknown default: continue
            }
            guard let frame = try? InboundFrame.decode(data) else { continue }
            if case .response(let r) = frame {
                resolve(r.requestId, with: .success(r))
            } else {
                pushContinuation?.yield(frame)
            }
        }
    }
}
