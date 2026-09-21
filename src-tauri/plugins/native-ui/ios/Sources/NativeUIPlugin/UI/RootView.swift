import SwiftUI

struct RootView: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.scenePhase) private var scenePhase
    var body: some View {
        TabView(selection: $bridge.selectedTab) {
            SendView().tabItem { Label("Send", systemImage: "paperplane.fill") }.tag("send")
            FriendsView().tabItem { Label("Friends", systemImage: "person.2.fill") }.tag("friends")
            ChatsView()
                .tabItem { Label("Chat", systemImage: "bubble.left.and.bubble.right.fill") }.tag("chat").badge(bridge.unread)
            HistoryView().tabItem { Label("History", systemImage: "clock.fill") }.tag("history")
            SettingsView().tabItem { Label("Settings", systemImage: "gearshape.fill") }.tag("settings")
        }
        .tint(.beam)
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
        .onChange(of: bridge.selectedTab) { _, tab in bridge.perform { try await bridge.setView(name: tab) } }
        .onChange(of: scenePhase) { _, phase in
            Task { try? await bridge.nativeChatFocus(phase == .active) }
        }
        .alert("Couldn’t complete that", isPresented: Binding(get: { bridge.errorMessage != nil }, set: { if !$0 { bridge.errorMessage = nil } })) {
            Button("OK", role: .cancel) { bridge.errorMessage = nil }
        } message: { Text(bridge.errorMessage ?? "Please try again.") }
    }
}