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
                    Text(file.bytes == 0 ? "Size unknown" : humanSize(file.bytes))
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
            Color.black.opacity(0.28)
                .ignoresSafeArea()
                .onTapGesture { model.cancelTrust() }
            if let peer = model.trustPeer {
                VStack(alignment: .leading, spacing: 14) {
                    Text("Trust \(peer.name)?")
                        .font(.system(size: 18, weight: .semibold))
                        .lineLimit(2)
                    if let fingerprint = peer.fingerprint, !fingerprint.isEmpty {
                        Text("Compare this fingerprint with the one shown on \(peer.name). Later sends to this address check it again.")
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
                        Button("Trust and send", action: model.confirmTrust)
                            .buttonStyle(.borderedProminent)
                            .keyboardShortcut(.defaultAction)
                    }
                }
                .padding(22)
                .frame(width: 440)
                .background(Theme.card, in: RoundedRectangle(cornerRadius: 20, style: .continuous))
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

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @State private var draftName = ""
    @State private var nameDirty = false
    @FocusState private var nameFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack(spacing: 10) {
                WhooshMark()
                    .frame(width: 28, height: 28)
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

            if !model.visibility.isEmpty {
                Text(model.visibility)
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(24)
        .frame(minWidth: 460, maxWidth: .infinity, alignment: .topLeading)
        .background(Theme.canvas)
        .onAppear {
            draftName = model.deviceName
            nameDirty = false
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
