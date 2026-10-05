import AppKit
import SwiftUI
import WhooshUI

struct OfferOverlay: View {
    @Environment(AppModel.self) private var model
    @Environment(\.colorScheme) private var scheme
    var offer: Offer

    var body: some View {
        ZStack {
            Color.black.opacity(0.28)
                .ignoresSafeArea()
                .onTapGesture { model.declineCurrentOffer() }
            VStack(alignment: .leading, spacing: 16) {
                VStack(alignment: .leading, spacing: 4) {
                    Text(offer.peer)
                        .font(.system(size: 18, weight: .semibold))
                        .lineLimit(1)
                    Text("\(protocolLabel(offer.via)) · \(summary)")
                        .font(.system(size: 12))
                        .foregroundStyle(.secondary)
                }
                if offer.via == "quickshare", let pin = offer.pin, !pin.isEmpty {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Compare this code")
                            .font(.system(size: 11, weight: .semibold))
                            .tracking(0.8)
                            .foregroundStyle(.secondary)
                            .textCase(.uppercase)
                        Text(pin)
                            .font(.system(size: 34, weight: .semibold, design: .monospaced))
                            .tracking(4)
                    }
                } else if offer.via == "whoosh", offer.pin != nil {
                    Text("Protected by your pin")
                        .font(.system(size: 13))
                        .foregroundStyle(.secondary)
                }
                fileList
                    .frame(maxHeight: 180)
                HStack {
                    Spacer()
                    Button("Decline", action: model.declineCurrentOffer)
                        .buttonStyle(.bordered)
                        .keyboardShortcut(.cancelAction)
                    Button("Accept", action: model.acceptCurrentOffer)
                        .buttonStyle(.borderedProminent)
                        .tint(Theme.signal(scheme))
                        .keyboardShortcut(.defaultAction)
                }
            }
            .padding(22)
            .frame(width: 420)
            .background(Theme.card, in: RoundedRectangle(cornerRadius: 20, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 20, style: .continuous)
                    .strokeBorder(Theme.hairline, lineWidth: 1)
            )
            .shadow(color: .black.opacity(0.18), radius: 30, y: 16)
        }
    }

    private var fileList: some View {
        VStack(spacing: 0) {
            ForEach(Array(offer.files.enumerated()), id: \.offset) { _, file in
                HStack(spacing: 8) {
                    Image(systemName: symbolName(for: file.name, kind: file.kind))
                        .frame(width: 16)
                        .foregroundStyle(.secondary)
                    Text(file.name)
                        .lineLimit(1)
                    Spacer(minLength: 8)
                    Text(file.isSizeKnown ? humanSize(file.bytes) : "Size unknown")
                        .foregroundStyle(.secondary)
                        .font(.system(size: 12))
                }
                .font(.system(size: 13))
                .padding(.vertical, 5)
            }
        }
    }

    private var summary: String {
        let count = offer.files.count
        let noun = count == 1 ? "file" : "files"
        if offer.sizeUnknown {
            return "\(count) \(noun)"
        }
        return "\(count) \(noun) · \(humanSize(offer.totalBytes))"
    }
}

