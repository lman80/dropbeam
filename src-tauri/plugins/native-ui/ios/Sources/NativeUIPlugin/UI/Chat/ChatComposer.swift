import SwiftUI
import UIKit
import UniformTypeIdentifiers

struct ChatComposer: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.scenePhase) private var scenePhase
    let friendID: String
    @Binding var reply: ChatMessage?
    @Binding var editing: ChatMessage?
    @Binding var text: String
    var didSend: () -> Void
    @State private var sending = false
    @State private var picking = false
    @State private var gifPicker = false
    @State private var typing = false
    @State private var lastBeacon = Date.distantPast
    @State private var idleTask: Task<Void, Never>?
    @FocusState private var focused: Bool
    private var canSend: Bool { !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || (editing == nil && !bridge.chatDraftFiles.isEmpty) }
    private var hasDrafts: Bool { !bridge.chatDraftFiles.isEmpty && editing == nil }
    var body: some View {
        VStack(spacing: 8) {
            if let target = editing ?? reply {
                HStack(spacing: 10) {
                    Image(systemName: editing != nil ? "pencil" : "arrowshape.turn.up.left.fill")
                        .font(.footnote.weight(.semibold)).foregroundStyle(ChatPalette.sent).frame(width: 20)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(editing != nil ? "Editing Message" : "Replying").font(.caption.weight(.semibold)).foregroundStyle(ChatPalette.sent)
                        Text(target.preview).font(.subheadline).lineLimit(1).foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 0)
                    Button { if editing != nil { text = "" }; editing = nil; reply = nil } label: {
                        Image(systemName: "xmark").font(.footnote.weight(.bold)).foregroundStyle(.secondary).frame(width: 36, height: 36)
                    }.buttonStyle(.plain).accessibilityLabel("Cancel \(editing != nil ? "edit" : "reply")")
                }
                .padding(.leading, 14).padding(.trailing, 4).padding(.vertical, 2)
                .glassSurface(RoundedRectangle(cornerRadius: 22, style: .continuous))
                .padding(.leading, 46)
                .transition(.move(edge: .bottom).combined(with: .opacity))
            }
            HStack(alignment: .bottom, spacing: 8) {
                    Menu {
                        Button { pick("photos") } label: { Label("Photos", systemImage: "photo.on.rectangle.angled") }
                        Button { pick("files") } label: { Label("Files", systemImage: "folder") }
                        // PasteButton: tapping it IS the consent, so iOS never shows the
                        // "Allow Paste" prompt (hasImages only peeks at the types — no prompt).
                        if UIPasteboard.general.hasImages {
                            PasteButton(supportedContentTypes: [.image]) { providers in pasteImages(providers) }
                        }
                        if bridge.settings?.giphyApiKey?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty == false {
                            Button { focused = false; gifPicker = true } label: { Label("GIFs", systemImage: "magnifyingglass") }
                        }
                    } label: {
                        Image(systemName: "plus").font(.system(size: 19, weight: .medium)).foregroundStyle(.primary)
                            .frame(width: 38, height: 38).contentShape(Circle())
                            .glassSurface(Circle(), interactive: true)
                    }.tint(.primary).disabled(picking || editing != nil).accessibilityLabel("Add attachment")
                    field
            }
        }
        .padding(.horizontal, 12).padding(.top, 6).padding(.bottom, 8)
        .animation(.spring(response: 0.3, dampingFraction: 0.82), value: editing?.id ?? reply?.id)
        .animation(.spring(response: 0.3, dampingFraction: 0.82), value: bridge.chatDraftFiles)
            .sheet(isPresented: $gifPicker) { ChatGifPicker(friendID: friendID).environmentObject(bridge) }
            .onChange(of: editing?.id) { _, _ in if let editing { text = editing.text ?? ""; focused = true } }
            .onChange(of: reply?.id) { _, _ in if reply != nil { focused = true } }
            #if targetEnvironment(simulator)
            // QA: `-focusComposer` shows then hides the keyboard (reproduces the state
            // that made feedback screenshots black).
            .task {
                guard CommandLine.arguments.contains("-focusComposer") else { return }
                try? await Task.sleep(for: .seconds(1.5)); focused = true
                try? await Task.sleep(for: .seconds(2.5)); focused = false
            }
            #endif
            .onChange(of: scenePhase) { _, phase in if phase != .active { stopTyping() } }
            .onDisappear { stopTyping() }
    }
    /// The capsule text field; staged attachments ride inside it, above the text, like Messages.
    private var field: some View {
        VStack(alignment: .leading, spacing: 0) {
            if hasDrafts {
                ScrollView(.horizontal) {
                    LazyHStack(spacing: 8) {
                        ForEach(bridge.chatDraftFiles, id: \.self) { path in
                            Group {
                                if LocalMedia(path: path) != nil { MediaThumbnail(path: path, width: 88, height: 88) }
                                else {
                                    // Documents: say what it is, not just a generic glyph.
                                    VStack(spacing: 6) {
                                        Image(systemName: Formatters.symbol(path)).font(.title2).foregroundStyle(.secondary)
                                        Text((path as NSString).lastPathComponent).font(.caption2).foregroundStyle(.primary)
                                            .lineLimit(2).truncationMode(.middle).multilineTextAlignment(.center)
                                    }.padding(8).frame(width: 88, height: 88).background(Color(uiColor: .secondarySystemFill))
                                }
                            }
                                .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
                                .overlay(alignment: .topTrailing) {
                                    Button { bridge.perform { try await bridge.removeChatDraftFile(path: path) } } label: {
                                        Image(systemName: "xmark.circle.fill").font(.system(size: 20))
                                            .symbolRenderingMode(.palette).foregroundStyle(.white, .black.opacity(0.55))
                                            .frame(width: 32, height: 32)
                                    }.accessibilityLabel("Remove \(URL(fileURLWithPath: path).lastPathComponent)")
                                }
                        }
                    }.padding(.horizontal, 8)
                }.frame(height: 88).scrollIndicators(.hidden).padding(.top, 8)
            }
            HStack(alignment: .bottom, spacing: 4) {
                TextField(editing != nil ? "Edit message" : "Message", text: $text, axis: .vertical)
                    .font(.body).lineLimit(1...6).focused($focused)
                    .padding(.leading, 14).padding(.vertical, 8).frame(minHeight: 38)
                    .onChange(of: text) { _, _ in onType() }
                if canSend {
                    Button(action: send) {
                        Image(systemName: editing != nil ? "checkmark" : "arrow.up").font(.system(size: 15, weight: .bold)).foregroundStyle(.white)
                            .frame(width: 30, height: 30).background(ChatPalette.sent, in: Circle())
                    }.buttonStyle(.plain).padding(4).disabled(sending)
                        .transition(.scale(scale: 0.4).combined(with: .opacity))
                        .accessibilityLabel(editing != nil ? "Save edit" : "Send message")
                } else { Color.clear.frame(width: 10, height: 38) }
            }
        }
        .animation(.spring(response: 0.28, dampingFraction: 0.7), value: canSend)
        .glassSurface(RoundedRectangle(cornerRadius: 19, style: .continuous), interactive: true)
        .contentShape(RoundedRectangle(cornerRadius: 19)).onTapGesture { focused = true }
    }
    private func pick(_ source: String) {
        focused = false; picking = true; reply = nil
        bridge.perform {
            let paths: [String]
            do { paths = try await bridge.pickFiles(source: source, purpose: .chat) }
            catch { picking = false; throw error }
            // Unlock + immediately on the picker reply, before any staging reply.
            picking = false
            guard !paths.isEmpty, bridge.chatPath.last == friendID else { return }
            try await bridge.action("stageChatFiles", ["friendId": friendID, "paths": paths])
        }
    }
    /// Stage the clipboard's image(s) like picked photos (screenshots are the #1 paste).
    private func pasteImages(_ providers: [NSItemProvider]) {
        Task { @MainActor in
            focused = false; reply = nil
            bridge.perform {
                let paths = try await PastedImages.save(providers)
                guard !paths.isEmpty, bridge.chatPath.last == friendID else { return }
                try await bridge.action("stageChatFiles", ["friendId": friendID, "paths": paths])
            }
        }
    }
    private func send() {
        guard canSend && !sending else { return }
        let body = text, target = editing, quote = reply
        sending = true; stopTyping()
        bridge.perform {
            defer { sending = false }
            if let target { try await bridge.editMessage(friendId: friendID, messageId: target.id, text: body) }
            else { try await bridge.sendChatText(friendId: friendID, text: body, replyTo: quote?.id) }
            if text == body { text = "" }
            if reply?.id == quote?.id { reply = nil }
            if editing?.id == target?.id { editing = nil }
            stopTyping(); didSend()
        }
    }
    private func onType() {
        idleTask?.cancel()
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, focused else { stopTyping(); return }
        if !typing || Date().timeIntervalSince(lastBeacon) >= 3 {
            typing = true; lastBeacon = Date()
            Task { try? await bridge.setTyping(friendId: friendID, on: true) }
        }
        idleTask = Task {
            do { try await Task.sleep(for: .seconds(3)) } catch { return }
            stopTyping()
        }
    }
    private func stopTyping() {
        idleTask?.cancel(); idleTask = nil
        if typing { typing = false; Task { try? await bridge.setTyping(friendId: friendID, on: false) } }
    }
}

