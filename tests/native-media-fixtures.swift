// Offline simulator harness: compile with all NativeUIPlugin Swift sources except
// NativeUIPlugin.swift. This is not part of the shipped package. Arguments: 0, 1,
// 14, long, grid1, grid2, grid3, grid6, thread, dark. Uses the actual shell views.
import SwiftUI
import AVFoundation

@main struct NativeMediaFixtures: App {
    @StateObject private var bridge = Bridge.shared
    @State private var text = CommandLine.arguments.contains("long") ? String(repeating: "A longer message grows naturally to six lines. ", count: 12) : ""
    @State private var reply: ChatMessage?
    @State private var editing: ChatMessage?
    @State private var ready = false
    @State private var fixtureMessage: ChatMessage?
    var body: some Scene {
        WindowGroup {
            NavigationStack {
                Group {
                    if CommandLine.arguments.contains("thread") {
                        ConversationView(friendID: "fixture")
                    } else {
                        VStack {
                            if let fixtureMessage { ChatBubble(message: fixtureMessage, lastInRun: true, query: "", onReply: {}, onEdit: {}).frame(maxWidth: 260) }
                            Spacer()
                            Text(ready ? "Fixture ready" : "Preparing fixtures").accessibilityIdentifier("fixture-ready")
                        }.padding().navigationTitle("Media QA")
                        .safeAreaInset(edge: .bottom, spacing: 0) {
                            ChatComposer(friendID: "fixture", reply: $reply, editing: $editing, text: $text, didSend: {})
                        }
                    }
                }.beamCanvas()
            }.environmentObject(bridge)
                .preferredColorScheme(CommandLine.arguments.contains("dark") ? .dark : .light)
                .task { await prepare() }
        }
    }
    @MainActor private func prepare() async {
        guard !ready else { return }
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("media-fixture")
        try! FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let paths = await Task.detached { () -> [String] in
            var paths: [String] = []
            for i in 0..<12 {
                let url = root.appendingPathComponent("photo-\(i).jpg")
                let format = UIGraphicsImageRendererFormat(); format.scale = 1
                let renderer = UIGraphicsImageRenderer(size: CGSize(width: 2400, height: 1800), format: format)
                let image = renderer.image { context in
                    UIColor(hue: CGFloat(i) / 12, saturation: 0.6, brightness: 0.85, alpha: 1).setFill()
                    context.fill(CGRect(x: 0, y: 0, width: 2400, height: 1800))
                    ("Photo \(i + 1)" as NSString).draw(at: CGPoint(x: 700, y: 740), withAttributes: [.font: UIFont.systemFont(ofSize: 200), .foregroundColor: UIColor.white])
                }
                try! image.jpegData(compressionQuality: 0.8)!.write(to: url)
                paths.append(url.path)
            }
            for i in 0..<2 {
                let url = root.appendingPathComponent("video-\(i).mov")
                try? FileManager.default.removeItem(at: url)
                let writer = try! AVAssetWriter(outputURL: url, fileType: .mov)
                let input = AVAssetWriterInput(mediaType: .video, outputSettings: [AVVideoCodecKey: AVVideoCodecType.h264, AVVideoWidthKey: 320, AVVideoHeightKey: 240])
                let adaptor = AVAssetWriterInputPixelBufferAdaptor(assetWriterInput: input, sourcePixelBufferAttributes: [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32ARGB, kCVPixelBufferWidthKey as String: 320, kCVPixelBufferHeightKey as String: 240])
                writer.add(input); writer.startWriting(); writer.startSession(atSourceTime: .zero)
                var buffer: CVPixelBuffer?
                CVPixelBufferPoolCreatePixelBuffer(nil, adaptor.pixelBufferPool!, &buffer)
                CVPixelBufferLockBaseAddress(buffer!, [])
                memset(CVPixelBufferGetBaseAddress(buffer!), 80, CVPixelBufferGetDataSize(buffer!))
                CVPixelBufferUnlockBaseAddress(buffer!, [])
                for frame in 0..<30 {
                    while !input.isReadyForMoreMediaData { try? await Task.sleep(for: .milliseconds(10)) }
                    adaptor.append(buffer!, withPresentationTime: CMTime(value: Int64(frame), timescale: 30))
                }
                input.markAsFinished(); await writer.finishWriting()
                precondition(writer.status == .completed)
                paths.append(url.path)
            }
            return paths
        }.value
        // Unit-style runtime checks cover the owner's 12 photos + 2 videos,
        // downsampling, cached identity and duration extraction off main.
        for path in paths {
            let first = await ThumbnailProvider.shared.image(path: path, points: 72)
            precondition(first != nil)
            precondition(max(first!.image.cgImage!.width, first!.image.cgImage!.height) <= 144)
            let cached = await ThumbnailProvider.shared.image(path: path, points: 72)
            precondition(first === cached)
            if LocalMedia(path: path)?.video == true { precondition(first!.duration! > 0) }
        }
        fixtureLog("PASS: 14 media thumbnails, 144px bound, cache identity, 2 video durations")
        try! bridge.update(key: "friends", value: [["id": "fixture", "name": "Linux Box", "avatar": paths[0]]])
        bridge.chatPath = ["fixture"]
        let count = CommandLine.arguments.contains("0") ? 0 : CommandLine.arguments.contains("1") ? 1 : 14
        bridge.chatDraftFiles = Array(paths.prefix(count))
        let gridCount = CommandLine.arguments.first(where: { $0.hasPrefix("grid") }).flatMap { Int($0.dropFirst(4)) } ?? 3
        // Put a real video in the grid, plus a non-media file below its caption.
        let chosen = [paths[12]] + Array(paths.prefix(max(0, gridCount - 1)))
        let payload: [String: Any] = ["id": "media", "peerId": "fixture", "fromMe": false, "ts": Date().timeIntervalSince1970 * 1000, "kind": "file", "text": "A few moments from today", "files": chosen.map { URL(fileURLWithPath: $0).lastPathComponent } + ["Notes.pdf"], "fileXferId": "batch"]
        fixtureMessage = try! JSONDecoder().decode(ChatMessage.self, from: JSONSerialization.data(withJSONObject: payload))
        try! bridge.update(key: "transfers", value: [["id": "batch", "chatOnly": true, "state": "completed", "direction": "receive", "chatTransfer": ["completedPaths": Dictionary(uniqueKeysWithValues: chosen.enumerated().map { ("file:\($0.offset):\(URL(fileURLWithPath: $0.element).lastPathComponent)", $0.element) })]]])
        try! bridge.update(key: "thread", value: ["friendId": "fixture", "messages": [
            ["id": "one", "peerId": "fixture", "fromMe": false, "ts": 1_790_000_000_000, "text": "Hello from Linux", "kind": "text"],
            ["id": "two", "peerId": "fixture", "fromMe": false, "ts": 1_790_000_001_000, "text": "Here are those photos", "kind": "text"],
            payload,
            ["id": "three", "peerId": "fixture", "fromMe": true, "ts": Date().timeIntervalSince1970 * 1000 + 1000, "text": "These look great!", "kind": "text", "status": "read"]
        ]])
        bridge.chatTyping["fixture"] = true
        ready = true
        if CommandLine.arguments.contains("bridge") { await runNativePickerChecks(paths: paths) }
        if CommandLine.arguments.contains("feedback") {
            SuperFeedback.configure(.init(backendURL: URL(string: "http://127.0.0.1:1")!, repo: "fixture", app: "Fixture", trigger: .draggable, captureLogs: false, captureCrashes: false))
            SuperFeedback.start()
            try? await Task.sleep(for: .milliseconds(500))
            SuperFeedback.present()
            try? await Task.sleep(for: .seconds(1))
            let windows = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.flatMap(\.windows)
            let overlay = windows.first { String(describing: type(of: $0)).contains("SFOverlayWindow") }!
            precondition(overlay.hitTest(CGPoint(x: 20, y: 100), with: nil) == nil)
            precondition(overlay.hitTest(CGPoint(x: overlay.bounds.midX, y: overlay.bounds.maxY - 120), with: nil) != nil)
            SuperFeedback.dismiss()
            try? await Task.sleep(for: .seconds(2))
            precondition(overlay.hitTest(CGPoint(x: 20, y: 100), with: nil) == nil)
            precondition(overlay.hitTest(CGPoint(x: overlay.bounds.midX, y: overlay.bounds.maxY - 120), with: nil) == nil)
            fixtureLog("PASS: feedback panel claims inside only; friend rows pass through before/after dismissal")
        }
    }
}

func fixtureLog(_ text: String) { FileHandle.standardOutput.write(Data((text + "\n").utf8)) }
