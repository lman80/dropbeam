import SwiftUI

/// "Folder" send source on the Send screen: a whole folder (subfolders included)
/// goes to a friend, one of your devices, or Quick Send — like desktop drag & drop.
/// The picked folder is copied into temporary storage first (iOS only lends the
/// original to the app while the picker is open).
struct SendFolderButton: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var picking = false
    var body: some View {
        ActionTile(title: "Folder", symbol: "folder", large: true) {
            picking = true
            bridge.perform { defer { picking = false }; try await bridge.pickAndSend(source: "folder") }
        }.disabled(picking).accessibilityLabel("Send a Folder")
    }
}
