// swift-tools-version: 6.0
// recordem-parakeet: Parakeet TDT speech recognition (FluidAudio, CoreML on the Neural Engine) as a helper process of
// the recorder, which is Rust. See Sources/recordem-parakeet/main.swift for the protocol.
import PackageDescription

let package = Package(
    name: "recordem-parakeet",
    platforms: [.macOS(.v14)],
    dependencies: [
        // Apache-2.0. Pinned: the recorder speaks to this helper, and the helper to this exact API.
        .package(url: "https://github.com/FluidInference/FluidAudio.git", revision: "82eb4c1e0ab67e1c7d8de5092aa42a8f74cbd7b8"),
    ],
    targets: [
        .executableTarget(
            name: "recordem-parakeet",
            dependencies: [.product(name: "FluidAudio", package: "FluidAudio")]
        ),
    ]
)
