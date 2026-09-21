import SwiftUI

struct RootView: View {
    @EnvironmentObject private var bridge: Bridge
    var body: some View {
        TabView(selection: $bridge.selectedTab) {
            SendView().tabItem { Label("Send", systemImage: "paperplane.fill") }.tag("send")
            FriendsView().tabItem { Label("Friends", systemImage: "person.2.fill") }.tag("friends")
            PlaceholderView(title: "Chat", symbol: "bubble.left.and.bubble.right.fill", headline: "A little closer, wherever you are.", detail: "Your conversations are connected. Native messages and attachments are coming next.")
                .tabItem { Label("Chat", systemImage: "bubble.left.and.bubble.right.fill") }.tag("chat").badge(bridge.unread)
            PlaceholderView(title: "History", symbol: "clock.arrow.circlepath", headline: "Every arrival has a story.", detail: "Your transfer history is safely kept. A native timeline is coming next.")
                .tabItem { Label("History", systemImage: "clock.fill") }.tag("history")
            PlaceholderView(title: "Settings", symbol: "slider.horizontal.3", headline: "Make room for your way.", detail: "Your preferences are already in use. Native profile, device linking and controls are coming next.")
                .tabItem { Label("Settings", systemImage: "gearshape.fill") }.tag("settings")
        }
        .tint(.beam)
        .preferredColorScheme(bridge.settings?.theme == "dark" ? .dark : bridge.settings?.theme == "light" ? .light : nil)
        .onChange(of: bridge.selectedTab) { _, tab in bridge.perform { try await bridge.setView(name: tab) } }
        .alert("Couldn’t complete that", isPresented: Binding(get: { bridge.errorMessage != nil }, set: { if !$0 { bridge.errorMessage = nil } })) {
            Button("OK", role: .cancel) { bridge.errorMessage = nil }
        } message: { Text(bridge.errorMessage ?? "Please try again.") }
    }
}
struct PlaceholderView: View {
    let title: String
    let symbol: String
    let headline: String
    let detail: String
    var body: some View {
        NavigationStack {
            ScrollView {
                GlassCard {
                    VStack(alignment: .leading, spacing: 22) {
                        Image(systemName: symbol).font(.system(size: 52, weight: .light)).foregroundStyle(.tint).padding(.vertical, 12)
                        Text(headline).font(.title2.weight(.semibold))
                        Text(detail).font(.body).foregroundStyle(.secondary)
                        Text("COMING NEXT").font(.caption.weight(.semibold)).tracking(2).foregroundStyle(.secondary)
                    }.padding(.vertical, 12)
                }.padding(20)
            }.navigationTitle(title).beamCanvas()
        }
    }
}
