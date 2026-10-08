import SwiftUI
import UniformTypeIdentifiers

// Same look as the app's Send To sheet (native-ui NativeSheets.swift / Design.swift):
// an inset-grouped List, Settings-style rows, the brand tint. Pieces of Design.swift
// the extension needs are mirrored at the bottom of this file.

struct ShareRootView: View {
    @ObservedObject var model: ShareModel

    private var mine: [ShareGroup.Recipient] { model.recipients.filter(\.own) }
    private var friends: [ShareGroup.Recipient] { model.recipients.filter { !$0.own } }
    private var busy: Bool { if case .sending = model.phase { return true }; return false }

    var body: some View {
        NavigationStack {
            List {
                Section { ShareSummary(model: model) }
                switch model.phase {
                case .failed(let message):
                    Section { Label(message, systemImage: "exclamationmark.triangle.fill").foregroundStyle(.red) }
                case .saved(let who):
                    Section {
                        Label {
                            VStack(alignment: .leading, spacing: 4) {
                                Text("Saved for DropBeam").font(.headline)
                                Text(who.isEmpty ? "Open DropBeam to finish sending." : "Open DropBeam to finish sending to \(who).")
                                    .font(.subheadline).foregroundStyle(.secondary)
                            }
                        } icon: { Image(systemName: "checkmark.circle.fill").foregroundStyle(.green) }
                    }
                default:
                    recipientSections
                }
            }
            .listStyle(.insetGrouped)
            .disabled(busy)
            .navigationTitle("DropBeam").navigationBarTitleDisplayMode(.inline)
            .toolbar {
                if case .saved = model.phase {
                    ToolbarItem(placement: .confirmationAction) { Button("Done") { model.done() } }
                } else {
                    ToolbarItem(placement: .cancellationAction) { Button("Cancel") { model.cancel() } }
                }
            }
        }
        .tint(.beam)
    }

    @ViewBuilder private var recipientSections: some View {
        if !mine.isEmpty {
            Section("My Devices") { ForEach(mine) { row($0) } }
        }
        Section("Friends") {
            if friends.isEmpty {
                Text(model.hasSnapshot ? "No friends yet — Quick Send works for anyone." : "Open DropBeam once and your friends will show up here.")
                    .foregroundStyle(.secondary)
            }
            ForEach(friends) { row($0) }
        }
        Section {
            Button { model.quickSend() } label: {
                HStack(spacing: 14) {
                    ShareRowIcon(symbol: "qrcode")
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Quick Send with a Code").foregroundStyle(.primary)
                        Text("Anyone with DropBeam can receive").font(.subheadline).foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 8)
                    if model.phase == .sending("quick") { ProgressView() }
                }.contentShape(Rectangle())
            }.buttonStyle(.plain)
            if !model.hasSnapshot {
                Button { model.chooseInApp() } label: {
                    HStack(spacing: 14) {
                        ShareRowIcon(symbol: "paperplane.fill")
                        Text("Choose in DropBeam").foregroundStyle(.primary)
                        Spacer(minLength: 8)
                        if model.phase == .sending("choose") { ProgressView() }
                    }.contentShape(Rectangle())
                }.buttonStyle(.plain)
            }
        } footer: {
            Text("DropBeam opens to send. Files go straight to the other device — never through a server.")
        }
    }

    private func row(_ recipient: ShareGroup.Recipient) -> some View {
        Button { model.send(to: recipient) } label: {
            HStack(spacing: 14) {
                ShareAvatar(recipient: recipient, image: model.avatar(for: recipient), size: 42)
                VStack(alignment: .leading, spacing: 2) {
                    Text(recipient.name).font(.body.weight(.semibold)).foregroundStyle(.primary).lineLimit(2)
                    if model.presenceFresh { SharePresence(online: recipient.online) }
                }
                Spacer(minLength: 8)
                if model.phase == .sending(recipient.id) { ProgressView() }
                else { Image(systemName: "paperplane").foregroundStyle(.tint) }
            }.contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Send to \(recipient.name)")
    }
}

