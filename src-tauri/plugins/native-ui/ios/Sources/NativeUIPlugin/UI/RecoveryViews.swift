import SwiftUI
import UIKit

// Recovery code (docs/RECOVERY-CODE.md): save the 12 (or 24) words, check two of
// them, restore an account on a new iPhone, and see what came back afterwards.
// The words only ever come from the engine for the screen; nothing here stores them.

struct RecoveryOldDevice: Decodable, Identifiable, Hashable {
    let endpointId: String
    let kind: String?
    let os: String?
    let model: String?
    let via: String
    var id: String { endpointId }
    /// "Your iPhone 12", "Your Mac".
    var title: String {
        if let model, !model.isEmpty { return "Your \(model)" }
        switch os {
        case "ios": return kind == "tablet" ? "Your iPad" : "Your iPhone"
        case "macos": return "Your Mac"
        case "windows": return "Your PC"
        case "linux": return "Your Linux computer"
        default: return kind == "phone" ? "Your phone" : "Your computer"
        }
    }
}
struct RecoveryFolderNote: Decodable, Hashable { let name: String; let with: String }
struct RecoveryRestoreView: Decodable, Hashable {
    let restoredAt: Double
    let friendsSynced: Int
    let returned: [String]
    let oldDevices: [RecoveryOldDevice]
    let folders: [RecoveryFolderNote]
    var summary: String {
        let n = max(friendsSynced, returned.count)
        if n == 0 { return "Your friends will find you as their DropBeam opens — this can take a few hours. Nothing to do here." }
        let who = returned.isEmpty ? "" : " (\(ListFormatter.localizedString(byJoining: Array(returned.prefix(3)))))"
        return "\(n == 1 ? "1 friend has" : "\(n) friends have") found you again\(who). Others will as their DropBeam opens."
    }
}
struct RecoveryStatus: Decodable, Hashable {
    let saved: Bool
    let hasAccount: Bool
    let laterAt: Double
    let restore: RecoveryRestoreView?
}
struct RecoveryCheck: Decodable, Hashable {
    let count: Int
    let unknown: [Int]
    let complete: Bool
    let valid: Bool
    let problem: String?
}
struct RecoveryReveal: Decodable { let words: [String]; let qr: String }

enum RecoveryText {
    static let why = "If you ever lose all your phones and computers, these words bring back your account on a new one. Your friends will recognize you again and send back your chats."
    static let warning = "Anyone with these words can become you. Keep them somewhere safe, like with your important papers."
    static let never = "DropBeam will never ask for them except when you set up a new device yourself. Don’t send them to anyone, not even to us."
}

extension Bridge {
    func recoveryStatus() async throws -> RecoveryStatus { try await call("recoveryStatus") }
    func recoveryReveal() async throws -> RecoveryReveal { try await call("recoveryReveal") }
    func recoveryConfirmSaved(_ answers: [(index: Int, word: String)]) async throws {
        try await action("recoveryConfirmSaved", ["answers": answers.map { ["index": $0.index, "word": $0.word] }])
    }
    func recoveryLater() async throws { try await action("recoveryLater") }
    func recoveryCheck(_ text: String) async throws -> RecoveryCheck { try await call("recoveryCheck", ["text": text]) }
    func recoveryRestore(_ text: String) async throws { try await action("recoveryRestore", ["text": text]) }
    func recoveryRemoveOldDevices(_ ids: [String]) async throws { try await action("recoveryRemoveOldDevices", ["endpointIds": ids]) }
    func recoveryKeepOldDevice(_ id: String) async throws { try await action("recoveryKeepOldDevice", ["endpointId": id]) }
}

/// Two "which word is number N?" questions, four choices each from the code.
struct RecoveryQuiz {
    struct Question { let index: Int; let options: [String] }
    static func make(_ words: [String], count: Int = 2) -> [Question] {
        guard words.count >= 4 else { return [] }
        var asked = Set<Int>()
        var out: [Question] = []
        while out.count < min(count, words.count) {
            let i = Int.random(in: 0..<words.count)
            guard asked.insert(i).inserted else { continue }
            var options = [words[i]]
            for j in (0..<words.count).shuffled() where options.count < 4 && !options.contains(words[j]) { options.append(words[j]) }
            out.append(Question(index: i, options: options.shuffled()))
        }
        return out
    }
}

