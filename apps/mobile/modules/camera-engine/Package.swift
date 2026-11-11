// swift-tools-version: 5.9

// Builds and tests the protocol and hand gesture sources on their own, without Xcode, CocoaPods or
// React Native:
//
//     cd apps/mobile/modules/camera-engine && swift test
//
// The app itself does not use this package. CocoaPods compiles the same files as part of the
// PairCamCameraEngine pod (see `ios/PairCamCameraEngine.podspec`), so the files in each target
// must not import one another's module.

import PackageDescription

let package = Package(
    name: "PairCamProtocol",
    platforms: [
        .iOS(.v16),
        .macOS(.v13),
    ],
    products: [
        .library(name: "PairCamProtocol", targets: ["PairCamProtocol"]),
        .library(name: "PairCamGestures", targets: ["PairCamGestures"]),
    ],
    targets: [
        .target(
            name: "PairCamProtocol",
            path: "ios/Protocol"
        ),
        .target(
            name: "PairCamGestures",
            path: "ios/Gestures"
        ),
        .testTarget(
            name: "PairCamProtocolTests",
            dependencies: ["PairCamProtocol", "PairCamGestures"],
            path: "tests/PairCamProtocolTests",
            resources: [.copy("Resources/v0.json")]
        ),
    ]
)
