import SwiftUI

/// "from Alex": who a received file came from, with their picture (GitHub #12).
/// The name is the sender recorded when the file landed — the same provenance
/// DropBeam stamps on the file itself. A friend with that name lends their photo.
struct FromChip: View {
    @EnvironmentObject private var bridge: Bridge
    let name: String
    private var friend: Friend {
        bridge.friends.first { $0.displayName == name || $0.name == name } ?? Friend(id: name, name: name)
    }
    var body: some View {
        HStack(spacing: 5) {
            ContactAvatar(friend: friend, size: 18)
            Text("from \(name)").lineLimit(1)
        }
        .font(.subheadline)
        .foregroundStyle(.secondary)
        .padding(.leading, 2).padding(.trailing, 8).padding(.vertical, 2)
        .background(Capsule().fill(Color(uiColor: .tertiarySystemFill)))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("From \(name)")
    }
}