/// The numbered words in two columns (1–6 | 7–12), like the printed sheet.
private struct WordColumns: View {
    let words: [String]
    var body: some View {
        let half = (words.count + 1) / 2
        HStack(alignment: .top, spacing: 18) {
            ForEach(0..<2, id: \.self) { col in
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(Array(words.indices.filter { col == 0 ? $0 < half : $0 >= half }), id: \.self) { i in
                        HStack(alignment: .firstTextBaseline, spacing: 8) {
                            Text("\(i + 1)").font(.caption.monospacedDigit()).foregroundStyle(.secondary).frame(minWidth: 20, alignment: .trailing)
                            Text(words[i]).font(.system(.title3, design: .monospaced).weight(.semibold))
                        }
                        .accessibilityElement(children: .combine).accessibilityLabel("Word \(i + 1): \(words[i])")
                    }
                }.frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .padding(16)
        .background(Color(uiColor: .secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 16, style: .continuous))
    }
}

/// What gets printed: the words, the QR and how to use them.
private struct RecoveryPrintSheet: View {
    let words: [String]
    let qr: UIImage?
    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("DropBeam recovery code").font(.title.bold())
            Text(RecoveryText.why).font(.body)
            Text(RecoveryText.warning).font(.body.bold())
            WordColumns(words: words).environment(\.colorScheme, .light)
            if let qr { Image(uiImage: qr).interpolation(.none).resizable().frame(width: 160, height: 160) }
            Text("To use it: install DropBeam on the new device, choose “Restore with Recovery Code”, then type these words or scan this code.").font(.callout)
        }
        .padding(36).frame(width: 612, alignment: .leading).background(.white).foregroundStyle(.black)
    }
}

private func qrImage(_ text: String) -> UIImage? {
    let filter = CIFilter(name: "CIQRCodeGenerator")
    filter?.setValue(Data(text.utf8), forKey: "inputMessage")
    filter?.setValue("M", forKey: "inputCorrectionLevel")
    guard let output = filter?.outputImage?.transformed(by: CGAffineTransform(scaleX: 8, y: 8)),
          let cg = CIContext().createCGImage(output, from: output.extent) else { return nil }
    return UIImage(cgImage: cg)
}

