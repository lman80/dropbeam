import SwiftUI
import UIKit

// MARK: - Block & Report (App Store guideline 1.2)
//
// Block: removes the person (all their devices) on every own device and the engine
// answers their messages, files, folder invites and Location requests like a
// stranger's — they aren't told. Report: a short sheet (reason, optional message
// text, notes, "also block") that opens a pre-filled email to the developer; the
// address and wording come from src/lib/report.ts through the bridge.

/// What the Report sheet is about: a person, or one of their messages.
struct ReportTarget: Identifiable {
    let friend: Friend
    var message: ChatMessage? = nil
    var alsoBlock = false
    var id: String { friend.id + "|" + (message?.id ?? "") }
}

extension View {
    /// The Block confirmation (with "Block and Report…") and the Report sheet,
    /// shared by the friend list, friend page and conversation.
    func safetyPrompts(block: Binding<Friend?>, report: Binding<ReportTarget?>) -> some View {
        modifier(SafetyPrompts(block: block, report: report))
    }
}

private struct SafetyPrompts: ViewModifier {
    @EnvironmentObject private var bridge: Bridge
    @Binding var block: Friend?
    @Binding var report: ReportTarget?
    func body(content: Content) -> some View {
        content
            .confirmationDialog(block.map { "Block \($0.displayName)?" } ?? "", isPresented: Binding(get: { block != nil }, set: { if !$0 { block = nil } }), titleVisibility: .visible, presenting: block) { friend in
                Button("Block", role: .destructive) {
                    bridge.perform { try await bridge.blockFriend(id: friend.id); Haptics.success(); bridge.showToast("\(friend.displayName) is blocked") }
                }
                Button("Block and Report…") { report = ReportTarget(friend: friend, alsoBlock: true) }
            } message: { friend in
                Text("\(friend.displayName) is removed from your friends on all your devices and can’t message you, send you files, invite you to folders or browse your Locations. They aren’t told. Unblock any time in Settings → Blocked.")
            }
            .sheet(item: $report) { target in ReportSheet(target: target).environmentObject(bridge) }
    }
}

struct ReportSheet: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.dismiss) private var dismiss
    let target: ReportTarget
    @State private var reasons: [ReportReason] = []
    @State private var reason: String?
    @State private var includeText = true
    @State private var notes = ""
    @State private var alsoBlock: Bool
    @State private var sending = false
    @State private var loadError: String?
    @State private var fallback: ReportMail?
    init(target: ReportTarget) { self.target = target; _alsoBlock = State(initialValue: target.alsoBlock) }
    private var message: ChatMessage? { target.message }
    private var isFile: Bool { message?.kind == "file" && message?.gif == nil }
    private var quote: String? {
        guard let message, message.deleted != true else { return nil }
        if isFile { return message.files?.isEmpty == false ? message.files?.joined(separator: ", ") : nil }
        return message.gif == nil && message.text?.isEmpty == false ? message.text : nil
    }
    private var title: String { message == nil ? "Report \(target.friend.displayName)" : isFile ? "Report File" : "Report Message" }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    if let loadError { Text(loadError).foregroundStyle(.secondary) }
                    else if reasons.isEmpty { ProgressView().frame(maxWidth: .infinity) }
                    ForEach(reasons) { r in
                        Button { reason = r.id; Haptics.tap() } label: {
                            HStack {
                                Text(r.label).foregroundStyle(.primary)
                                Spacer()
                                if reason == r.id { Image(systemName: "checkmark").font(.body.weight(.semibold)).foregroundStyle(.tint) }
                            }.contentShape(Rectangle())
                        }
                        .accessibilityAddTraits(reason == r.id ? .isSelected : [])
                    }
                } header: { Text("What’s Wrong?") }
                if let quote {
                    Section {
                        Toggle(isFile ? "Include File Name" : "Include Message Text", isOn: $includeText).tint(.green)
                        Text(quote).font(.subheadline).foregroundStyle(.secondary).lineLimit(5)
                            .opacity(includeText ? 1 : 0.45)
                    } footer: { Text(isFile ? "Only the name is included — never the file itself." : "Only this message is included, and only if you leave this on.") }
                }
                Section {
                    TextField("What happened (optional)", text: $notes, axis: .vertical).lineLimit(3...6)
                } header: { Text("Details") }
                Section {
                    Toggle("Also Block \(target.friend.displayName)", isOn: $alsoBlock).tint(.green)
                } footer: {
                    Text("Your report opens as an email to the DropBeam team in your Mail app — you’ll see it before it’s sent. We review reports within 24 hours.")
                }
            }
            .navigationTitle(title).navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    if sending { ProgressView() } else { Button("Continue") { send() }.disabled(reason == nil) }
                }
            }
            .task {
                guard reasons.isEmpty else { return }
                do { reasons = try await bridge.reportReasons() } catch { loadError = error.localizedDescription }
            }
            .alert("Send Your Report", isPresented: Binding(get: { fallback != nil }, set: { if !$0 { fallback = nil; dismiss() } }), presenting: fallback) { mail in
                Button("Copy Report") { UIPasteboard.general.string = "To: \(mail.to)\nSubject: \(mail.subject)\n\n\(mail.body)"; bridge.showToast("Report copied") }
                Button("OK", role: .cancel) {}
            } message: { mail in Text("No mail app is set up. Email your report to \(mail.to) — copy it to paste into any email app.") }
        }
        .tint(.beam)
    }

    private func send() {
        guard let reason else { return }
        sending = true
        var args: [String: Any] = ["friendId": target.friend.id, "reason": reason, "includeText": includeText && quote != nil, "notes": notes, "alsoBlock": alsoBlock]
        if let message { args["messageId"] = message.id }
        let friend = target.friend, block = alsoBlock
        bridge.perform {
            defer { sending = false }
            let mail = try await bridge.reportMail(args)
            let opened: Bool
            if let url = URL(string: mail.url) { opened = await UIApplication.shared.open(url) } else { opened = false }
            if block { try await bridge.blockFriend(id: friend.id) }
            if opened { bridge.showToast(block ? "\(friend.displayName) is blocked. Send the email to finish your report." : "Send the email to finish your report."); dismiss() }
            else { fallback = mail }
        }
    }
}

