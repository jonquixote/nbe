import NBEProtocol
import SwiftUI

/// §10.8 / §17.1: the indications the operator UI must show.
enum Indication {
    static func color(forShowState s: String?) -> Color {
        switch s {
        case "RUNNING": .red
        case "READY", "LOADED": .gray
        case "ERROR": .red
        default: .secondary
        }
    }

    static func color(forOutput s: String?) -> Color {
        switch s {
        case "live", "recording": .red
        case "reconnecting", "starting", "stopping", "held": .yellow
        case "error", "failed": .orange
        default: .gray
        }
    }
}

struct HeaderBar: View {
    @ObservedObject var model: ShellModel

    var body: some View {
        HStack(spacing: 16) {
            Text(model.status.showState ?? model.telemetry.showState ?? "—")
                .font(.title2.bold())
                .padding(.horizontal, 10).padding(.vertical, 4)
                .background(Indication.color(forShowState: model.status.showState).opacity(0.25), in: .rect(cornerRadius: 6))
            Text("frame \(model.telemetry.masterClockFrame.map(String.init) ?? "—")").monospacedDigit()
            Text("sv \(model.status.stateVersion.map(String.init) ?? "—")").monospacedDigit().foregroundStyle(.secondary)
            Spacer()
            Text(connectionText).foregroundStyle(.secondary)
        }
        .padding(8)
    }

    private var connectionText: String {
        switch model.connection {
        case .disconnected: "disconnected"
        case .connecting: "connecting…"
        case .connected(let role): "connected as \(role.rawValue)"
        case .failed(let why): "failed: \(why)"
        case .fixture: "fixture (no control plane)"
        }
    }
}

/// §5.6: Preview and View side by side, TAKE between them. The picture is a
/// placeholder: the View's surface is not wired yet (the draft's finding 3).
struct BusesPanel: View {
    @ObservedObject var model: ShellModel
    @State private var transition: ShellCommand.Transition = .cut

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            bus("PREVIEW", item: model.status.previewItem, tint: .green)
            VStack(spacing: 8) {
                Picker("", selection: $transition) {
                    ForEach(ShellCommand.Transition.allCases, id: \.self) { Text($0.rawValue) }
                }
                .frame(width: 90)
                button("TAKE", .take(transition: transition, durationFrames: transition == .cut ? nil : 15))
                    .keyboardShortcut(.space, modifiers: [])
            }
            bus("VIEW", item: model.status.viewItem, tint: .red)
        }
        .padding(8)
    }

    private func bus(_ title: String, item: String?, tint: Color) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.caption.bold()).foregroundStyle(tint)
            ZStack {
                Rectangle().fill(.black)
                Text("picture not wired\n(the View's surface, finding 3)")
                    .font(.caption).multilineTextAlignment(.center).foregroundStyle(.gray)
            }
            .frame(width: 352, height: 198)
            .overlay(RoundedRectangle(cornerRadius: 2).stroke(tint, lineWidth: 2))
            Text(item ?? "—").font(.headline.monospaced())
        }
    }

    private func button(_ title: String, _ command: ShellCommand) -> some View {
        Button { model.run(command) } label: {
            Text(title).font(.title3.bold()).frame(width: 90, height: 36)
        }
        .buttonStyle(.borderedProminent)
        .tint(.red)
        .controlSize(.large)
        .disabled(!model.may(command))
        .help(model.why(command))
    }
}

struct ShowPanel: View {
    @ObservedObject var model: ShellModel
    @State private var packagePath = ""

    var body: some View {
        GroupBox("Show") {
            VStack(alignment: .leading, spacing: 6) {
                HStack {
                    TextField("package path", text: $packagePath)
                    action("Load", .showLoad(packagePath: packagePath)).disabled(packagePath.isEmpty)
                }
                HStack {
                    action("Preflight", .showPreflight)
                    action("Start", .showStart)
                    action("Stop", .showStop(force: false))
                }
                Text("package: \(model.status.package?.path ?? "none") · preflight: \(model.status.preflightPassed.map { $0 ? "passed" : "not passed" } ?? "—")")
                    .font(.caption).foregroundStyle(.secondary)
                Text("The rundown needs a §16 command that returns it (the draft's finding 1).")
                    .font(.caption2).foregroundStyle(.orange)
            }
        }
    }

