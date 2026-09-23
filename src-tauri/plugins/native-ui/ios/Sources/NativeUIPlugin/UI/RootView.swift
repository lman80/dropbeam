import SwiftUI
import UIKit

struct RootView: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.scenePhase) private var scenePhase
    private var tabs: some View {
        TabView(selection: $bridge.selectedTab) {
            SendView().tabItem { Label("Send", systemImage: "paperplane.fill") }.tag("send")
            FriendsView().tabItem { Label("Friends", systemImage: "person.2.fill") }.tag("friends")
            ChatsView()
                .tabItem { Label("Chats", systemImage: "bubble.left.and.bubble.right.fill") }.tag("chat").badge(bridge.unread)
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
                Label("You’re offline. Connect to Wi-Fi or cellular to reach other devices.", systemImage: "wifi.slash")
                    .font(.footnote.weight(.medium)).multilineTextAlignment(.leading)
                    .padding(.horizontal, 16).padding(.vertical, 10).glassCapsule()
                    .padding(.horizontal, 16).padding(.top, 4)
                    .accessibilityAddTraits(.updatesFrequently)
            }
        }
        .overlay(alignment: .top) {
            if let toast = bridge.toast {
                Label(toast, systemImage: "checkmark.circle.fill").font(.subheadline.weight(.medium)).labelStyle(ToastLabelStyle())
                    .padding(.horizontal, 18).padding(.vertical, 12).glassCapsule().padding(.top, 8)
                    .transition(.move(edge: .top).combined(with: .opacity))
                    .accessibilityAddTraits(.updatesFrequently).allowsHitTesting(false)
            }
        }
        .sheet(isPresented: Binding(get: { bridge.needsName }, set: { _ in })) { OnboardingSheet() }
        .sheet(isPresented: Binding(get: { !bridge.needsName && !bridge.pendingSend.isEmpty }, set: { if !$0 { bridge.pendingSend = []; bridge.perform { try await bridge.action("dismissSend") } } })) {
            SendToSheet(paths: bridge.pendingSend)
        }
        .sheet(item: Binding(get: { !bridge.needsName && bridge.pendingSend.isEmpty ? bridge.folderInvites.first : nil }, set: { if $0 == nil && !bridge.folderInvites.isEmpty { bridge.folderInvites.removeFirst() } })) { invite in FolderInviteSheet(invite: invite) }

        .animation(.snappy, value: bridge.toast)
        .onChange(of: bridge.toast) { _, toast in if let toast { UIAccessibility.post(notification: .announcement, argument: toast) } }
        // The conversation's composer and bubbles own the screen edges.
        .onChange(of: bridge.selectedTab == "chat" && !bridge.chatPath.isEmpty) { _, inThread in SuperFeedback.setSuppressed(inThread) }
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
                }.padding(24).frame(maxWidth: 360).modifier(PanelGlass()).padding(32)
            }
        }
    }
}

private struct ToastLabelStyle: LabelStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 8) { configuration.icon.foregroundStyle(.tint); configuration.title }
    }
}
private struct PanelGlass: ViewModifier {
    @ViewBuilder func body(content: Content) -> some View {
        if #available(iOS 26, *) { content.glassEffect(.regular, in: .rect(cornerRadius: 28)) }
        else { content.background(.regularMaterial, in: RoundedRectangle(cornerRadius: 28, style: .continuous)) }
    }
}
