import AppKit
import SwiftUI
import WhooshUI

struct RootView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.colorScheme) private var scheme
    @State private var dropping = false

    var body: some View {
        ZStack {
            Theme.canvas
            VStack(alignment: .leading, spacing: 14) {
                header
                if !model.warnings.isEmpty {
                    Text(model.warnings.joined(separator: " "))
                        .font(.system(size: 12))
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                HStack(alignment: .top, spacing: 14) {
                    SendCard(dropping: dropping)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                    NearbyCard()
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                }
                .frame(maxHeight: .infinity)
                ActivityCard()
                footer
            }
            .padding(.top, 36)
            .padding(.horizontal, 22)
            .padding(.bottom, 16)

            if let banner = model.banner {
                VStack {
                    Text(banner)
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(.primary)
                        .multilineTextAlignment(.center)
                        .lineLimit(3)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 8)
                        .background(.ultraThinMaterial, in: Capsule())
                        .shadow(color: .black.opacity(0.08), radius: 12, y: 4)
                        .padding(.top, 8)
                        .padding(.horizontal, 40)
                    Spacer()
                }
                .transition(.opacity)
            }

            if let offer = model.currentOffer {
                OfferOverlay(offer: offer)
            } else if model.trustPeer != nil {
                TrustOverlay()
            }

            if model.engineDown {
                EngineDownOverlay()
            }
        }
        .animation(.easeOut(duration: 0.16), value: model.banner)
        .animation(.easeOut(duration: 0.16), value: model.currentOffer?.id)
        .onDrop(of: [.fileURL], isTargeted: $dropping) { providers in
            model.takeDrop(providers)
        }
    }

    private var header: some View {
        HStack(spacing: 12) {
            WhooshMark()
                .frame(width: 34, height: 34)
            VStack(alignment: .leading, spacing: 1) {
                Text("Whoosh")
                    .font(.system(size: 20, weight: .semibold))
                    .tracking(-0.3)
                Text(model.receiving ? "On this network" : "Not receiving")
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 12)
            if model.requirePin, let pin = model.pin, !pin.isEmpty {
                HStack(spacing: 6) {
                    Text("PIN")
                        .font(.system(size: 9, weight: .semibold))
                        .tracking(0.8)
                        .foregroundStyle(.secondary)
                    Text(pin)
                        .font(.system(size: 13, weight: .semibold, design: .monospaced))
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 5)
                .background(Capsule().fill(Color.primary.opacity(0.06)))
                .help("People sending to this Mac enter this pin.")
            }
            HStack(spacing: 6) {
                Circle()
                    .fill(model.receiving ? Theme.signal(scheme) : Color.secondary.opacity(0.45))
                    .frame(width: 7, height: 7)
                Text(model.receiving ? "Receiving" : "Paused")
                    .font(.system(size: 12, weight: .medium))
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .background(Capsule().fill(Color.primary.opacity(0.05)))
            SettingsLink {
                Image(systemName: "gearshape")
                    .font(.system(size: 14, weight: .medium))
                    .frame(width: 28, height: 28)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help("Settings")
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var footer: some View {
        HStack(spacing: 12) {
            Button(action: model.revealSaveFolder) {
                HStack(spacing: 6) {
                    Image(systemName: "folder")
                        .font(.system(size: 12))
                    Text(model.saveDir.isEmpty ? "Choose a folder" : abbreviatePath(model.saveDir))
                        .font(.system(size: 12))
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                .foregroundStyle(.secondary)
            }
            .buttonStyle(.plain)
            .help("Show save folder in Finder")
            Spacer()
            Toggle("Receiving", isOn: Binding(
                get: { model.receiving },
                set: { model.setReceiving($0) }
            ))
            .toggleStyle(.switch)
            .controlSize(.small)
        }
    }
}

private struct SendCard: View {
    @Environment(AppModel.self) private var model
    var dropping: Bool

    var body: some View {
        @Bindable var model = model
        VStack(alignment: .leading, spacing: 12) {
            SectionTitle("Send")
            dropWell
            if !model.files.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(alignment: .top, spacing: 8) {
                        ForEach(model.files) { file in
                            FileChip(file: file)
                        }
                    }
                    .padding(.top, 2)
                    .padding(.trailing, 4)
                }
            }
            Spacer(minLength: 4)
            if model.selectedPeer?.via == "whoosh" {
                TextField("Pin, if they asked for one", text: $model.outgoingPin)
                    .textFieldStyle(.plain)
                    .font(.system(size: 13, design: .monospaced))
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(
                        RoundedRectangle(cornerRadius: 10, style: .continuous)
                            .fill(Color.primary.opacity(0.04))
                    )
            }
            Text(hint)
                .font(.system(size: 12))
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack {
                Spacer()
                PrimaryButton(title: model.sendTitle, enabled: model.canSend, action: model.send)
                    .frame(maxWidth: 240)
            }
        }
        .modifier(CardStyle())
    }

    private var dropWell: some View {
        VStack(spacing: 6) {
            Image(systemName: dropping ? "arrow.down.doc.fill" : "arrow.down.doc")
                .font(.system(size: 18, weight: .regular))
                .foregroundStyle(dropping ? AnyShapeStyle(Theme.signal(colorScheme)) : AnyShapeStyle(.secondary))
            Text(dropping ? "Drop to add" : "Drop files")
                .font(.system(size: 13, weight: .medium))
            Button("Choose…", action: model.chooseFiles)
                .buttonStyle(.plain)
                .font(.system(size: 12))
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity)
        .frame(height: model.files.isEmpty ? 124 : 88)
        .background(
            RoundedRectangle(cornerRadius: 14, style: .continuous)
                .fill(Color.primary.opacity(dropping ? 0.04 : 0.02))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 14, style: .continuous)
                .strokeBorder(
                    dropping ? Theme.signal(colorScheme) : Color.primary.opacity(0.16),
                    style: StrokeStyle(lineWidth: 1, dash: [5, 4])
                )
        )
    }

    @Environment(\.colorScheme) private var colorScheme

    private var hint: String {
        if model.files.isEmpty {
            return "Drop files here, or choose them."
        }
        guard let peer = model.selectedPeer else {
            return "Choose a device nearby."
        }
        if peer.via == "whoosh", !peer.trusted {
            return "You confirm this device’s fingerprint before the first send."
        }
        let count = model.files.count
        let noun = count == 1 ? "file" : "files"
        return "\(count) \(noun) to \(peer.name)."
    }
}

private struct FileChip: View {
    @Environment(AppModel.self) private var model
    var file: SendFile

    private var kind: MediaThumbnail.Kind? {
        MediaThumbnail.kind(of: file.path)
    }

    var body: some View {
        VStack(spacing: 5) {
            preview
            Text(file.name)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(width: MediaThumbnail.side + 8)
        }
        .help(file.name)
    }

    private var preview: some View {
        ZStack(alignment: .topTrailing) {
            ZStack {
                if let image = model.thumbnails[file.path] {
                    Image(nsImage: image)
                        .resizable()
                        .scaledToFill()
                } else {
                    Image(systemName: symbolName(for: file.name))
                        .font(.system(size: 16))
                        .foregroundStyle(.secondary)
                }
                if kind == .video, model.thumbnails[file.path] != nil {
                    Image(systemName: "play.fill")
                        .font(.system(size: 8, weight: .bold))
                        .foregroundStyle(.white)
                        .frame(width: 16, height: 16)
                        .background(Circle().fill(.black.opacity(0.5)))
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomLeading)
                        .padding(5)
                }
            }
            .frame(width: MediaThumbnail.side, height: MediaThumbnail.side)
            .background(Color.primary.opacity(0.06))
            .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .strokeBorder(Color.primary.opacity(0.10), lineWidth: 1)
            )
            .accessibilityHidden(true)

            Button {
                model.removeFile(file)
            } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 7, weight: .bold))
                    .foregroundStyle(.primary)
                    .frame(width: 16, height: 16)
                    .background(Circle().fill(Theme.card))
                    .overlay(Circle().strokeBorder(Theme.hairline, lineWidth: 1))
            }
            .buttonStyle(.plain)
            .offset(x: 5, y: -5)
            .help("Remove")
            .accessibilityLabel("Remove \(file.name)")
        }
        .padding(.top, 6)
        .padding(.trailing, 6)
    }
}

