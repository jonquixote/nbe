// swift-tools-version:5.9
// The nbe operator shell (Prompt 13; the design draft of 2026-11-07). A
// client of the control plane's one WebSocket endpoint (SPEC §5.3): every
// button is a §16 command, every indicator §10.1 telemetry or the status
// snapshot. No engine is embedded yet (the draft's finding 3).
import PackageDescription

let package = Package(
    name: "NBEMacOS",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "NBEProtocol", targets: ["NBEProtocol"]),
        .library(name: "NBEClient", targets: ["NBEClient"]),
        .executable(name: "nbe-operator-shell", targets: ["NBEOperatorShell"]),
    ],
    targets: [
        .target(
            name: "NBEProtocol",
            resources: [.copy("Resources/commands.v0.4.json")]
        ),
        .target(name: "NBEClient", dependencies: ["NBEProtocol"]),
        // The operator shell's model and its pure logic, in a library so the
        // tests can import it (the executable target cannot be imported).
        .target(name: "NBEShell", dependencies: ["NBEProtocol", "NBEClient"]),
        .executableTarget(name: "NBEOperatorShell", dependencies: ["NBEProtocol", "NBEClient", "NBEShell"]),
        .testTarget(
            name: "NBEProtocolTests",
            dependencies: ["NBEProtocol", "NBEClient", "NBEShell"],
            resources: [.copy("Fixtures")]
        ),
    ]
)