    private func action(_ title: String, _ command: ShellCommand) -> some View {
        Button(title) { model.run(command) }.disabled(!model.may(command)).help(model.why(command))
    }
}

struct GraphicsPanel: View {
    @ObservedObject var model: ShellModel
    @State private var elementId = "lowerThird"
    @State private var headline = ""

    var body: some View {
        GroupBox("Graphics") {
            VStack(alignment: .leading, spacing: 6) {
                HStack {
                    TextField("element", text: $elementId).frame(width: 140)
                    Button("Show") { model.run(.elementToggle(elementId: elementId, visible: true)) }
                        .disabled(!model.may(.elementToggle(elementId: elementId, visible: true)))
                    Button("Hide") { model.run(.elementToggle(elementId: elementId, visible: false)) }
                        .disabled(!model.may(.elementToggle(elementId: elementId, visible: false)))
                }
                HStack {
                    TextField("breaking headline", text: $headline)
                    Button("Breaking") { model.run(.breakingShow(headline: headline, subhead: nil)) }
                        .disabled(headline.isEmpty || !model.may(.breakingShow(headline: headline, subhead: nil)))
                    Button("Clear") { model.run(.breakingHide) }.disabled(!model.may(.breakingHide))
                }
                Text("on air: \((model.status.visibleOverlays ?? []).joined(separator: ", "))")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
    }
}

struct SoundboardPanel: View {
    @ObservedObject var model: ShellModel
    let pads = ["stab", "sting", "bed", "whoosh"]

    var body: some View {
        GroupBox("Soundboard") {
            HStack {
                ForEach(Array(pads.enumerated()), id: \.offset) { i, pad in
                    Button(pad) { model.run(.soundboardPlay(assetId: pad)) }
                        .keyboardShortcut(KeyEquivalent(Character(String(i + 1))), modifiers: [.command])
                        .disabled(!model.may(.soundboardPlay(assetId: pad)))
                }
                Button("Stop all") { model.run(.soundboardStopAll) }.disabled(!model.may(.soundboardStopAll))
            }
        }
    }
}

struct OutputsPanel: View {
    @ObservedObject var model: ShellModel

    var body: some View {
        GroupBox("Outputs") {
            Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 6) {
                GridRow {
                    Text("Record")
                    state(model.status.recordState ?? model.telemetry.recordState)
                    Button("Start") { model.run(.recordStart(outputId: nil)) }.disabled(!model.may(.recordStart(outputId: nil)))
                    Button("Stop") { model.run(.recordStop) }.disabled(!model.may(.recordStop))
                    Text(model.telemetry.recordSpaceMib.map { "\(Int($0)) MiB free" } ?? "").font(.caption)
                }
                GridRow {
                    Text("Stream")
                    state(model.status.streamState ?? model.telemetry.streamState)
                    Button("Start") { model.run(.streamStart(outputId: nil)) }.disabled(!model.may(.streamStart(outputId: nil)))
                    Button("Stop") { model.run(.streamStop) }.disabled(!model.may(.streamStop))
                    Text(model.telemetry.streamBufferMs.map { "buffer \(Int($0)) ms" } ?? "").font(.caption)
                }
            }
        }
    }

    private func state(_ s: String?) -> some View {
        Text(s ?? "—").bold().foregroundStyle(Indication.color(forOutput: s))
    }
}

struct HealthPanel: View {
    @ObservedObject var model: ShellModel