private struct ChatGifPicker: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.dismiss) private var dismiss
    let friendID: String
    @State private var query = ""
    @State private var results: [GifResult] = []
    @State private var loading = false
    @State private var error: String?
    var body: some View {
        NavigationStack {
            ScrollView {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 130))], spacing: 12) {
                    ForEach(results) { gif in
                        Button {
                            bridge.perform { try await bridge.sendChatGif(friendId: friendID, id: gif.id); dismiss() }
                        } label: {
                            MediaThumbnail(path: gif.thumbUrl ?? "", width: 140, height: 110, badges: false)
                                .frame(minHeight: 100).clipShape(RoundedRectangle(cornerRadius: 14))
                        }.accessibilityLabel(gif.title ?? "Send GIF")
                    }
                }.padding(16)
                if loading { ProgressView().padding() }
                if let error { Text(error).foregroundStyle(.secondary).padding() }
                if !loading && error == nil && results.isEmpty { Text("No GIFs found").foregroundStyle(.secondary).padding() }
            }.navigationTitle("GIFs").navigationBarTitleDisplayMode(.inline)
                .searchable(text: $query, prompt: "Search GIFs")
                .safeAreaInset(edge: .bottom) { Text("Powered by GIPHY").font(.caption).padding().frame(maxWidth: .infinity).background(.regularMaterial) }
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Done") { dismiss() } } }
                .task(id: query) {
                    loading = true; error = nil
                    do {
                        try await Task.sleep(for: .milliseconds(280))
                        let fetched = try await bridge.chatGifs(query: query)
                        try Task.checkCancellation()
                        results = fetched; loading = false
                    } catch is CancellationError {} catch { if !Task.isCancelled { self.error = error.localizedDescription; loading = false } }
                }
        }.tint(.beam)
    }
}

