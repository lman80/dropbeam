import SwiftUI
import UIKit
import UserNotifications

struct RootView: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.scenePhase) private var scenePhase
    private var tabs: some View {
        TabView(selection: $bridge.selectedTab) {
            // Files someone wants to send her wait for Accept on the Send tab: say so from any tab.
            SendView().tabItem { Label("Send", systemImage: "paperplane.fill") }.tag("send")
                .badge(bridge.sendTransfers.filter { $0.direction == "receive" && $0.state == "waitingForAccept" }.count)
            FriendsView().tabItem { Label("Friends", systemImage: "person.2.fill") }.tag("friends").badge(bridge.friendRequests.count)
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
                Label("Offline — transfers resume when you reconnect", systemImage: "wifi.slash")
                    .multilineTextAlignment(.leading)
                    .font(.footnote.weight(.semibold))
                    .accessibilityHint("Connect to Wi-Fi or cellular to reach other devices.")
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
        .modifier(RootPresentations())
        .animation(.snappy, value: bridge.toast)
        .onChange(of: bridge.toast) { _, toast in if let toast { UIAccessibility.post(notification: .announcement, argument: toast) } }
        .preferredColorScheme(bridge.settings?.theme == "dark" ? .dark : bridge.settings?.theme == "light" ? .light : nil)
        .task { try? await bridge.nativeChatFocus(scenePhase == .active); await bridge.mailboxFetchNow(); await bridge.refreshFriendRequests() }
        .onChange(of: bridge.selectedTab) { _, tab in
            SuperFeedback.setContext(["route": tab]) // sent as the report's url
            bridge.perform { try await bridge.setView(name: tab) }
        }
        .onChange(of: scenePhase) { _, phase in
            Task { try? await bridge.nativeChatFocus(phase == .active) }
            // Anything a Transfer Server held for us while we were away.
            if phase == .active { Task { await bridge.mailboxFetchNow() } }
        }
        .alert("That Didn’t Work", isPresented: showsError) {
            // A device-link code used outside Settings → Devices (S1): offer the way there.
            if Bridge.isDeviceCodeMessage(bridge.errorMessage) {
                Button("Open Settings → Devices") { bridge.errorMessage = nil; bridge.openDevicesSettings() }
            }
            // A permission she turned off: one tap to the place that turns it back on.
            if PlainError.needsSettings(bridge.errorMessage) {
                Button("Open Settings") { bridge.errorMessage = nil; SystemSettings.open() }
            }
            Button("OK", role: .cancel) { bridge.errorMessage = nil }
        } message: { Text(bridge.errorMessage.map(PlainError.humanize) ?? "Please try again.") }
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
                    Text(title.hasPrefix("Preparing files") || title.hasPrefix("Preparing folder") ? "Files stored online may take a moment to download." : "Photos stored in iCloud may take a moment to download.")
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

/// Everything the root presents over the tabs: first-run setup, Send To, folder
/// invites, invite links — one at a time, setup first.
private struct RootPresentations: ViewModifier {
    @EnvironmentObject private var bridge: Bridge
    private var settingUp: Bool { bridge.onboarding || bridge.needsName }
    private var setup: Binding<Bool> { Binding(get: { settingUp }, set: { _ in }) }
    private var sending: Binding<Bool> {
        Binding(get: { !settingUp && !bridge.sendQueue.isEmpty }, set: { presented in
            guard !presented else { return }
            bridge.pickedToSend = []; bridge.pendingSend = []
            bridge.perform { try await bridge.action("dismissSend") }
        })
    }
    private var folderInvite: Binding<FolderInvite?> {
        Binding(get: { !settingUp && bridge.sendQueue.isEmpty ? bridge.folderInvites.first : nil },
                set: { if $0 == nil && !bridge.folderInvites.isEmpty { bridge.folderInvites.removeFirst() } })
    }
    private var link: Binding<IncomingLink?> {
        Binding(get: { !settingUp && bridge.sendQueue.isEmpty && bridge.folderInvites.isEmpty ? bridge.incomingLink : nil },
                set: { if $0 == nil { bridge.incomingLink = nil } })
    }
    func body(content: Content) -> some View {
        content
            .fullScreenCover(isPresented: setup) { OnboardingFlow().environmentObject(bridge) }
            .sheet(isPresented: sending) { SendToSheet(paths: bridge.sendQueue) }
            .sheet(item: folderInvite) { invite in FolderInviteSheet(invite: invite) }
            // An invite link / Camera-scanned friend QR opened the app: add them.
            .sheet(item: link) { link in AddFriendSheet(initialCode: link.value).environmentObject(bridge) }
            // The floating feedback button would sit over the setup copy.
            .onChange(of: settingUp, initial: true) { _, now in SuperFeedback.setSuppressed(now) }
            .task(id: settingUp || !bridge.nameKnown) {
                // Existing installs that never answered the notification prompt get it
                // once here (new ones are asked inside setup, at the moment it's explained).
                // Wait until the engine says whether this is a new install: asking before
                // that popped the system prompt over the Welcome screen, unexplained.
                guard bridge.nameKnown, !settingUp else { return }
                try? await Task.sleep(for: .seconds(2))
                let center = UNUserNotificationCenter.current()
                if bridge.nameKnown, !settingUp, await center.notificationSettings().authorizationStatus == .notDetermined {
                    _ = try? await center.requestAuthorization(options: [.alert, .sound, .badge])
                    PushRegistration.permissionGranted()
                }
            }
    }
}
