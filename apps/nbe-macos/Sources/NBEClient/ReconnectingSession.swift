import Foundation
import NBEProtocol

/// The retry schedule after a lost or refused connection (operator failure
/// design, build step 3): 0.25 s, doubling, at most 2 s.
public enum Backoff {
    public static func delay(attempt: Int) -> Duration {
        let n = max(1, attempt)
        let ms = 250 * (1 << min(n - 1, 3))
        return .milliseconds(min(ms, 2000))
    }
}

/// What a reconnecting session reports, in order.
public enum SessionEvent: Sendable {
    /// Connected (again): the `system.status` answer that opened the session.
    case connected(CommandResponse)
    /// A server push on the current connection.
    case frame(InboundFrame)
    /// The connection ended, or could not be made.
    case lost(Date)
    /// The next attempt, and how long until it.
    case retrying(attempt: Int, after: Duration)
}

/// A session that comes back by itself (operator failure design, build step
/// 3). Each connection is a fresh `ControlPlaneClient`. On every connect it
/// asks `system.status` (the client does) and subscribes to telemetry again:
/// a subscription belongs to a connection, and without it a reconnected shell
/// would hear nothing and the NO CONTACT curtain would stay up. Between
/// attempts it waits `Backoff`; `retryNow()` (the shell's `R`) cuts the wait.
public actor ReconnectingSession {
    public nonisolated let events: AsyncStream<SessionEvent>
    private let continuation: AsyncStream<SessionEvent>.Continuation
    private let config: ControlPlaneClient.Config
    private let catalog: CommandCatalog?
    private let subscribeIntervalMs: Int
    private let backoff: @Sendable (Int) -> Duration
    private var current: ControlPlaneClient?
    private var loop: Task<Void, Never>?
    private var wake: CheckedContinuation<Void, Never>?
    /// Which wait `wake` belongs to, so a timer from an earlier wait cannot
    /// resume a later one.
    private var pauseGeneration = 0
    private var stopped = false

    public init(
        config: ControlPlaneClient.Config,
        catalog: CommandCatalog? = try? .bundled(),
        subscribeIntervalMs: Int = 100,
        backoff: @escaping @Sendable (Int) -> Duration = { Backoff.delay(attempt: $0) }
    ) {
        self.config = config
        self.catalog = catalog
        self.subscribeIntervalMs = subscribeIntervalMs
        self.backoff = backoff
        var cont: AsyncStream<SessionEvent>.Continuation!
        self.events = AsyncStream { cont = $0 }
        self.continuation = cont
    }

    /// Start connecting; returns at once. Events arrive on `events`.
    public func start() {
        guard loop == nil, !stopped else { return }
        loop = Task { await self.run() }
    }

    /// Stop: the current connection closes and no retry follows.
    public func stop() async {
        stopped = true
        resumeWait()
        await current?.disconnect()
        current = nil
        loop?.cancel()
        continuation.finish()
    }

    /// Cut the wait before the next attempt (the shell's `R`).
    public func retryNow() {
        resumeWait()
    }

    private func resumeWait() {
        guard let w = wake else { return }
        wake = nil
        w.resume()
    }

    /// Send on the current connection; `.notConnected` between connections.
    public func send(_ command: ShellCommand) async throws -> CommandResponse {
        guard let current else { throw ControlPlaneClient.ClientError.notConnected }
        return try await current.send(command)
    }

    private func run() async {
        var attempt = 0
        while !stopped {
            let client = ControlPlaneClient(config: config, catalog: catalog)
            current = client
            do {
                let hello = try await client.connect()
                attempt = 0
                continuation.yield(.connected(hello))
                _ = try? await client.send(.telemetrySubscribe(intervalMs: subscribeIntervalMs))
                for await frame in client.pushes {
                    continuation.yield(.frame(frame))
                }
            } catch {
                await client.disconnect()
            }
            if current === client { current = nil }
            if stopped { break }
            continuation.yield(.lost(Date()))
            attempt += 1
            let delay = backoff(attempt)
            continuation.yield(.retrying(attempt: attempt, after: delay))
            await pause(delay)
        }
    }

    /// Wait `delay`, or until `retryNow()` or `stop()`.
    private func pause(_ delay: Duration) async {
        pauseGeneration += 1
        let generation = pauseGeneration
        await withCheckedContinuation { (cont: CheckedContinuation<Void, Never>) in
            wake = cont
            Task {
                try? await Task.sleep(for: delay)
                self.timerFired(generation)
            }
        }
    }

    private func timerFired(_ generation: Int) {
        // Only the wait this timer was set for; retryNow or stop may have
        // ended it already, and a later wait has its own timer.
        guard generation == pauseGeneration else { return }
        resumeWait()
    }
}