private struct NearbyCard: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                SectionTitle("Nearby")
                Spacer()
                if !model.sawPeers {
                    ProgressView()
                        .controlSize(.small)
                }
            }
            if model.peers.isEmpty {
                Text(model.sawPeers ? "No devices nearby" : "Looking…")
                    .font(.system(size: 13))
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollView {
                    LazyVStack(spacing: 2) {
                        ForEach(model.peers) { peer in
                            PeerRow(peer: peer, selected: peer.id == model.selectedPeerID)
                                .onTapGesture { model.select(peer) }
                        }
                    }
                }
            }
        }
        .modifier(CardStyle())
    }
}

private struct PeerRow: View {
    @Environment(\.colorScheme) private var scheme
    var peer: Peer
    var selected: Bool
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(peer.name)
                    .font(.system(size: 13, weight: .medium))
                    .lineLimit(1)
                Text(peer.detail)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 8)
            Text(protocolLabel(peer.via))
                .font(.system(size: 10, weight: .medium))
                .foregroundStyle(.secondary)
                .padding(.horizontal, 7)
                .padding(.vertical, 3)
                .background(Capsule().fill(Color.primary.opacity(0.06)))
            Circle()
                .fill(selected ? Theme.signal(scheme) : Color.clear)
                .frame(width: 6, height: 6)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .background(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(selected ? Color.primary.opacity(0.07) : (hovering ? Color.primary.opacity(0.04) : Color.clear))
        )
        .contentShape(Rectangle())
        .onHover { inside in
            hovering = inside
            if inside {
                NSCursor.pointingHand.set()
            } else {
                NSCursor.arrow.set()
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isButton)
        .accessibilityLabel("\(peer.name), \(protocolLabel(peer.via))")
    }
}

