import AppKit
import NBEProtocol
import SwiftUI

/// The operator shell. Configuration from the environment, so no token is
/// ever written into the app: `NBE_SHELL_URL` (default
/// `ws://127.0.0.1:8462/nbe/v0.3`), `NBE_SHELL_TOKEN`, `NBE_SHELL_ROLE`
/// (default `operator`). `--fixture` draws the spec's example with no
/// control plane.
@main
struct OperatorShellApp: App {
    @StateObject private var model = ShellModel()

    init() {
        // `--snapshot <path.png>`: render the fixture state to a PNG and exit,
        // with no window and no control plane (a look at the layout).
        let args = CommandLine.arguments
        if let i = args.firstIndex(of: "--snapshot"), i + 1 < args.count {
            Snapshot.write(to: URL(fileURLWithPath: args[i + 1]))
            exit(0)
        }
        // Run from `swift run`, with no app bundle: become a regular app with
        // a Dock icon and key windows.
        NSApplication.shared.setActivationPolicy(.regular)
        NSApplication.shared.activate(ignoringOtherApps: true)
    }

    var body: some Scene {
        WindowGroup("nbe — operator") {
            ShellWindow(model: model)
                .frame(minWidth: 1100, minHeight: 720)
                .task { start() }
        }
    }

    @MainActor private func start() {
        let env = ProcessInfo.processInfo.environment
        if CommandLine.arguments.contains("--fixture") {
            model.loadFixture()
            return
        }
        guard let token = env["NBE_SHELL_TOKEN"], !token.isEmpty else {
            model.connection = .failed("NBE_SHELL_TOKEN is not set (or run with --fixture)")
            return
        }
        let url = URL(string: env["NBE_SHELL_URL"] ?? "ws://127.0.0.1:\(defaultPort)\(wsPath)")!
        let role = Role(rawValue: env["NBE_SHELL_ROLE"] ?? "operator") ?? .operator
        model.connect(url: url, token: token, role: role)
    }
}

struct ShellWindow: View {
    @ObservedObject var model: ShellModel

    var body: some View {
        ZStack {
            VStack(spacing: 0) {
                HeaderBar(model: model)
                AlertsBanner(model: model)
                Divider()
                HStack(alignment: .top, spacing: 12) {
                    VStack(spacing: 12) {
                        BusesPanel(model: model)
                        HStack(alignment: .top) {
                            GraphicsPanel(model: model)
                            SoundboardPanel(model: model)
                        }
                        RundownPanel(model: model)
                    }
                    VStack(spacing: 12) {
                        ShowPanel(model: model)
                        OutputsPanel(model: model)
                        HealthPanel(model: model)
                    }
                    .frame(width: 380)
                }
                .padding(12)
                if !model.log.isEmpty {
                    Divider()
                    List(model.log, id: \.self) { Text($0).font(.caption.monospaced()) }
                        .frame(height: 110)
                }
            }
            if model.status.fallbackActive == true || model.telemetry.fallbackActive == true {
                FallbackCurtain()
            }
        }
    }
}

/// The fixture state drawn offscreen into a real AppKit view hierarchy
/// (`cacheDisplay`), so controls draw as they do in a window. No server.
@MainActor
enum Snapshot {
    static func write(to url: URL) {
        _ = NSApplication.shared
        let model = ShellModel()
        model.loadFixture()
        let size = NSSize(width: 1280, height: 820)
        let root = ShellWindow(model: model)
            .frame(width: size.width, height: size.height, alignment: .top)
            .background(Color(nsColor: .windowBackgroundColor))
            .environment(\.colorScheme, .dark)
        let hosting = NSHostingView(rootView: root)
        hosting.appearance = NSAppearance(named: .darkAqua)
        hosting.frame = NSRect(origin: .zero, size: size)
        let window = NSWindow(contentRect: hosting.frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.contentView = hosting
        hosting.layoutSubtreeIfNeeded()
        // One turn of the run loop so SwiftUI commits its first layout.
        RunLoop.main.run(until: Date().addingTimeInterval(0.3))
        guard let rep = hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds) else {
            FileHandle.standardError.write(Data("snapshot: no bitmap\n".utf8))
            exit(1)
        }
        hosting.cacheDisplay(in: hosting.bounds, to: rep)
        guard let png = rep.representation(using: .png, properties: [:]) else { exit(1) }
        do { try png.write(to: url) } catch {
            FileHandle.standardError.write(Data("snapshot: \(error)\n".utf8))
            exit(1)
        }
    }
}