struct TrustOverlay: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ZStack {
            Color.black.opacity(0.62)
                .ignoresSafeArea()
                .onTapGesture { model.cancelTrust() }
            if let peer = model.trustPeer {
                VStack(alignment: .leading, spacing: 14) {
                    Text("Trust \(peer.name)?")
                        .font(.system(size: 18, weight: .semibold))
                        .lineLimit(2)
                    if let fingerprint = peer.fingerprint, !fingerprint.isEmpty {
                        Text(model.trustForClipboard
                             ? "Compare this fingerprint with Settings on \(peer.name). Clipboard sharing is encrypted and only works with devices you both trust. \(peer.name) has to trust this computer too."
                             : "Compare this fingerprint with the one shown on \(peer.name). Later sends to this address check it again. Trusted Whoosh devices can also copy this computer's clipboard.")
                            .font(.system(size: 13))
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                        Text(groupedFingerprint(fingerprint))
                            .font(.system(size: 12, design: .monospaced))
                            .foregroundStyle(.primary)
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(12)
                            .background(
                                RoundedRectangle(cornerRadius: 12, style: .continuous)
                                    .fill(Color.primary.opacity(0.04))
                            )
                    } else {
                        Text("This device did not share a fingerprint. Trusting it remembers \(peer.address) for later sends.")
                            .font(.system(size: 13))
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    HStack {
                        Spacer()
                        Button("Cancel", action: model.cancelTrust)
                            .buttonStyle(.bordered)
                            .keyboardShortcut(.cancelAction)
                        Button(model.trustForClipboard ? "Trust and copy" : "Trust and send", action: model.confirmTrust)
                            .buttonStyle(.borderedProminent)
                            .keyboardShortcut(.defaultAction)
                    }
                }
                .padding(22)
                .frame(width: 440)
                .background(Theme.canvas, in: RoundedRectangle(cornerRadius: 20, style: .continuous))
                .overlay(
                    RoundedRectangle(cornerRadius: 20, style: .continuous)
                        .strokeBorder(Theme.hairline, lineWidth: 1)
                )
                .shadow(color: .black.opacity(0.18), radius: 30, y: 16)
            }
        }
    }
}

struct EngineDownOverlay: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ZStack {
            Theme.canvas.opacity(0.94)
            VStack(spacing: 12) {
                WhooshMark()
                    .frame(width: 52, height: 52)
                Text("Whoosh stopped")
                    .font(.system(size: 18, weight: .semibold))
                Text(model.engineError)
                    .font(.system(size: 13))
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 380)
                PrimaryButton(title: "Try Again", action: model.retry)
                    .frame(width: 140)
                    .padding(.top, 4)
            }
        }
    }
}

struct ActivityView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack(spacing: 8) {
                Button {
                    model.showHome()
                } label: {
                    Image(systemName: "chevron.left")
                        .font(.system(size: 13, weight: .semibold))
                        .frame(width: 28, height: 28)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help("Back")
                .accessibilityLabel("Back")
                .accessibilityIdentifier("ActivityBack")
                Text("Activity")
                    .font(.system(size: 18, weight: .semibold))
                Spacer()
                Button("Clear History", action: model.clearActivity)
                    .buttonStyle(.bordered)
                    .disabled(!model.canClearActivity)
            }

            if model.activity.isEmpty {
                Text("Transfers show up here.")
                    .font(.system(size: 13))
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            } else {
                ScrollView {
                    VStack(spacing: 0) {
                        ForEach(model.activity) { item in
                            ActivityRow(item: item)
                            if item.id != model.activity.last?.id {
                                Divider().opacity(0.4)
                            }
                        }
                    }
                }
            }
        }
        .padding(24)
        .frame(minWidth: 460, maxWidth: .infinity, minHeight: 420, maxHeight: .infinity, alignment: .topLeading)
        .background(Theme.canvas)
    }
}

private struct ActivityRow: View {
    @Environment(AppModel.self) private var model
    @Environment(\.colorScheme) private var scheme
    var item: ActivityItem

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 10) {
                stateMark
                    .frame(width: item.showsDeterminate ? 36 : 16)
                VStack(alignment: .leading, spacing: 1) {
                    Text(item.title)
                        .font(.system(size: 13, weight: .medium))
                        .lineLimit(1)
                    Text(item.subtitle)
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
            if item.showsProgress {
                TransferBar(item: item)
            }
        }
        .padding(.vertical, 8)
    }

    @ViewBuilder
    private var stateMark: some View {
        if item.showsDeterminate {
            Text(item.progressLabel)
                .font(.system(size: 10, weight: .semibold, design: .rounded))
                .foregroundStyle(Theme.signal(scheme))
                .lineLimit(1)
        } else if item.showsIndeterminate {
            Color.clear.frame(width: 16, height: 16)
        } else if item.state == "working" {
            ProgressView().controlSize(.small)
        } else if item.state == "done" {
            Image(systemName: "checkmark")
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(Theme.signal(scheme))
        } else if item.state == "failed" {
            Image(systemName: "xmark")
                .font(.system(size: 10, weight: .bold))
                .foregroundStyle(Color(red: 0.70, green: 0.28, blue: 0.24))
        } else {
            Image(systemName: "minus")
                .font(.system(size: 10, weight: .bold))
                .foregroundStyle(.secondary)
        }
    }
}

