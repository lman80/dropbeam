import SwiftUI

struct RootView: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.scenePhase) private var scenePhase
    private var tabs: some View {
        TabView(selection: $bridge.selectedTab) {
            SendView().tabItem { Label("Send", systemImage: "paperplane.fill") }.tag("send")
            FriendsView().tabItem { Label("Friends", systemImage: "person.2.fill") }.tag("friends")
            ChatsView()
                .tabItem { Label("Chat", systemImage: "bubble.left.and.bubble.right.fill") }.tag("chat").badge(bridge.unread)
            HistoryView().tabItem { Label("History", systemImage: "clock.fill") }.tag("history")
            SettingsView().tabItem { Label("Settings", systemImage: "gearshape.fill") }.tag("settings")
        }
    }
    private var showsError: Binding<Bool> {
        Binding(get: { bridge.errorMessage != nil }, set: { presented in
            if !presented { bridge.errorMessage = nil }
        })
    }
    var body: some View {
        tabs.tint(.beam)
        .overlay { MediaPreparationOverlay() }
        .safeAreaInset(edge: .top) {
            if bridge.networkAvailable == false {
                Text("No network connection. Connect to Wi-Fi or cellular to reach other devices.")
                    .font(.footnote).multilineTextAlignment(.center).padding(10)
                    .frame(maxWidth: .infinity).background(.regularMaterial)
            }
        }
        .overlay(alignment: .top) {
            if let toast = bridge.toast { Text(toast).font(.subheadline).padding(16).background(.regularMaterial, in: Capsule()).padding().accessibilityAddTraits(.updatesFrequently).allowsHitTesting(false) }
        }
        .sheet(isPresented: Binding(get: { bridge.needsName }, set: { _ in })) { OnboardingSheet() }
        .sheet(isPresented: Binding(get: { !bridge.needsName && !bridge.pendingSend.isEmpty }, set: { if !$0 { bridge.pendingSend = []; bridge.perform { try await bridge.action("dismissSend") } } })) {
            SendToSheet(paths: bridge.pendingSend)
        }
        .sheet(item: Binding(get: { !bridge.needsName && bridge.pendingSend.isEmpty ? bridge.folderInvites.first : nil }, set: { if $0 == nil && !bridge.folderInvites.isEmpty { bridge.folderInvites.removeFirst() } })) { invite in FolderInviteSheet(invite: invite) }

        .preferredColorScheme(bridge.settings?.theme == "dark" ? .dark : bridge.settings?.theme == "light" ? .light : nil)
        .task { try? await bridge.nativeChatFocus(scenePhase == .active) }
        .onChange(of: bridge.selectedTab) { _, tab in
            SuperFeedback.setContext(["screen": tab])
            bridge.perform { try await bridge.setView(name: tab) }
        }
        .onChange(of: scenePhase) { _, phase in
            Task { try? await bridge.nativeChatFocus(phase == .active) }
        }
        .alert("Couldn’t complete that", isPresented: showsError) {
            Button("OK", role: .cancel) { bridge.errorMessage = nil }
        } message: { Text(bridge.errorMessage ?? "Please try again.") }
    }
}

struct MediaPreparationOverlay: View {
    @EnvironmentObject private var bridge: Bridge
    var body: some View {
        if let title = bridge.preparingMedia {
            ZStack {
                Color.black.opacity(0.18).ignoresSafeArea()
                VStack(spacing: 16) {
                    ProgressView(title)
                    Text(title == "Preparing photo…" ? "Photos stored in iCloud may take a moment to download." : "Files stored online may take a moment to download.")
                        .font(.footnote).foregroundStyle(.secondary).multilineTextAlignment(.center)
                    Button("Cancel") { bridge.cancelMediaPreparation() }.beamButton()
                }.padding(24).background(.regularMaterial, in: RoundedRectangle(cornerRadius: 24)).padding(32)
            }
        }
    }
}