    var body: some View {
        GroupBox("Health") {
            let t = model.telemetry
            VStack(alignment: .leading, spacing: 4) {
                row("dropped frames", t.droppedFramesTotal.map(String.init))
                row("audio underruns", t.audioUnderrunsTotal.map(String.init))
                row("watchdog trips / clears", zip2(t.watchdogTripsTotal, t.watchdogClearsTotal))
                row("degradation rung", t.degradationRung.map(String.init))
                row("audio device", t.audioDevice.map { "\($0.state) \($0.name ?? "")" })
                row("record tap", t.recordTapPath)
                Divider()
                ForEach((t.busPeakDbfs ?? [:]).keys.sorted(), id: \.self) { bus in
                    Meter(bus: bus, dbfs: (t.busPeakDbfs?[bus]).flatMap { $0 })
                }
            }
            .font(.callout.monospacedDigit())
        }
    }

    private func row(_ label: String, _ value: String?) -> some View {
        HStack { Text(label).foregroundStyle(.secondary); Spacer(); Text(value ?? "—") }
    }

    private func zip2(_ a: Int?, _ b: Int?) -> String? {
        guard let a, let b else { return nil }
        return "\(a) / \(b)"
    }
}

/// A bus meter from `busPeakDbfs` (−60 to 0 dBFS). At 1 Hz this strobes
/// (finding R2); the shell asks for 100 ms ticks.
struct Meter: View {
    let bus: String
    let dbfs: Double?

    var body: some View {
        HStack {
            Text(bus).frame(width: 90, alignment: .leading)
            GeometryReader { g in
                let level = max(0, min(1, ((dbfs ?? -60) + 60) / 60))
                ZStack(alignment: .leading) {
                    Rectangle().fill(.quaternary)
                    Rectangle().fill(level > 0.9 ? Color.red : level > 0.75 ? .yellow : .green)
                        .frame(width: g.size.width * level)
                }
            }
            .frame(height: 10)
            Text(dbfs.map { String(format: "%.1f", $0) } ?? "—").frame(width: 50, alignment: .trailing)
        }
    }
}

/// §10.8: ERROR is a red banner, FALLBACK a full-screen warning.
struct AlertsBanner: View {
    @ObservedObject var model: ShellModel

    var body: some View {
        let alerts = model.status.alerts ?? model.telemetry.alerts ?? []
        VStack(spacing: 2) {
            ForEach(alerts) { a in
                Text("\(a.severity.uppercased()): \(a.reason)")
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(6)
                    .background(a.severity == "critical" ? Color.red : .orange)
                    .foregroundStyle(.white)
            }
            if let p = model.lastProblem {
                Text(p).frame(maxWidth: .infinity, alignment: .leading).padding(6)
                    .background(Color.red.opacity(0.8)).foregroundStyle(.white)
                    .onTapGesture { model.lastProblem = nil }
            }
        }
    }
}

struct FallbackCurtain: View {
    var body: some View {
        ZStack {
            Color.red.opacity(0.85)
            Text("FALLBACK ON AIR").font(.system(size: 64, weight: .black)).foregroundStyle(.white)
        }
        .ignoresSafeArea()
        .allowsHitTesting(false)
    }
}

/// The rundown (§17.1's colours) waits on a §16 command that returns the
/// sequences and items (the draft's finding 1). Until then it shows what the
/// status snapshot does carry: the items on the two buses.
struct RundownPanel: View {
    @ObservedObject var model: ShellModel

    var body: some View {
        GroupBox("Rundown") {
            VStack(alignment: .leading, spacing: 6) {
                row(model.status.viewItem, state: "LIVE", color: .red)
                row(model.status.previewItem, state: "ARMED", color: .yellow)
                Text("Sequences and items need a §16 command that returns them (finding 1); §17.1's READY, PLAYING, DONE, MISSING and ERROR colours draw from it.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func row(_ item: String?, state: String, color: Color) -> some View {
        HStack {
            RoundedRectangle(cornerRadius: 3).fill(color).frame(width: 10, height: 18)
            Text(item ?? "—").font(.body.monospaced())
            Spacer()
            Text(state).font(.caption.bold()).foregroundStyle(color)
        }
    }
}