/// Save Your Recovery Code: why → the words (+ QR, print) → check two words → done.
struct SaveRecoverySheet: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.dismiss) private var dismiss
    /// Offered after setup: "Later" remembers the person put it off.
    var offer = false
    var onFinish: () -> Void = {}
    private enum Step { case intro, words, quiz, done }
    @State private var step: Step = .intro
    @State private var words: [String] = []
    @State private var qr = ""
    @State private var quiz: [RecoveryQuiz.Question] = []
    @State private var q = 0
    @State private var answers: [(index: Int, word: String)] = []
    @State private var wrong = false
    @State private var busy = false
    @State private var error: String?
    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 18) { content }
                    .padding(.horizontal, 24).padding(.vertical, 16).frame(maxWidth: 560).frame(maxWidth: .infinity)
            }
            .background(Color(uiColor: .systemGroupedBackground))
            .navigationTitle(title).navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    if step != .done { Button(offer && step == .intro ? "Later" : "Cancel") { close(later: offer && step == .intro) } }
                }
            }
            .safeAreaInset(edge: .bottom) { actions.padding(.horizontal, 24).padding(.vertical, 10).frame(maxWidth: 560).frame(maxWidth: .infinity).background(Color(uiColor: .systemGroupedBackground)) }
        }
        .interactiveDismissDisabled(busy)
        .onDisappear { words = []; qr = "" }
    }
    private var title: String {
        switch step { case .intro: "Recovery Code"; case .words: "Write These Down"; case .quiz: "Check Your Paper"; case .done: "Saved" }
    }
    @ViewBuilder private var content: some View {
        switch step {
        case .intro:
            Image(systemName: "key.horizontal").font(.system(size: 52, weight: .light)).foregroundStyle(.tint).frame(maxWidth: .infinity).padding(.top, 12).accessibilityHidden(true)
            Text("Save Your Recovery Code").font(.title.bold()).frame(maxWidth: .infinity)
            Text(RecoveryText.why).font(.body)
            Text("Your code is 12 words. Have a pen and paper ready — it takes about two minutes.").font(.body).foregroundStyle(.secondary)
        case .words:
            Text("Write each word on paper, in order, exactly as shown.").font(.body)
            WordColumns(words: words)
            Label(RecoveryText.warning, systemImage: "exclamationmark.shield.fill").font(.subheadline)
                .padding(12).frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.orange.opacity(0.14), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            Text(RecoveryText.never).font(.footnote).foregroundStyle(.secondary)
            if !qr.isEmpty {
                VStack(spacing: 6) {
                    QRCodeView(code: qr, side: 150)
                    Text("The same code as a picture, for printing").font(.footnote).foregroundStyle(.secondary)
                }.frame(maxWidth: .infinity)
            }
        case .quiz:
            if quiz.indices.contains(q) {
                Text("Look at your paper. Which word is number \(quiz[q].index + 1)?").font(.title3.weight(.semibold))
                LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 10) {
                    ForEach(quiz[q].options, id: \.self) { w in
                        Button { choose(w) } label: { Text(w).font(.system(.title3, design: .monospaced)).frame(maxWidth: .infinity, minHeight: 44) }
                            .beamButton().disabled(busy)
                    }
                }
                Text("Question \(q + 1) of \(quiz.count)").font(.footnote).foregroundStyle(.secondary)
                if wrong {
                    Label("That’s not it. Check word number \(quiz[q].index + 1) on your paper — if it’s missing or different, tap Show the Words Again.", systemImage: "exclamationmark.triangle.fill")
                        .font(.subheadline).foregroundStyle(.red)
                }
            }
        case .done:
            Image(systemName: "checkmark.seal.fill").font(.system(size: 56)).foregroundStyle(.green).frame(maxWidth: .infinity).padding(.top, 12).accessibilityHidden(true)
            Text("Your Code Is Saved").font(.title.bold()).frame(maxWidth: .infinity)
            Text("Keep the paper somewhere safe, like with your important papers. You can see your code again any time in Settings → Devices.").font(.body)
        }
        if let error { Label(error, systemImage: "exclamationmark.triangle.fill").font(.subheadline).foregroundStyle(.red) }
    }
    @ViewBuilder private var actions: some View {
        VStack(spacing: 8) {
            switch step {
            case .intro:
                Button { show() } label: { HStack { if busy { ProgressView() }; Text("Show My Code").font(.headline) }.frame(maxWidth: .infinity, minHeight: 30) }
                    .beamButton(prominent: true).controlSize(.large).disabled(busy)
            case .words:
                Button { startQuiz() } label: { Text("I’ve Written Them Down").font(.headline).frame(maxWidth: .infinity, minHeight: 30) }
                    .beamButton(prominent: true).controlSize(.large)
                Button { print() } label: { Label("Print…", systemImage: "printer").frame(maxWidth: .infinity, minHeight: 36) }
            case .quiz:
                Button("Show the Words Again") { wrong = false; step = .words }.frame(maxWidth: .infinity, minHeight: 44)
            case .done:
                Button { close(later: false) } label: { Text("Done").font(.headline).frame(maxWidth: .infinity, minHeight: 30) }
                    .beamButton(prominent: true).controlSize(.large)
            }
        }
    }
    private func show() {
        busy = true; error = nil
        Task {
            defer { busy = false }
            do { let r = try await bridge.recoveryReveal(); words = r.words; qr = r.qr; step = .words }
            catch { self.error = error.localizedDescription; Haptics.warning() }
        }
    }
    private func startQuiz() { Haptics.tap(); quiz = RecoveryQuiz.make(words); q = 0; answers = []; wrong = false; step = .quiz }
    private func choose(_ word: String) {
        guard quiz.indices.contains(q) else { return }
        let question = quiz[q]
        guard words.indices.contains(question.index), words[question.index] == word else { wrong = true; Haptics.warning(); return }
        wrong = false
        answers.append((question.index, word))
        if q + 1 < quiz.count { Haptics.tap(); q += 1; return }
        busy = true
        Task {
            defer { busy = false }
            do { try await bridge.recoveryConfirmSaved(answers); words = []; qr = ""; Haptics.success(); step = .done }
            catch { self.error = error.localizedDescription }
        }
    }
    private func print() {
        let renderer = ImageRenderer(content: RecoveryPrintSheet(words: words, qr: qrImage(qr)))
        renderer.scale = 2
        guard let image = renderer.uiImage else { error = "Printing isn’t available right now. Write the words down instead."; return }
        let info = UIPrintInfo(dictionary: nil)
        info.outputType = .general
        info.jobName = "DropBeam recovery code"
        let controller = UIPrintInteractionController.shared
        controller.printInfo = info
        controller.printingItem = image
        controller.present(animated: true) { _, _, _ in
            // Don't leave the sheet's image behind in the shared controller.
            controller.printingItem = nil
        }
    }
    private func close(later: Bool) {
        words = []; qr = ""
        if later { Task { try? await bridge.recoveryLater() } }
        onFinish()
        dismiss()
    }
}