/// Pasted images are written as files so they ride the normal staged-attachment path.
/// They live in a PickedMedia session (not Caches): a paste sent to a friend who is
/// offline must still exist when the send finally goes out.
enum PastedImages {
    static func save(_ providers: [NSItemProvider]) async throws -> [String] {
        guard !providers.isEmpty else { return [] }
        let dir = try PickedMedia.session(.chat)
        let stamp = Date().formatted(.iso8601.year().month().day().time(includingFractionalSeconds: false).timeSeparator(.omitted).dateSeparator(.dash)).replacingOccurrences(of: ":", with: "")
        var paths: [String] = []
        for (index, provider) in providers.enumerated() {
            let type = provider.registeredTypeIdentifiers.first { UTType($0)?.conforms(to: .image) == true } ?? UTType.png.identifier
            let data: Data? = await withCheckedContinuation { continuation in
                _ = provider.loadDataRepresentation(forTypeIdentifier: type) { data, _ in continuation.resume(returning: data) }
            }
            guard let data, !data.isEmpty else { continue }
            let ext = UTType(type)?.preferredFilenameExtension ?? "png"
            let url = PickedMedia.unique("Pasted \(stamp)\(providers.count > 1 ? "-\(index + 1)" : "").\(ext)", in: dir)
            try data.write(to: url, options: .atomic)
            paths.append(url.path)
        }
        if paths.isEmpty { try? FileManager.default.removeItem(at: dir) }
        return paths
    }
}