/// What's being shared: thumbnails, a title and the total size.
private struct ShareSummary: View {
    @ObservedObject var model: ShareModel
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 14) {
                if model.items.count <= 1 { lead }
                VStack(alignment: .leading, spacing: 2) {
                    Text(title).font(.headline).lineLimit(2)
                    Text(subtitle).font(.subheadline).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer(minLength: 0)
            }
            if model.items.count > 1 {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) { ForEach(model.items) { ShareThumb(item: $0, size: 56) } }
                }
            }
        }
        .accessibilityElement(children: .combine)
    }
    @ViewBuilder private var lead: some View {
        if model.items.count == 1, let item = model.items.first { ShareThumb(item: item, size: 56) }
        else if model.items.isEmpty && model.pending > 0 {
            ProgressView().frame(width: 56, height: 56)
                .background(Color.beam.opacity(0.13), in: RoundedRectangle(cornerRadius: 15, style: .continuous))
        } else {
            ShareGlyph(symbol: model.items.count > 1 ? "square.stack" : "doc", size: 56)
        }
    }
    private var title: String {
        let items = model.items
        guard !items.isEmpty else { return model.total == 1 ? "Preparing item…" : "Preparing \(model.total) items…" }
        if items.count == 1, let item = items.first {
            if item.isLink, let url = URL(string: item.name) { return url.host ?? item.name }
            if case .text = item.kind { return item.name.trimmingCharacters(in: .whitespacesAndNewlines) }
            return item.name
        }
        let photos = items.allSatisfy { $0.type?.conforms(to: .image) == true }
        let videos = items.allSatisfy { $0.type?.conforms(to: .movie) == true }
        return photos ? "\(items.count) Photos" : videos ? "\(items.count) Videos" : "\(items.count) Items"
    }
    private var subtitle: String {
        if model.pending > 0 && !model.items.isEmpty { return "Preparing \(model.total - model.pending + 1) of \(model.total)…" }
        if model.pending > 0 { return "Items in iCloud may take a moment to download." }
        var parts: [String] = []
        if model.totalBytes > 0 { parts.append(ByteCountFormatter.string(fromByteCount: model.totalBytes, countStyle: .file)) }
        else if model.items.count == 1, model.items[0].isLink { parts.append("Link") }
        else if model.items.count == 1, case .text = model.items[0].kind { parts.append("Text") }
        if model.failures > 0 { parts.append("\(model.failures) couldn’t be read") }
        return parts.joined(separator: " · ")
    }
}

private struct ShareThumb: View {
    let item: ShareModel.Item
    let size: CGFloat
    var body: some View {
        if let image = item.thumbnail {
            Image(uiImage: image).resizable().scaledToFill().frame(width: size, height: size)
                .clipShape(RoundedRectangle(cornerRadius: size * 0.22, style: .continuous))
                .overlay(alignment: .bottomTrailing) {
                    if item.type?.conforms(to: .movie) == true {
                        Image(systemName: "video.fill").font(.system(size: 10, weight: .semibold)).foregroundStyle(.white)
                            .padding(4).shadow(radius: 2)
                    }
                }
        } else {
            ShareGlyph(symbol: symbol, size: size)
        }
    }
    private var symbol: String {
        if item.isLink { return "link" }
        if case .text = item.kind { return "text.alignleft" }
        guard let type = item.type else { return "doc" }
        if type.conforms(to: .image) { return "photo" }
        if type.conforms(to: .movie) { return "film" }
        if type.conforms(to: .audio) { return "waveform" }
        if type.conforms(to: .pdf) { return "doc.richtext" }
        if type.conforms(to: .archive) { return "doc.zipper" }
        return "doc"
    }
}

// MARK: - Mirrored from native-ui Design.swift

extension Color {
    static let beam = Color(uiColor: UIColor { traits in
        traits.userInterfaceStyle == .dark
            ? UIColor(red: 108/255, green: 106/255, blue: 250/255, alpha: 1)
            : UIColor(red: 91/255, green: 91/255, blue: 240/255, alpha: 1)
    })
}