private struct ActivityCard: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionTitle("Activity")
            if model.activity.isEmpty {
                Text("Transfers show up here.")
                    .font(.system(size: 13))
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
            } else {
                ScrollView {
                    VStack(spacing: 0) {
                        ForEach(model.activity) { item in
                            ActivityRow(item: item)
                        }
                    }
                }
            }
        }
        .frame(height: 168)
        .modifier(CardStyle())
    }
}

private struct ActivityRow: View {
    @Environment(AppModel.self) private var model
    @Environment(\.colorScheme) private var scheme
    var item: ActivityItem

    var body: some View {
        HStack(spacing: 10) {
            stateMark
                .frame(width: 16)
            VStack(alignment: .leading, spacing: 1) {
                Text(item.title)
                    .font(.system(size: 13, weight: .medium))
                    .lineLimit(1)
                Text(subtitle)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer(minLength: 8)
            if item.state == "done", !item.paths.isEmpty {
                Button("Show") { model.showActivity(item) }
                    .buttonStyle(.plain)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.vertical, 6)
    }

    private var subtitle: String {
        if let bytes = item.bytes, item.state == "done" {
            return "\(item.detail) · \(humanSize(bytes))"
        }
        return item.detail
    }

    @ViewBuilder
    private var stateMark: some View {
        switch item.state {
        case "working":
            ProgressView().controlSize(.small)
        case "done":
            Image(systemName: "checkmark")
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(Theme.signal(scheme))
        case "failed":
            Image(systemName: "xmark")
                .font(.system(size: 10, weight: .bold))
                .foregroundStyle(Color(red: 0.70, green: 0.28, blue: 0.24))
        default:
            Image(systemName: "minus")
                .font(.system(size: 10, weight: .bold))
                .foregroundStyle(.secondary)
        }
    }
}

struct PrimaryButton: View {
    var title: String
    var enabled: Bool = true
    var action: () -> Void

    var body: some View {
        Button(action: { if enabled { action() } }) {
            Text(title)
        }
        .buttonStyle(CapsuleButtonStyle(enabled: enabled))
        .allowsHitTesting(enabled)
        .accessibilityAddTraits(enabled ? [] : .isStaticText)
    }
}

/// Draws the label after the button style, so macOS does not repaint it in the capsule color.
private struct CapsuleButtonStyle: ButtonStyle {
    var enabled: Bool
    @Environment(\.colorScheme) private var scheme

    func makeBody(configuration: Configuration) -> some View {
        let fill = enabled ? Theme.ink(scheme) : Color.primary.opacity(scheme == .dark ? 0.16 : 0.08)
        let text = enabled
            ? Theme.onInk(scheme)
            : (scheme == .dark ? Color.white.opacity(0.88) : Color.black.opacity(0.72))
        configuration.label
            .font(.system(size: 14, weight: .semibold))
            .lineLimit(1)
            .foregroundStyle(text)
            .padding(.horizontal, 18)
            .padding(.vertical, 9)
            .frame(maxWidth: .infinity)
            .background(Capsule().fill(fill))
            .opacity(configuration.isPressed && enabled ? 0.84 : 1)
            .contentShape(Capsule())
    }
}