struct TransferBar: View {
    @Environment(\.colorScheme) private var scheme
    var item: ActivityItem

    var body: some View {
        Group {
            if item.showsDeterminate {
                ProgressView(value: item.percent)
            } else {
                ProgressView()
                    .progressViewStyle(.linear)
            }
        }
        .tint(Theme.signal(scheme))
    }
}

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @State private var draftName = ""
    @State private var nameDirty = false
    @FocusState private var nameFocused: Bool

    var body: some View {
        ScrollView {
        VStack(alignment: .leading, spacing: 18) {
            HStack(spacing: 8) {
                Button {
                    model.showHome()
                } label: {
                    Image(systemName: "chevron.left")
                        .font(.system(size: 13, weight: .semibold))
                        .frame(width: 28, height: 28)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help("Back")
                .accessibilityLabel("Back")
                .accessibilityIdentifier("SettingsBack")
                Text("Settings")
                    .font(.system(size: 18, weight: .semibold))
                Spacer()
                if !model.version.isEmpty {
                    Text(model.version)
                        .font(.system(size: 12))
                        .foregroundStyle(.tertiary)
                }
            }

            VStack(alignment: .leading, spacing: 8) {
                Text("Name")
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
                TextField("This Mac", text: $draftName)
                    .textFieldStyle(.plain)
                    .font(.system(size: 14))
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(
                        RoundedRectangle(cornerRadius: 10, style: .continuous)
                            .fill(Color.primary.opacity(0.04))
                    )
                    .focused($nameFocused)
                    .onSubmit { commitName() }
                    .onChange(of: nameFocused) { _, focused in
                        if !focused { commitName() }
                    }
                    .onChange(of: draftName) { _, value in
                        nameDirty = value != model.deviceName
                    }
            }

            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Save to")
                        .font(.system(size: 12))
                        .foregroundStyle(.secondary)
                    Text(model.saveDir.isEmpty ? "Choose a folder" : abbreviatePath(model.saveDir))
                        .font(.system(size: 13))
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                Spacer()
                Button("Choose…", action: model.chooseSaveFolder)
                    .buttonStyle(.bordered)
            }

            VStack(alignment: .leading, spacing: 0) {
                settingToggle(
                    "Whoosh",
                    "Other computers running Whoosh",
                    Binding(get: { model.native }, set: { model.native = $0; storeProtocols() })
                )
                Divider().opacity(0.4)
                settingToggle(
                    "Quick Share",
                    "Android phones on this Wi-Fi",
                    Binding(get: { model.quickshare }, set: { model.quickshare = $0; storeProtocols() })
                )
                Divider().opacity(0.4)
                settingToggle(
                    "AirDrop",
                    "Apple devices nearby",
                    Binding(get: { model.airdrop }, set: { model.airdrop = $0; storeProtocols() })
                )
                Divider().opacity(0.4)
                settingToggle(
                    "Sort photos and videos",
                    "Keep them in their own folders",
                    Binding(get: { model.sortMedia }, set: { model.sortMedia = $0; storeFlag(Pref.sortMedia, $0) })
                )
                Divider().opacity(0.4)
                settingToggle(
                    "Require a pin",
                    "Whoosh senders enter the code in the window",
                    Binding(get: { model.requirePin }, set: { model.requirePin = $0; storeFlag(Pref.requirePin, $0) })
                )
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 12)
            .background(
                RoundedRectangle(cornerRadius: 14, style: .continuous)
                    .fill(Color.primary.opacity(0.03))
            )

            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    Text("Fingerprint")
                        .font(.system(size: 12))
                        .foregroundStyle(.secondary)
                    Spacer()
                    Button("Copy", action: model.copyFingerprint)
                        .buttonStyle(.plain)
                        .font(.system(size: 12, weight: .medium))
                        .disabled(model.fingerprint.isEmpty)
                }
                Text(model.fingerprint.isEmpty ? "Waiting for the engine." : groupedFingerprint(model.fingerprint))
                    .font(.system(size: 12, design: .monospaced))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }

            VStack(alignment: .leading, spacing: 8) {
                Text("Trusted devices")
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
                Text("Whoosh computers this one trusts. They can send files here and copy this computer's clipboard. Add one from the nearby list.")
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                VStack(alignment: .leading, spacing: 8) {
                    if model.trustedDevices.isEmpty {
                        Text("No trusted devices.")
                            .font(.system(size: 13))
                            .foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 8)
                            .padding(.top, 8)
                            .accessibilityIdentifier("TrustedEmpty")
                    }
                    ForEach(model.trustedDevices) { device in
                        HStack(alignment: .center, spacing: 8) {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(model.trustedTitle(for: device))
                                    .font(.system(size: 13, weight: .medium))
                                    .lineLimit(1)
                                if device.isSelf {
                                    Text("This computer's own fingerprint.")
                                        .font(.system(size: 11))
                                        .foregroundStyle(.secondary)
                                }
                                Text(groupedFingerprint(device.fingerprint))
                                    .font(.system(size: 11, design: .monospaced))
                                    .foregroundStyle(.secondary)
                                    .textSelection(.enabled)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                            }
                            Button("Remove") {
                                model.removeTrusted(device.fingerprint)
                            }
                            .buttonStyle(.bordered)
                            .controlSize(.small)
                            .accessibilityIdentifier("RemoveTrusted")
                            .accessibilityLabel("Remove \(model.trustedTitle(for: device))")
                        }
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                    }
                }
                .padding(8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(
                    RoundedRectangle(cornerRadius: 14, style: .continuous)
                        .fill(Color.primary.opacity(0.03))
                )
            }

            if !model.visibility.isEmpty {
                Text(model.visibility)
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(24)
        .frame(maxWidth: .infinity, alignment: .topLeading)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(Theme.canvas)
        .onAppear {
            draftName = model.deviceName
            nameDirty = false
            model.refreshTrusted()
        }
        .onChange(of: model.deviceName) { _, name in
            if !nameDirty && !nameFocused {
                draftName = name
            }
        }
        .onDisappear { commitName() }
    }

    private func settingToggle(_ title: String, _ detail: String, _ binding: Binding<Bool>) -> some View {
        HStack(alignment: .center, spacing: 16) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.system(size: 13, weight: .medium))
                    .frame(maxWidth: .infinity, alignment: .leading)
                Text(detail)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .multilineTextAlignment(.leading)

            Toggle(title, isOn: binding)
                .toggleStyle(.switch)
                .controlSize(.small)
                .labelsHidden()
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.vertical, 8)
    }

    private func commitName() {
        let value = draftName
        nameDirty = false
        model.applyName(value)
    }

    private func storeProtocols() {
        let defaults = UserDefaults.standard
        defaults.set(model.native, forKey: Pref.native)
        defaults.set(model.quickshare, forKey: Pref.quickshare)
        defaults.set(model.airdrop, forKey: Pref.airdrop)
        model.pushConfig()
    }

    private func storeFlag(_ key: String, _ value: Bool) {
        UserDefaults.standard.set(value, forKey: key)
        model.pushConfig()
    }
}