/// Restore an account on this iPhone from the words on paper.
struct RestoreRecoverySheet: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.dismiss) private var dismiss
    var onRestored: () -> Void = {}
    @State private var count = 12
    @State private var words: [String] = Array(repeating: "", count: 12)
    @State private var check: RecoveryCheck?
    @State private var busy = false
    @State private var error: String?
    @State private var scanning = false
    @State private var done = false
    @FocusState private var focus: Int?
    private var text: String { words.map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }.joined(separator: " ") }
    var body: some View {
        NavigationStack {
            Form {
                if done {
                    Section {
                        VStack(alignment: .leading, spacing: 12) {
                            Image(systemName: "person.crop.circle.badge.checkmark").font(.system(size: 48)).foregroundStyle(.tint).accessibilityHidden(true)
                            Text("Welcome Back").font(.title.bold())
                            Text("Your account is on this iPhone now.")
                            Text("Your friends will find you over the next few hours, as their DropBeam opens, and send back your chats. You don’t need to do anything — you can see who’s back in Settings → Devices.").foregroundStyle(.secondary)
                            Text("Files and shared folders don’t come back by themselves. Settings → Devices lists the folders your friends remember, so you can ask them to invite you again.").foregroundStyle(.secondary)
                        }.padding(.vertical, 6)
                    }
                } else {
                    Section {
                        Picker("How many words", selection: $count) { Text("12 words").tag(12); Text("24 words").tag(24) }
                            .pickerStyle(.segmented)
                            .onChange(of: count) { _, n in words = (0..<n).map { words.indices.contains($0) ? words[$0] : "" } }
                    } footer: { Text("Type the words from your paper, in order. The first four letters of each word are enough.") }
                    Section {
                        ForEach(words.indices, id: \.self) { i in
                            HStack(spacing: 12) {
                                Text("\(i + 1)").font(.callout.monospacedDigit()).foregroundStyle(.secondary).frame(width: 26, alignment: .trailing)
                                TextField("Word \(i + 1)", text: binding(i))
                                    .font(.system(.body, design: .monospaced))
                                    .textInputAutocapitalization(.never).autocorrectionDisabled().keyboardType(.asciiCapable)
                                    .submitLabel(i == words.count - 1 ? .done : .next)
                                    .focused($focus, equals: i)
                                    .onSubmit { if i + 1 < words.count { focus = i + 1 } else if check?.valid == true { restore() } }
                                    .foregroundStyle(check?.unknown.contains(i) == true ? Color.red : Color.primary)
                                    .accessibilityLabel("Word \(i + 1)")
                            }
                        }
                    } footer: {
                        if let problem = error ?? ((check?.count == count || check?.unknown.isEmpty == false) ? check?.problem : nil) {
                            Label(problem, systemImage: "exclamationmark.triangle.fill").foregroundStyle(.red)
                        } else if check?.valid == true {
                            Label("These words are right. Tap Restore.", systemImage: "checkmark.circle.fill").foregroundStyle(.green)
                        } else {
                            Text("Restoring makes this iPhone yours again: your friends will recognize it and send back your chats.")
                        }
                    }
                    Section {
                        Button { scanning = true } label: { Label("Scan Printed Code", systemImage: "qrcode.viewfinder") }
                    }
                }
            }
            .navigationTitle(done ? "" : "Restore Your Account").navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { if !done { Button("Cancel") { words = []; dismiss() }.disabled(busy) } }
                ToolbarItem(placement: .confirmationAction) {
                    if done { Button("Done") { onRestored(); dismiss() } }
                    else if busy { ProgressView() }
                    else { Button("Restore") { restore() }.disabled(check?.valid != true) }
                }
            }
            .task(id: text) {
                guard !text.isEmpty else { check = nil; return }
                try? await Task.sleep(for: .milliseconds(250))
                guard !Task.isCancelled else { return }
                check = try? await bridge.recoveryCheck(text)
            }
            .sheet(isPresented: $scanning) {
                QRScannerSheet(title: "Scan Your Recovery Code", autoSubmit: true, hint: "Point the camera at the code printed with your words.") { value in
                    guard value.lowercased().hasPrefix("dropbeamrecover1:") else {
                        throw NSError(domain: "DropBeam", code: 1, userInfo: [NSLocalizedDescriptionKey: "That isn’t a recovery code. Look for the code printed with your words."])
                    }
                    fill(Self.split(value), from: 0)
                    scanning = false
                }
            }
        }
        .interactiveDismissDisabled(busy)
    }
    private func binding(_ i: Int) -> Binding<String> {
        Binding(get: { words.indices.contains(i) ? words[i] : "" }, set: { value in
            let parts = Self.split(value)
            if parts.count > 1 { fill(parts, from: i); return }
            if words.indices.contains(i) { words[i] = value.lowercased().filter { $0.isLetter } }
        })
    }
    static func split(_ text: String) -> [String] {
        var body = text.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if body.hasPrefix("dropbeamrecover1:") { body.removeFirst("dropbeamrecover1:".count) }
        return body.split(whereSeparator: { !($0.isASCII && $0.isLetter) }).map(String.init)
    }
    private func fill(_ list: [String], from: Int) {
        if from == 0 && list.count > 12 { count = 24 }
        var next = (0..<count).map { words.indices.contains($0) ? words[$0] : "" }
        for (k, w) in list.prefix(count - from).enumerated() { next[from + k] = w }
        words = next
        focus = min(from + list.count, count - 1)
    }
    private func restore() {
        guard check?.valid == true, !busy else { return }
        busy = true; error = nil; focus = nil
        let entered = text
        Task {
            defer { busy = false }
            do {
                try await bridge.recoveryRestore(entered)
                words = Array(repeating: "", count: count)
                Haptics.success()
                done = true
            } catch { self.error = error.localizedDescription; Haptics.warning() }
        }
    }
}

