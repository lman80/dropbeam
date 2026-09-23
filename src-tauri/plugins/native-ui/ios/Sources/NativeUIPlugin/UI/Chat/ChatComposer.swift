import SwiftUI

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
                            MediaThumbnail(path: path, width: 88, height: 88)
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
            do { paths = try await bridge.pickFiles(source: source) }
            catch { picking = false; throw error }
            // Unlock + immediately on the picker reply, before any staging reply.
            picking = false
            guard !paths.isEmpty, bridge.chatPath.last == friendID else { return }
            try await bridge.action("stageChatFiles", ["friendId": friendID, "paths": paths])
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
