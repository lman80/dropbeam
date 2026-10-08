import Foundation
import ImageIO
import UniformTypeIdentifiers
import UIKit

/// The main-app half of the share extension (gen/apple/DropBeamShare).
///
/// Both processes share the App Group container `group.com.ashtonmiller.dropbeam`:
///
///     share-recipients.json     written HERE whenever friends/presence change; the
///     share-avatars/<id>.jpg    extension shows them as the recipient list.
///     ShareJobs/.incoming-<id>/ the extension copies shared items into files/ …
///     ShareJobs/<id>/job.json   … then renames the folder (atomic) once job.json exists.
///
/// The engine can't run in the extension (memory cap, no iroh), so on launch /
/// foreground / a `dropbeam://share` open the app takes every finished job: moves its
/// folder into Application Support/dropbeam-picked (where every other picked file lives),
/// then sends to the chosen friend, starts a Quick Send, or shows the Send To sheet.
///
/// The JSON shapes are duplicated in DropBeamShare/ShareShared.swift — keep them in sync.
@MainActor
final class ShareInbox {
    static let shared = ShareInbox()
    static let appGroup = "group.com.ashtonmiller.dropbeam"

    private let fm = FileManager.default
    private var ingesting = false
    private var again = false
    private var writeTask: Task<Void, Never>?
    private var lastSnapshot: Data?

    private var container: URL? { fm.containerURL(forSecurityApplicationGroupIdentifier: Self.appGroup) }

    // MARK: Recipient snapshot (app → extension)

    struct Recipient: Codable {
        let id: String
        let name: String
        let own: Bool
        let deviceKind: String?
        let deviceOs: String?
        let online: Bool
        /// Relative to the container ("share-avatars/….jpg").
        let avatar: String?
    }
    struct Snapshot: Codable {
        var version = 1
        let updatedMs: Double
        let recipients: [Recipient]
    }

