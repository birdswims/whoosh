import SwiftUI

public enum Theme {
    public static var canvas: Color { Color(nsColor: .windowBackgroundColor) }
    public static var card: Color { Color(nsColor: .controlBackgroundColor) }
    public static var hairline: Color { Color.primary.opacity(0.08) }

    public static func signal(_ scheme: ColorScheme) -> Color {
        scheme == .dark
            ? Color(red: 0.58, green: 0.84, blue: 0.74)
            : Color(red: 0.13, green: 0.45, blue: 0.37)
    }

    public static func ink(_ scheme: ColorScheme) -> Color {
        scheme == .dark ? .white : Color(red: 0.11, green: 0.11, blue: 0.105)
    }

    public static func onInk(_ scheme: ColorScheme) -> Color {
        scheme == .dark ? Color(red: 0.10, green: 0.10, blue: 0.10) : .white
    }
}

public struct CardStyle: ViewModifier {
    public init() {}

    public func body(content: Content) -> some View {
        content
            .padding(16)
            .background(Theme.card)
            .clipShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 18, style: .continuous)
                    .strokeBorder(Theme.hairline, lineWidth: 1)
            )
            .shadow(color: Color.black.opacity(0.05), radius: 16, y: 8)
    }
}

public struct SectionTitle: View {
    public var text: String

    public init(_ text: String) {
        self.text = text
    }

    public var body: some View {
        Text(text)
            .font(.system(size: 11, weight: .semibold))
            .tracking(1.15)
            .foregroundStyle(.secondary)
            .textCase(.uppercase)
    }
}

public func humanSize(_ bytes: UInt64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"]
    var value = Double(bytes)
    var unit = 0
    while value >= 1024, unit < units.count - 1 {
        value /= 1024
        unit += 1
    }
    if unit == 0 {
        return "\(bytes) B"
    }
    return String(format: "%.1f %@", value, units[unit])
}

public func groupedFingerprint(_ hex: String) -> String {
    let clean = hex.lowercased().filter { !$0.isWhitespace }
    var groups: [String] = []
    var current = ""
    for character in clean {
        current.append(character)
        if current.count == 4 {
            groups.append(current)
            current = ""
        }
    }
    if !current.isEmpty {
        groups.append(current)
    }
    var lines: [String] = []
    var line: [String] = []
    for group in groups {
        line.append(group)
        if line.count == 8 {
            lines.append(line.joined(separator: " "))
            line = []
        }
    }
    if !line.isEmpty {
        lines.append(line.joined(separator: " "))
    }
    return lines.joined(separator: "\n")
}

public func abbreviatePath(_ path: String) -> String {
    let home = FileManager.default.homeDirectoryForCurrentUser.path
    if path == home {
        return "~"
    }
    if path.hasPrefix(home + "/") {
        return "~" + path.dropFirst(home.count)
    }
    return path
}

public func symbolName(for fileName: String, kind: String? = nil) -> String {
    if let kind {
        switch kind {
        case "photo": return "photo"
        case "video": return "film"
        case "audio": return "waveform"
        default: break
        }
    }
    let ext = (fileName as NSString).pathExtension.lowercased()
    switch ext {
    case "jpg", "jpeg", "png", "gif", "heic", "heif", "webp", "tif", "tiff", "avif":
        return "photo"
    case "mp4", "mov", "m4v", "webm", "mkv", "avi":
        return "film"
    case "mp3", "m4a", "wav", "flac", "aac", "aiff":
        return "waveform"
    default:
        return "doc"
    }
}
