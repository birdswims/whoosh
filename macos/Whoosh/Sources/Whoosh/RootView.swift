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
                    .padding(.horizontal, 22)
                if model.screen == .home {
                    VStack(alignment: .leading, spacing: 14) {
                    if !model.warnings.isEmpty {
                        Text(model.warnings.joined(separator: " "))
                            .font(.system(size: 12))
                            .foregroundStyle(.secondary)
                            .lineLimit(2)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    if !model.incomingNow.isEmpty {
                        VStack(alignment: .leading, spacing: 8) {
                            ForEach(model.incomingNow) { item in
                                VStack(alignment: .leading, spacing: 4) {
                                    Text(item.title)
                                        .font(.system(size: 13, weight: .medium))
                                        .lineLimit(1)
                                    Text(item.subtitle)
                                        .font(.system(size: 12))
                                        .foregroundStyle(.secondary)
                                        .lineLimit(1)
                                        .truncationMode(.middle)
                                    if item.showsProgress {
                                        TransferBar(item: item)
                                    } else {
                                        ProgressView()
                                            .controlSize(.small)
                                    }
                                }
                            }
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    HStack(alignment: .top, spacing: 14) {
                        SendCard(dropping: dropping)
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                        NearbyCard()
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                    }
                    .frame(maxHeight: .infinity)
                    footer
                    }
                    .padding(.horizontal, 22)
                    .frame(maxHeight: .infinity)
                } else if model.screen == .settings {
                    SettingsView()
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    ActivityView()
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                }
            }
            .padding(.top, 36)
            .padding(.bottom, model.screen == .home ? 16 : 0)

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
        .onExitCommand {
            if model.currentOffer != nil {
                model.declineCurrentOffer()
            } else if model.trustPeer != nil {
                model.cancelTrust()
            } else if model.screen != .home {
                model.showHome()
            }
        }
        .onDrop(of: [.fileURL], isTargeted: $dropping) { providers in
            guard model.screen == .home else { return false }
            return model.takeDrop(providers)
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
            HStack(spacing: 2) {
                Button {
                    model.showActivity()
                } label: {
                    Image(systemName: "clock")
                        .font(.system(size: 14, weight: .medium))
                        .frame(width: 28, height: 28)
                        .contentShape(Rectangle())
                        .background {
                            if model.screen == .activity {
                                Circle().fill(Color.primary.opacity(0.08))
                            }
                        }
                }
                .buttonStyle(.plain)
                .help("Activity")
                .accessibilityLabel("Activity")
                .accessibilityIdentifier("ActivityButton")
                Button {
                    model.showSettings()
                } label: {
                    Image(systemName: "gearshape")
                        .font(.system(size: 14, weight: .medium))
                        .frame(width: 28, height: 28)
                        .contentShape(Rectangle())
                        .background {
                            if model.screen == .settings {
                                Circle().fill(Color.primary.opacity(0.08))
                            }
                        }
                }
                .buttonStyle(.plain)
                .help("Settings")
                .accessibilityLabel("Settings")
                .accessibilityIdentifier("SettingsButton")
            }
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
            if model.sending, let outgoing = model.liveOutgoing, outgoing.showsProgress {
                TransferBar(item: outgoing)
            }
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
        if model.sending {
            return model.liveOutgoing?.subtitle ?? "Preparing transfer…"
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
    @State private var showHiddenDevices = false

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
                        ForEach(model.primaryPeers) { peer in
                            PeerRow(peer: peer, selected: peer.id == model.selectedPeerID)
                        }
                        if !model.hiddenPeers.isEmpty {
                            DisclosureGroup(isExpanded: $showHiddenDevices) {
                                ForEach(model.hiddenPeers) { peer in
                                    PeerRow(peer: peer, selected: peer.id == model.selectedPeerID)
                                }
                            } label: {
                                // The arrow is the group's only hit target. A button is
                                // required; a tap gesture is lost to the window drop target.
                                Button {
                                    withAnimation(.easeOut(duration: 0.16)) {
                                        showHiddenDevices.toggle()
                                    }
                                } label: {
                                    Text(model.hiddenDevicesTitle)
                                        .font(.system(size: 12, weight: .semibold))
                                        .foregroundStyle(.secondary)
                                        .frame(maxWidth: .infinity, alignment: .leading)
                                        .contentShape(Rectangle())
                                }
                                .buttonStyle(RowButtonStyle())
                                .onHover { inside in
                                    if inside {
                                        NSCursor.pointingHand.set()
                                    } else {
                                        NSCursor.arrow.set()
                                    }
                                }
                            }
                            .accessibilityIdentifier("HiddenDevices")
                            .padding(.top, 8)
                        }
                    }
                }
            }
        }
        .modifier(CardStyle())
    }
}

