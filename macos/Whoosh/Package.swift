// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "Whoosh",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "Whoosh", targets: ["Whoosh"])
    ],
    targets: [
        .target(name: "WhooshUI", path: "Sources/WhooshUI"),
        .target(name: "WhooshAirDrop", path: "Sources/WhooshAirDrop"),
        .testTarget(name: "WhooshAirDropTests", dependencies: ["WhooshAirDrop"]),
        .executableTarget(
            name: "Whoosh",
            dependencies: ["WhooshUI", "WhooshAirDrop"],
            path: "Sources/Whoosh"
        ),
        .executableTarget(
            name: "RenderIcon",
            dependencies: ["WhooshUI"],
            path: "Sources/RenderIcon"
        ),
    ]
)
