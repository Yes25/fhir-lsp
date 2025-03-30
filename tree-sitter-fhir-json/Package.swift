// swift-tools-version:5.3
import PackageDescription

let package = Package(
    name: "TreeSitterFhirJson",
    products: [
        .library(name: "TreeSitterFhirJson", targets: ["TreeSitterFhirJson"]),
    ],
    dependencies: [
        .package(url: "https://github.com/ChimeHQ/SwiftTreeSitter", from: "0.8.0"),
    ],
    targets: [
        .target(
            name: "TreeSitterFhirJson",
            dependencies: [],
            path: ".",
            sources: [
                "src/parser.c",
                // NOTE: if your language has an external scanner, add it here.
            ],
            resources: [
                .copy("queries")
            ],
            publicHeadersPath: "bindings/swift",
            cSettings: [.headerSearchPath("src")]
        ),
        .testTarget(
            name: "TreeSitterFhirJsonTests",
            dependencies: [
                "SwiftTreeSitter",
                "TreeSitterFhirJson",
            ],
            path: "bindings/swift/TreeSitterFhirJsonTests"
        )
    ],
    cLanguageStandard: .c11
)