/// Settings → Devices: the recovery code row, and after a restore what came back.
struct RecoverySection: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var status: RecoveryStatus?
    @State private var saving = false
    @State private var removing: [String]?
    var body: some View {
        Group {
            if let status {
                Section {
                    Button { saving = true; Haptics.tap() } label: {
                        VStack(alignment: .leading, spacing: 3) {
                            Label(status.saved ? "Show Recovery Code" : "Save Your Recovery Code", systemImage: "key.horizontal")
                            Text(status.saved ? "Saved. Keep the paper somewhere safe." : "Not saved yet. If you lose all your devices, it’s the only way to get your friends and chats back.")
                                .font(.footnote).foregroundStyle(status.saved ? Color.secondary : Color.orange)
                        }
                    }
                } header: { Text("Recovery Code") }
                if let r = status.restore {
                    Section {
                        Text(r.summary).font(.subheadline)
                        ForEach(r.oldDevices) { d in
                            VStack(alignment: .leading, spacing: 6) {
                                Text(d.title).font(.body.weight(.semibold))
                                Text("\(d.via) still knew it. Lost or stolen? Remove it so nobody can use it as you.").font(.subheadline).foregroundStyle(.secondary)
                                HStack(spacing: 10) {
                                    Button("Remove", role: .destructive) { removing = [d.endpointId] }.buttonStyle(.borderedProminent).tint(.red)
                                    Button("I Still Have It") { keep(d.endpointId) }.buttonStyle(.bordered)
                                }.controlSize(.small)
                            }.padding(.vertical, 2)
                        }
                        if r.oldDevices.count > 1 {
                            Button("Remove All Old Devices", role: .destructive) { removing = r.oldDevices.map(\.endpointId) }
                        }
                    } header: { Text("Since You Restored") } footer: {
                        if !r.folders.isEmpty {
                            Text("Shared folders you were in: \(r.folders.map { "\($0.name) (with \($0.with))" }.joined(separator: ", ")). Their files are still with your friends — ask them to invite you again.")
                        }
                    }
                }
            }
        }
        .task { await refresh() }
        .onChange(of: bridge.friends.count) { _, _ in Task { await refresh() } }
        .sheet(isPresented: $saving, onDismiss: { Task { await refresh() } }) { SaveRecoverySheet().environmentObject(bridge) }
        .confirmationDialog((removing?.count ?? 0) > 1 ? "Remove your old devices?" : "Remove this old device?",
                            isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }), titleVisibility: .visible, presenting: removing) { ids in
            Button("Remove", role: .destructive) {
                bridge.perform { try await bridge.recoveryRemoveOldDevices(ids); bridge.showToast(ids.count == 1 ? "The old device was removed" : "The old devices were removed"); await refresh() }
            }
        } message: { _ in Text("Your friends will stop treating it as you, and it gets no new messages. If you find it later, you can link it again.") }
    }
    private func refresh() async { if let s = try? await bridge.recoveryStatus() { status = s } }
    private func keep(_ id: String) { Task { try? await bridge.recoveryKeepOldDevice(id); await refresh() } }
}