private struct ShareGlyph: View {
    let symbol: String
    var size: CGFloat = 44
    var body: some View {
        Image(systemName: symbol).font(.system(size: size * 0.4)).foregroundStyle(Color.beam)
            .frame(width: size, height: size)
            .background(Color.beam.opacity(0.13), in: RoundedRectangle(cornerRadius: size * 0.27, style: .continuous))
            .accessibilityHidden(true)
    }
}

private struct ShareRowIcon: View {
    let symbol: String
    @ScaledMetric(relativeTo: .body) private var size: CGFloat = 30
    var body: some View {
        Image(systemName: symbol).font(.system(size: size * 0.5, weight: .semibold)).foregroundStyle(.white)
            .frame(width: size, height: size)
            .background(Color.beam.gradient, in: RoundedRectangle(cornerRadius: size * 0.27, style: .continuous))
            .accessibilityHidden(true)
    }
}

private struct SharePresence: View {
    let online: Bool
    var body: some View {
        HStack(spacing: 6) {
            Circle().fill(online ? Color.green : Color(uiColor: .tertiaryLabel)).frame(width: 7, height: 7)
            Text(online ? "Online" : "Offline").font(.subheadline).foregroundStyle(.secondary)
        }
    }
}

/// A friend's picture or monogram; an own device shows its device glyph.
private struct ShareAvatar: View {
    let recipient: ShareGroup.Recipient
    let image: UIImage?
    var size: CGFloat = 42
    var body: some View {
        Group {
            if recipient.own {
                ZStack {
                    Circle().fill(LinearGradient(colors: [Color(uiColor: .systemGray5), Color(uiColor: .systemGray4)], startPoint: .top, endPoint: .bottom))
                    Image(systemName: deviceSymbol).font(.system(size: size * 0.42)).foregroundStyle(.primary.opacity(0.8))
                }
            } else if let image {
                Image(uiImage: image).resizable().scaledToFill()
            } else {
                ZStack {
                    Self.color(for: recipient.id)
                    Text(Self.initials(recipient.name)).font(.system(size: size * 0.38, weight: .semibold, design: .rounded))
                        .foregroundStyle(.white).minimumScaleFactor(0.5).lineLimit(1).padding(.horizontal, size * 0.08)
                }
            }
        }
        .frame(width: size, height: size).clipShape(Circle()).accessibilityHidden(true)
    }
    private var deviceSymbol: String {
        switch (recipient.deviceOs, recipient.deviceKind) {
        case ("ios", "tablet"): return "ipad"
        case ("ios", _): return "iphone"
        case ("macos", "desktop"): return "desktopcomputer"
        case ("macos", _): return "laptopcomputer"
        case ("windows", _): return "pc"
        case (_, "phone"), (_, "iphone"): return "iphone"
        case (_, "laptop"): return "laptopcomputer"
        default: return "desktopcomputer"
        }
    }
    private static let palette: [UInt32] = [0x6e6ee8, 0x3a9ad9, 0xe0764a, 0x3aa57a, 0xc9609a, 0x9a6fd6, 0xd69a2e, 0x5d8a9e]
    /// Same hash as the app and desktop, so a friend keeps their colour everywhere.
    static func color(for seed: String) -> Color {
        var h: UInt32 = 0
        for unit in seed.utf16 { h = h &* 31 &+ UInt32(unit) }
        let v = palette[Int(h % UInt32(palette.count))]
        return Color(red: Double((v >> 16) & 0xff) / 255, green: Double((v >> 8) & 0xff) / 255, blue: Double(v & 0xff) / 255)
    }
    static func initials(_ name: String) -> String {
        let parts = name.split(whereSeparator: \.isWhitespace)
        guard let first = parts.first else { return "?" }
        if parts.count == 1 { return String(first.prefix(2)).uppercased() }
        return (String(first.prefix(1)) + String(parts[parts.count - 1].prefix(1))).uppercased()
    }
}