    /// Friends / presence / identity changed: rewrite the snapshot (coalesced).
    func recipientsChanged() {
        writeTask?.cancel()
        writeTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(800))
            guard !Task.isCancelled else { return }
            self?.writeSnapshot()
        }
    }

    private func writeSnapshot() {
        guard let container else { return }
        let bridge = Bridge.shared
        let account = bridge.myDevice?.accountPub ?? ""
        let avatars = container.appendingPathComponent("share-avatars", isDirectory: true)
        try? fm.createDirectory(at: avatars, withIntermediateDirectories: true)
        var keep = Set<String>()
        let people = bridge.friends.filter { $0.groupedUnder == nil }
        let recipients: [Recipient] = people.map { friend in
            let own = friend.ownDevice || (!account.isEmpty && friend.accountPub == account)
            var avatarRel: String?
            if !own, let raw = friend.avatar, !raw.isEmpty {
                let file = Self.safeName(friend.id) + ".jpg"
                if Self.writeAvatar(from: LocalPaths.resolve(raw), to: avatars.appendingPathComponent(file)) {
                    avatarRel = "share-avatars/" + file; keep.insert(file)
                }
            }
            return Recipient(id: friend.id, name: friend.displayName, own: own, deviceKind: friend.deviceKind,
                             deviceOs: friend.deviceOs, online: bridge.presence[friend.id] == true, avatar: avatarRel)
        }
        // Own devices first (like Send To), then friends in the app's order.
        let ordered = recipients.filter(\.own) + recipients.filter { !$0.own }
        // Pictures of removed friends don't linger in the shared container.
        for name in (try? fm.contentsOfDirectory(atPath: avatars.path)) ?? [] where !keep.contains(name) {
            try? fm.removeItem(at: avatars.appendingPathComponent(name))
        }
        let body = try? JSONEncoder().encode(ordered)
        // Presence ticks re-push the same data every 15s; skip identical rewrites but
        // still refresh the timestamp at most once a minute so "online" stays trusted.
        let now = Date().timeIntervalSince1970 * 1000
        if body == lastSnapshot, let old = try? Data(contentsOf: container.appendingPathComponent("share-recipients.json")),
           let prior = try? JSONDecoder().decode(Snapshot.self, from: old), now - prior.updatedMs < 60_000 { return }
        lastSnapshot = body
        guard let data = try? JSONEncoder().encode(Snapshot(updatedMs: now, recipients: ordered)) else { return }
        try? data.write(to: container.appendingPathComponent("share-recipients.json"), options: .atomic)
    }

    nonisolated static func safeName(_ id: String) -> String {
        let chars = id.unicodeScalars.map { CharacterSet.alphanumerics.contains($0) || $0 == "-" ? Character($0) : "_" }
        return String(String(chars).prefix(80))
    }

    /// A small JPEG of a friend's picture (the extension can't read the app's container).
    /// Skips the work when the copy is already newer than the source.
    private static func writeAvatar(from path: String, to dest: URL) -> Bool {
        let fm = FileManager.default
        guard let srcDate = (try? fm.attributesOfItem(atPath: path))?[.modificationDate] as? Date else { return false }
        if let destDate = (try? fm.attributesOfItem(atPath: dest.path))?[.modificationDate] as? Date, destDate >= srcDate { return true }
        guard let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
              let image = CGImageSourceCreateThumbnailAtIndex(source, 0, [
                kCGImageSourceCreateThumbnailFromImageAlways: true,
                kCGImageSourceCreateThumbnailWithTransform: true,
                kCGImageSourceThumbnailMaxPixelSize: 192
              ] as CFDictionary),
              let out = CGImageDestinationCreateWithURL(dest as CFURL, UTType.jpeg.identifier as CFString, 1, nil) else { return false }
        CGImageDestinationAddImage(out, image, [kCGImageDestinationLossyCompressionQuality: 0.85] as CFDictionary)
        return CGImageDestinationFinalize(out)
    }

    // MARK: Jobs (extension → app)

    struct Job: Codable {
        let version: Int
        let id: String
        let createdMs: Double
        /// "friend" | "quick" | "choose"
        let recipient: String
        let friendId: String?
        let friendName: String?
        /// Names inside the job's files/ folder, in the order they were shared.
        let files: [String]
        /// Links / text snippets (sent as chat messages when a friend was chosen).
        let texts: [String]
    }

    /// True for the URL the extension opens us with (`dropbeam://share?job=…`).
    nonisolated static func isShareURL(_ value: String) -> Bool {
        value.lowercased().hasPrefix("dropbeam://share")
    }

    /// Take every finished share job. Safe to call often (launch, every foreground, the
    /// extension's open URL): concurrent calls collapse into one pass plus a re-run.
    func ingestSoon() {
        if ingesting { again = true; return }
        ingesting = true
        Task {
            repeat { again = false; await ingestPending() } while again
            ingesting = false
        }
    }

    private func ingestPending() async {
        guard let container else { return }
        let jobs = container.appendingPathComponent("ShareJobs", isDirectory: true)
        let names = ((try? fm.contentsOfDirectory(atPath: jobs.path)) ?? []).sorted()
        // Abandoned copies (extension killed mid-copy) are dropped after a day.
        for name in names where name.hasPrefix(".incoming-") {
            let url = jobs.appendingPathComponent(name)
            if let date = (try? fm.attributesOfItem(atPath: url.path))?[.modificationDate] as? Date,
               date.timeIntervalSinceNow < -86_400 { try? fm.removeItem(at: url) }
        }
        let ready: [(URL, Job)] = names.filter { !$0.hasPrefix(".") }.compactMap { name in
            let dir = jobs.appendingPathComponent(name, isDirectory: true)
            guard let data = try? Data(contentsOf: dir.appendingPathComponent("job.json")),
                  let job = try? JSONDecoder().decode(Job.self, from: data) else {
                // A folder without a readable manifest can never be sent.
                try? fm.removeItem(at: dir); return nil
            }
            return (dir, job)
        }.sorted { $0.1.createdMs < $1.1.createdMs }
        guard !ready.isEmpty else { return }
        NSLog("DropBeam share inbox: %d job(s) waiting", ready.count)
        // The web store must be up (it owns sending) before anything can go out.
        // Not ready yet: the first settings snapshot re-runs this (Bridge.apply), and so
        // does every return to the foreground — nothing is dropped.
        guard Bridge.shared.webview != nil && Bridge.shared.settings != nil else {
            NSLog("DropBeam share inbox: bridge not ready; will run when the engine is up")
            return
        }
        for (dir, job) in ready { await run(job, from: dir) }
    }

    private func waitUntil(seconds: Double, _ condition: @MainActor () -> Bool) async -> Bool {
        let deadline = Date().addingTimeInterval(seconds)
        while !condition() {
            if Date() > deadline { return false }
            try? await Task.sleep(for: .milliseconds(250))
        }
        return true
    }

    private func run(_ job: Job, from dir: URL) async {
        let bridge = Bridge.shared
        // Claim: move the whole job into the app's own durable picked-files area, so it
        // is ours (the extension may clean its container) and runs exactly once.
        let picked = fm.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("dropbeam-picked", isDirectory: true)
        let claimed = picked.appendingPathComponent("share-" + Self.safeName(job.id), isDirectory: true)
        do {
            try fm.createDirectory(at: picked, withIntermediateDirectories: true)
            if fm.fileExists(atPath: claimed.path) { try fm.removeItem(at: claimed) }
            do { try fm.moveItem(at: dir, to: claimed) }
            catch { try fm.copyItem(at: dir, to: claimed); try? fm.removeItem(at: dir) }
            // Swept like any other copy once sent (and never while still queued).
            PickedMedia.mark(claimed, .send)
        } catch {
            NSLog("DropBeam share inbox: could not claim job: %@", error.localizedDescription)
            try? fm.removeItem(at: dir)
            bridge.errorMessage = "DropBeam couldn’t open what you shared. Please share it again."
            return
        }
        let files = claimed.appendingPathComponent("files", isDirectory: true)
        var paths = job.files.compactMap { name -> String? in
            // Names come from another process: never let one escape the job folder.
            guard !name.isEmpty, !name.contains("/"), name != "..", name != "." else { return nil }
            let url = files.appendingPathComponent(name)
            return fm.fileExists(atPath: url.path) ? url.path : nil
        }
        var texts = job.texts.map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }.filter { !$0.isEmpty }
        NSLog("DropBeam share inbox: job %@ → %@ (%d file(s), %d text(s))", job.id, job.recipient, paths.count, texts.count)

        var friendId: String?
        if job.recipient == "friend", let id = job.friendId {
            // Right after launch the store may still be loading friends.
            if await waitUntil(seconds: 15, { Bridge.shared.friends.contains { $0.id == id } }) { friendId = id }
        }
        guard let friendId else {
            // No friend (Quick Send, "choose", or the friend is gone): text rides along as a file.
            if !texts.isEmpty, let note = Self.writeNote(texts, in: files) { paths.append(note); texts = [] }
            guard !paths.isEmpty else { return }
            bridge.selectedTab = "send"
            if job.recipient == "quick" {
                do { BackgroundTransfers.shared.userStartedSend(paths: paths, to: nil); try await bridge.action("quickSend", ["paths": paths]) }
                catch { bridge.pickedToSend = paths }
            } else {
                bridge.pickedToSend = paths // The Send To sheet lets them pick.
            }
            return
        }
        let name = job.friendName ?? bridge.friends.first { $0.id == friendId }?.displayName ?? "your friend"
        do {
            if !paths.isEmpty {
                try await bridge.sendToFriend(friendId: friendId, paths: paths)
                bridge.selectedTab = "send"
            }
            for text in texts { try await bridge.sendChatText(friendId: friendId, text: text) }
            if paths.isEmpty && !texts.isEmpty { try? await bridge.openChat(friendId: friendId) }
            bridge.showToast(paths.isEmpty ? "Sent to \(name)" : "Sending \(Self.describe(paths.count)) to \(name)")
        } catch {
            NSLog("DropBeam share inbox: send failed: %@", error.localizedDescription)
            // Don't lose it: say why, then offer the normal Send To sheet with the same
            // files. One at a time — presenting the sheet under the alert gets both dismissed.
            bridge.errorMessage = "Couldn’t send to \(name): \(error.localizedDescription)"
            if !paths.isEmpty {
                _ = await waitUntil(seconds: 600) { Bridge.shared.errorMessage == nil }
                bridge.pickedToSend = paths
            }
        }
    }

    private static func describe(_ count: Int) -> String { count == 1 ? "1 item" : "\(count) items" }

    private static func writeNote(_ texts: [String], in dir: URL) -> String? {
        let allLinks = texts.allSatisfy { URL(string: $0)?.scheme?.hasPrefix("http") == true }
        let url = dir.appendingPathComponent(allLinks ? (texts.count == 1 ? "Link.txt" : "Links.txt") : "Note.txt")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        guard (try? texts.joined(separator: "\n\n").write(to: url, atomically: true, encoding: .utf8)) != nil else { return nil }
        return url.path
    }
}
