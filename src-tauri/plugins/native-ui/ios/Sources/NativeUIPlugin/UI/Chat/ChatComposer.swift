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
    var body: some View {
        VStack(spacing: 8) {
            if let target = editing ?? reply {
                HStack(spacing: 10) {
                    RoundedRectangle(cornerRadius: 2).fill(Color.beam).frame(width: 3)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(editing != nil ? "Edit Message" : "Reply").font(.caption.weight(.semibold)).foregroundStyle(.tint)
                        Text(target.preview).font(.subheadline).lineLimit(2).foregroundStyle(.secondary)
                    }
                    Spacer()
                    Button { if editing != nil { text = "" }; editing = nil; reply = nil } label: {
                        Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary).frame(width: 44, height: 44)
                    }.accessibilityLabel("Cancel \(editing != nil ? "edit" : "reply")")
                }.padding(.horizontal, 12).padding(.top, 8)
            }
            if !bridge.chatDraftFiles.isEmpty && editing == nil {
                ScrollView(.horizontal) {
                    HStack(spacing: 8) {
                        ForEach(bridge.chatDraftFiles, id: \.self) { path in
                            HStack(spacing: 6) {
                                Image(systemName: Formatters.symbol(path))
                                Text(URL(fileURLWithPath: path).lastPathComponent).font(.caption).lineLimit(1).frame(maxWidth: 150)
                                Button { bridge.perform { try await bridge.removeChatDraftFile(path: path) } } label: {
                                    Image(systemName: "xmark.circle.fill").frame(width: 44, height: 44)
                                }.accessibilityLabel("Remove \(URL(fileURLWithPath: path).lastPathComponent)")
                            }.padding(.leading, 12).background(.quaternary, in: Capsule())
                        }
                    }.padding(.horizontal, 12)
                }.scrollIndicators(.hidden)
            }
            HStack(alignment: .bottom, spacing: 4) {
                Menu {
                    Button { pick("photos") } label: { Label("Photos and Videos", systemImage: "photo.on.rectangle") }
                    Button { pick("files") } label: { Label("Files", systemImage: "folder") }
                    if bridge.settings?.giphyApiKey?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty == false {
                        Button { focused = false; gifPicker = true } label: { Label("GIF", systemImage: "face.smiling") }
                    }
                } label: {
                    Image(systemName: "plus").font(.title3.weight(.medium)).frame(width: 34, height: 34)
                        .chatGlass(radius: 17).frame(width: 44, height: 44)
                }.disabled(picking || editing != nil).accessibilityLabel("Add attachment")
                HStack(alignment: .bottom, spacing: 2) {
                    TextField("Message", text: $text, axis: .vertical)
                        .font(.body).lineLimit(1...6).focused($focused)
                        .padding(.leading, 14).padding(.vertical, 11)
                        .onChange(of: text) { _, _ in onType() }
                    if canSend {
                        Button(action: send) {
                            Image(systemName: "arrow.up").font(.system(size: 16, weight: .bold)).foregroundStyle(.white)
                                .frame(width: 28, height: 28).background(Color.beam.gradient, in: Circle())
                                .frame(width: 44, height: 44)
                        }.disabled(sending).transition(.scale.combined(with: .opacity))
                            .accessibilityLabel(editing != nil ? "Save edit" : "Send message")
                    } else { Spacer(minLength: 12).frame(width: 12) }
                }
                .overlay(RoundedRectangle(cornerRadius: 23).strokeBorder(.secondary.opacity(0.25), lineWidth: 0.5))
                .animation(.spring(response: 0.3, dampingFraction: 0.7), value: canSend)
            }.padding(.horizontal, 8).padding(.bottom, 8).padding(.top, 4)
        }.chatGlass(radius: 26).padding(.horizontal, 8).padding(.top, 6).padding(.bottom, 4)
            .sheet(isPresented: $gifPicker) { ChatGifPicker(friendID: friendID).environmentObject(bridge) }
            .onChange(of: editing?.id) { _, _ in if let editing { text = editing.text ?? ""; focused = true } }
            .onChange(of: reply?.id) { _, _ in if reply != nil { focused = true } }
            .onChange(of: scenePhase) { _, phase in if phase != .active { stopTyping() } }
            .onDisappear { stopTyping() }
    }
    private func pick(_ source: String) {
        focused = false; picking = true; reply = nil
        bridge.perform { defer { picking = false }; try await bridge.sendChatFiles(friendId: friendID, source: source) }
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
                            AsyncImage(url: URL(string: gif.thumbUrl ?? "")) { image in image.resizable().scaledToFit() } placeholder: { ProgressView() }
                                .frame(minHeight: 100).clipShape(RoundedRectangle(cornerRadius: 14))
                        }.accessibilityLabel(gif.title ?? "Send GIF")
                    }
                }.padding(16)
                if loading { ProgressView().padding() }
                if let error { Text(error).foregroundStyle(.secondary).padding() }
                if !loading && error == nil && results.isEmpty { Text("No GIFs found").foregroundStyle(.secondary).padding() }
            }.beamCanvas().navigationTitle("GIFs").navigationBarTitleDisplayMode(.inline)
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