private struct PeerRow: View {
    @Environment(AppModel.self) private var model
    @Environment(\.colorScheme) private var scheme
    var peer: Peer
    var selected: Bool
    @State private var hovering = false

    var body: some View {
        // The window is a file drop target. A tap gesture on this row loses
        // that click, so choosing an iPhone never enabled Send. The clipboard
        // control stays outside that button; a nested button does not receive clicks.
        selectButton
            .overlay(alignment: .trailing) { trailingOverlay }
            .onHover { inside in
                hovering = inside
                if inside {
                    NSCursor.pointingHand.set()
                } else {
                    NSCursor.arrow.set()
                }
            }
            .accessibilityElement(children: .contain)
    }

    private var selectButton: some View {
        Button {
            model.select(peer)
        } label: {
            HStack(spacing: 10) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(peer.name)
                        .font(.system(size: 13, weight: .medium))
                        .foregroundStyle(.primary)
                        .lineLimit(1)
                    if peer.showsWaiting {
                        Text(peer.waitingText)
                            .font(.system(size: 11))
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                            .help(peer.waitingText)
                            .accessibilityIdentifier("WaitingForTrust")
                    } else {
                        Text(peer.detail)
                            .font(.system(size: 11, design: .monospaced))
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                }
                Spacer(minLength: 8)
                if peer.showsAdd {
                    Color.clear.frame(width: 54, height: 22)
                }
                if peer.showsClipboard {
                    Color.clear.frame(width: 28, height: 28)
                }
                protocolBadge
                Circle()
                    .fill(selected ? Theme.signal(scheme) : Color.clear)
                    .frame(width: 6, height: 6)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .fill(selected ? Color.primary.opacity(0.07) : (hovering ? Color.primary.opacity(0.04) : Color.clear))
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(RowButtonStyle())
        .accessibilityLabel("\(peer.name), \(protocolLabel(peer.via))")
    }

    @ViewBuilder
    private var trailingOverlay: some View {
        if peer.showsAdd || peer.showsClipboard {
            HStack(spacing: 10) {
                if peer.showsAdd {
                    Button("Add") {
                        model.addNearby(peer)
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .frame(width: 54)
                    .help("Trust this device")
                    .accessibilityLabel(peer.addAccessLabel)
                    .accessibilityIdentifier("AddTrusted")
                }
                if peer.showsClipboard {
                    Button {
                        model.copyFrom(peer)
                    } label: {
                        Image(systemName: "doc.on.clipboard")
                            .font(.system(size: 13, weight: .medium))
                            .foregroundStyle(.secondary)
                            .frame(width: 28, height: 28)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .help(peer.clipboardTip)
                    .accessibilityLabel(peer.clipboardAccessLabel)
                    .accessibilityIdentifier("CopyClipboard")
                }
                protocolBadge.hidden()
                Circle()
                    .fill(Color.clear)
                    .frame(width: 6, height: 6)
            }
            .padding(.trailing, 10)
        }
    }

    private var protocolBadge: some View {
        Text(protocolLabel(peer.via))
            .font(.system(size: 10, weight: .medium))
            .foregroundStyle(.secondary)
            .padding(.horizontal, 7)
            .padding(.vertical, 3)
            .background(Capsule().fill(Color.primary.opacity(0.06)))
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

/// Leaves the row colors alone. The plain style repaints the label in the accent color.
private struct RowButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .opacity(configuration.isPressed ? 0.72 : 1)
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
