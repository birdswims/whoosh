import SwiftUI

/// Three arcs leaving a shared center. The mark is the Whoosh logo.
public struct WhooshMark: View {
    public var tint: Color

    public init(tint: Color = .primary) {
        self.tint = tint
    }

    public var body: some View {
        Canvas { context, size in
            let side = min(size.width, size.height)
            let center = CGPoint(x: size.width * 0.50, y: size.height * 0.66)
            let arcs: [(CGFloat, Double, CGFloat)] = [
                (0.40, 0.28, 0.072),
                (0.285, 0.58, 0.078),
                (0.165, 1.0, 0.086),
            ]
            for (radius, alpha, width) in arcs {
                var path = Path()
                path.addArc(
                    center: center,
                    radius: side * radius,
                    startAngle: .degrees(206),
                    endAngle: .degrees(338),
                    clockwise: false
                )
                context.stroke(
                    path,
                    with: .color(tint.opacity(alpha)),
                    style: StrokeStyle(lineWidth: side * width, lineCap: .round, lineJoin: .round)
                )
            }
        }
        .aspectRatio(1, contentMode: .fit)
        .accessibilityLabel("Whoosh")
    }
}

public struct AppIconArtwork: View {
    public init() {}

    public var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 229, style: .continuous)
                .fill(Color(red: 0.075, green: 0.086, blue: 0.082))
            RoundedRectangle(cornerRadius: 229, style: .continuous)
                .fill(
                    RadialGradient(
                        colors: [Color.white.opacity(0.16), Color.white.opacity(0)],
                        center: UnitPoint(x: 0.32, y: 0.22),
                        startRadius: 20,
                        endRadius: 640
                    )
                )
            WhooshMark(tint: Color(red: 0.945, green: 0.965, blue: 0.953))
                .frame(width: 620, height: 620)
                .offset(y: 8)
        }
        .frame(width: 1024, height: 1024)
    }
}

public struct LogoLockup: View {
    public init() {}

    public var body: some View {
        ZStack {
            Color(red: 0.965, green: 0.961, blue: 0.949)
            VStack(spacing: 28) {
                WhooshMark(tint: Color(red: 0.09, green: 0.11, blue: 0.10))
                    .frame(width: 280, height: 280)
                Text("Whoosh")
                    .font(.system(size: 72, weight: .semibold))
                    .foregroundStyle(Color(red: 0.09, green: 0.10, blue: 0.09))
                    .tracking(-1.5)
            }
        }
        .frame(width: 1024, height: 1024)
    }
}

public struct InstallBackdrop: View {
    public init() {}

    public var body: some View {
        ZStack(alignment: .top) {
            Color(red: 0.965, green: 0.961, blue: 0.949)
            VStack(spacing: 14) {
                WhooshMark(tint: Color(red: 0.10, green: 0.12, blue: 0.11))
                    .frame(width: 54, height: 54)
                    .padding(.top, 36)
                Text("Drag Whoosh to Applications")
                    .font(.system(size: 20, weight: .medium))
                    .foregroundStyle(Color.black.opacity(0.62))
            }
        }
        .frame(width: 720, height: 460)
    }
}