/// Settings → Blocked: everyone the user blocked, with Unblock.
struct BlockedView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var unblocking: BlockedPerson?
    var body: some View {
        List {
            if !bridge.blocked.isEmpty {
                Section {
                    ForEach(bridge.blocked) { person in
                        HStack(spacing: 14) {
                            FriendAvatar(friend: Friend(id: person.id, name: person.name), size: 40)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(person.name).font(.body.weight(.semibold)).lineLimit(1)
                                Text(detail(person)).font(.subheadline).foregroundStyle(.secondary).lineLimit(1)
                            }
                            Spacer(minLength: 4)
                            Button("Unblock") { unblocking = person }.beamButton()
                        }
                        .accessibilityElement(children: .combine)
                        .accessibilityAction(named: "Unblock") { unblocking = person }
                        .swipeActions(edge: .trailing) {
                            Button { unblocking = person } label: { Label("Unblock", systemImage: "person.fill.checkmark") }.tint(.beam)
                        }
                    }
                } footer: { Text("Blocked people can’t message you, send you files, invite you to folders or browse your Locations. Blocks apply on all your linked devices.") }
            }
        }
        .beamList()
        .overlay {
            if bridge.blocked.isEmpty {
                ContentUnavailableView {
                    Label("No One Blocked", systemImage: "hand.raised")
                } description: {
                    Text("To block someone, open their page in Friends or the ⋯ menu in your chat with them.")
                }
            }
        }
        .navigationTitle("Blocked").navigationBarTitleDisplayMode(.inline)
        .animation(.smooth, value: bridge.blocked.map(\.id))
        .confirmationDialog(unblocking.map { "Unblock \($0.name)?" } ?? "", isPresented: Binding(get: { unblocking != nil }, set: { if !$0 { unblocking = nil } }), titleVisibility: .visible, presenting: unblocking) { person in
            Button("Unblock") { bridge.perform { try await bridge.unblockPerson(id: person.id); bridge.showToast("\(person.name) is unblocked") } }
        } message: { person in Text("\(person.name) won’t be added back as a friend. Add them again with their code if you want to.") }
    }
    private func detail(_ p: BlockedPerson) -> String {
        let when = p.at > 0 ? "Blocked " + Date(timeIntervalSince1970: p.at / 1000).formatted(date: .abbreviated, time: .omitted) : "Blocked"
        return p.endpointIds.count > 1 ? "\(when) · \(p.endpointIds.count) devices" : when
    }
}
