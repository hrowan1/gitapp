// swift-tools-version: 6.2
// The dev-inbox-recorder sidecar: recording, on-device transcription and summaries, and speaker
// separation (FluidAudio). Built by ../build.rs into ../binaries/.
import PackageDescription

let package = Package(
    name: "dev-inbox-recorder",
    platforms: [.macOS("14.2")],
    dependencies: [
        // Speaker diarization on the Neural Engine (Apache-2.0). Pinned; `traits: []` leaves out its
        // prebuilt text-normalisation binary, which diarization doesn't use.
        .package(url: "https://github.com/FluidInference/FluidAudio.git", exact: "0.17.5", traits: []),
    ],
    targets: [
        .executableTarget(
            name: "dev-inbox-recorder",
            dependencies: [.product(name: "FluidAudio", package: "FluidAudio")],
            path: "Sources/Recorder",
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
    ]
)
