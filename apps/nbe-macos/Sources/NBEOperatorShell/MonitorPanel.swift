import SwiftUI
import WebKit

/// The program monitor (the user's ruling of 2026-11-12, item 2): the
/// engine streams RTMP into the media server, and the shell plays it back
/// over WHEP in a web view — the media server's own reader page. No engine
/// window, no readback. Picture only: the media server offers no WebRTC
/// audio for the engine's AAC (the spike's finding), so the operator
/// listens locally.
struct MonitorPanel: View {
    @State private var url = ProcessInfo.processInfo.environment["NBE_MONITOR_URL"] ?? "http://127.0.0.1:8889/live/program/"
    @State private var shown: URL?

    var body: some View {
        GroupBox("Program monitor (WHEP)") {
            VStack(alignment: .leading, spacing: 6) {
                HStack {
                    TextField("WHEP reader page", text: $url).font(.caption.monospaced())
                    Button(shown == nil ? "Open" : "Reload") { shown = URL(string: url) }
                        .disabled(URL(string: url) == nil)
                }
                ZStack {
                    Rectangle().fill(.black)
                    if let shown {
                        WebView(url: shown)
                    } else {
                        Text("not open").font(.caption).foregroundStyle(.secondary)
                    }
                }
                .aspectRatio(16.0 / 9.0, contentMode: .fit)
            }
        }
    }
}

private struct WebView: NSViewRepresentable {
    let url: URL

    func makeNSView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        config.mediaTypesRequiringUserActionForPlayback = []
        let view = WKWebView(frame: .zero, configuration: config)
        view.load(URLRequest(url: url))
        return view
    }

    func updateNSView(_ view: WKWebView, context: Context) {
        if view.url != url { view.load(URLRequest(url: url)) }
    }
}
