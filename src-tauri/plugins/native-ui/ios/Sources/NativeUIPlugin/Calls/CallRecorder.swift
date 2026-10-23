import AVFoundation
import Foundation
import os

/// Records a call on the iPhone (docs/CALLS.md): your voice (microphone after
/// echo cancellation), their voice (what plays from the call) and both mixed,
/// each as AAC (.m4a) in the app's temp folder. The audio threads only copy
/// samples into small locked buffers; a background queue mixes and encodes.
/// When it stops, the files are handed to the engine (`call_record_import`),
/// which moves them into Documents/DropBeam Calls/<date> <name>/ beside call.json.
final class CallRecorder {
    enum Track: String, CaseIterable { case me, them, mixed }

    private static let rate = 48_000.0
    private let queue = DispatchQueue(label: "dropbeam.call-recorder")
    private var lock = os_unfair_lock()
    private var pendingMe: [Int16] = []
    private var pendingThem: [Int16] = []
    /// Your voice is silent while you're muted (the call didn't send it either).
    private var _meMuted = false
    var meMuted: Bool {
        get { os_unfair_lock_lock(&lock); defer { os_unfair_lock_unlock(&lock) }; return _meMuted }
        set { os_unfair_lock_lock(&lock); _meMuted = newValue; os_unfair_lock_unlock(&lock) }
    }
    private var files: [Track: AVAudioFile] = [:]
    let urls: [Track: URL]
    private var timer: DispatchSourceTimer?

    init() throws {
        let id = UUID().uuidString.prefix(8)
        var u: [Track: URL] = [:]
        for t in Track.allCases { u[t] = FileManager.default.temporaryDirectory.appendingPathComponent("dropbeam-call-\(id)-\(t.rawValue).m4a") }
        urls = u
        let settings: [String: Any] = [AVFormatIDKey: kAudioFormatMPEG4AAC, AVSampleRateKey: Self.rate, AVNumberOfChannelsKey: 1, AVEncoderBitRateKey: 96_000]
        for t in Track.allCases {
            files[t] = try AVAudioFile(forWriting: u[t]!, settings: settings, commonFormat: .pcmFormatInt16, interleaved: true)
        }
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now() + 0.25, repeating: 0.25)
        timer.setEventHandler { [weak self] in self?.drain(final: false) }
        timer.resume()
        self.timer = timer
    }

    /// Audio thread: copy samples (no encoding here).
    func append(_ track: Track, _ samples: UnsafePointer<Int16>, _ count: Int) {
        guard count > 0 else { return }
        os_unfair_lock_lock(&lock)
        defer { os_unfair_lock_unlock(&lock) }
        switch track {
        case .me:
            if _meMuted { pendingMe.append(contentsOf: repeatElement(0, count: count)) }
            else { pendingMe.append(contentsOf: UnsafeBufferPointer(start: samples, count: count)) }
        case .them: pendingThem.append(contentsOf: UnsafeBufferPointer(start: samples, count: count))
        case .mixed: break
        }
        // Never grow without bound if the writer stalls (≈20 s per voice).
        if pendingMe.count > 960_000 { pendingMe.removeFirst(pendingMe.count - 960_000) }
        if pendingThem.count > 960_000 { pendingThem.removeFirst(pendingThem.count - 960_000) }
    }

    private func write(_ track: Track, _ samples: [Int16]) {
        guard !samples.isEmpty, let file = files[track],
              let buf = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: AVAudioFrameCount(samples.count)),
              let dst = buf.int16ChannelData?[0] else { return }
        samples.withUnsafeBufferPointer { dst.update(from: $0.baseAddress!, count: samples.count) }
        buf.frameLength = AVAudioFrameCount(samples.count)
        try? file.write(from: buf)
    }

    /// Queue: encode what arrived; the mixed track sums the two voices sample by
    /// sample (a voice that falls more than a second behind is padded with silence).
    private func drain(final: Bool) {
        os_unfair_lock_lock(&lock)
        let me = pendingMe, them = pendingThem
        var n = min(me.count, them.count)
        if final || abs(me.count - them.count) > Int(Self.rate) { n = max(me.count, them.count) }
        pendingMe = n < me.count ? Array(me[n...]) : []
        pendingThem = n < them.count ? Array(them[n...]) : []
        os_unfair_lock_unlock(&lock)
        guard n > 0 else { return }
        let a = me.prefix(n), b = them.prefix(n)
        write(.me, Array(a))
        write(.them, Array(b))
        var mixed = [Int16](repeating: 0, count: n)
        for i in 0..<n {
            let x = Int32(i < a.count ? a[a.startIndex + i] : 0) + Int32(i < b.count ? b[b.startIndex + i] : 0)
            mixed[i] = Int16(clamping: x)
        }
        write(.mixed, mixed)
    }

    /// Stop and close the files (AAC is finalized when the file is released).
    /// Returns each track's finished file.
    func finish() async -> [Track: URL] {
        await withCheckedContinuation { (done: CheckedContinuation<Void, Never>) in
            queue.async {
                self.timer?.cancel()
                self.timer = nil
                self.drain(final: true)
                self.files.removeAll()
                done.resume()
            }
        }
        return urls.filter { (try? $0.value.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0) ?? 0 > 0 }
    }

    /// Throw the files away (recording failed to open, or nothing to keep).
    func discard() {
        queue.async {
            self.timer?.cancel()
            self.files.removeAll()
            for u in self.urls.values { try? FileManager.default.removeItem(at: u) }
        }
    }
}
