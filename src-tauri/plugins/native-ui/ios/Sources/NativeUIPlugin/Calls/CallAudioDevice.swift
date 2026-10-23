import AudioToolbox
import AVFoundation
import Foundation
import WebRTC

/// WebRTC's audio device on the iPhone (docs/CALLS.md): the iOS voice-processing
/// I/O unit (echo cancellation, noise suppression, gain — the same unit WebRTC's
/// built-in iOS device uses), 48 kHz mono 16-bit. Owning it lets a recording see
/// both voices: the microphone after echo cancellation ("me") and exactly what
/// plays from the other side ("them"), without a second capture.
///
/// The unit only runs while the audio session is active — CallKit's
/// didActivate/didDeactivate (or the app's own activation without CallKit) —
/// and while WebRTC wants playout or recording.
final class CallAudioDevice: NSObject, RTCAudioDevice {
    static let shared = CallAudioDevice()

    private static let rate = 48_000.0
    private static let maxFrames = 4096

    private let lock = NSLock()
    private var delegate: RTCAudioDeviceDelegate?
    private var unit: AudioUnit?
    private var wantPlayout = false
    private var wantRecording = false
    private var sessionActive = false
    private var running = false
    private var playoutReady = false
    private var recordingReady = false
    /// Microphone samples rendered from the unit (render thread only).
    private let inputBuffer = UnsafeMutablePointer<Int16>.allocate(capacity: maxFrames)
    /// The recorder listening right now (set/cleared on the main thread, read
    /// on the render threads — a single pointer swap).
    private let tapLock = NSLock()
    private var _tap: CallRecorder?
    var tap: CallRecorder? {
        get { tapLock.lock(); defer { tapLock.unlock() }; return _tap }
        set { tapLock.lock(); _tap = newValue; tapLock.unlock() }
    }

    private override init() { super.init() }

    // ── session (CallCenter) ──

    func setSessionActive(_ on: Bool) {
        lock.lock()
        sessionActive = on
        lock.unlock()
        update()
    }

    // ── RTCAudioDevice ──

    var deviceInputSampleRate: Double { Self.rate }
    var inputIOBufferDuration: TimeInterval { AVAudioSession.sharedInstance().ioBufferDuration }
    var inputNumberOfChannels: Int { 1 }
    var inputLatency: TimeInterval { AVAudioSession.sharedInstance().inputLatency }
    var deviceOutputSampleRate: Double { Self.rate }
    var outputIOBufferDuration: TimeInterval { AVAudioSession.sharedInstance().ioBufferDuration }
    var outputNumberOfChannels: Int { 1 }
    var outputLatency: TimeInterval { AVAudioSession.sharedInstance().outputLatency }

    var isInitialized: Bool { lock.lock(); defer { lock.unlock() }; return delegate != nil }

    func initialize(with delegate: RTCAudioDeviceDelegate) -> Bool {
        lock.lock()
        self.delegate = delegate
        lock.unlock()
        return true
    }

    func terminateDevice() -> Bool {
        lock.lock()
        wantPlayout = false
        wantRecording = false
        playoutReady = false
        recordingReady = false
        lock.unlock()
        update()
        lock.lock()
        if let u = unit { AudioUnitUninitialize(u); AudioComponentInstanceDispose(u); unit = nil }
        delegate = nil
        lock.unlock()
        return true
    }

    var isPlayoutInitialized: Bool { lock.lock(); defer { lock.unlock() }; return playoutReady }
    func initializePlayout() -> Bool { lock.lock(); playoutReady = true; lock.unlock(); return makeUnit() }
    var isPlaying: Bool { lock.lock(); defer { lock.unlock() }; return wantPlayout }
    func startPlayout() -> Bool { lock.lock(); wantPlayout = true; lock.unlock(); update(); return true }
    func stopPlayout() -> Bool { lock.lock(); wantPlayout = false; lock.unlock(); update(); return true }

    var isRecordingInitialized: Bool { lock.lock(); defer { lock.unlock() }; return recordingReady }
    func initializeRecording() -> Bool { lock.lock(); recordingReady = true; lock.unlock(); return makeUnit() }
    var isRecording: Bool { lock.lock(); defer { lock.unlock() }; return wantRecording }
    func startRecording() -> Bool { lock.lock(); wantRecording = true; lock.unlock(); update(); return true }
    func stopRecording() -> Bool { lock.lock(); wantRecording = false; lock.unlock(); update(); return true }

    // ── the voice-processing unit ──

    private func makeUnit() -> Bool {
        lock.lock(); defer { lock.unlock() }
        if unit != nil { return true }
        var desc = AudioComponentDescription(componentType: kAudioUnitType_Output, componentSubType: kAudioUnitSubType_VoiceProcessingIO,
                                             componentManufacturer: kAudioUnitManufacturer_Apple, componentFlags: 0, componentFlagsMask: 0)
        guard let comp = AudioComponentFindNext(nil, &desc) else { return false }
        var u: AudioUnit?
        guard AudioComponentInstanceNew(comp, &u) == noErr, let u else { return false }
        var one: UInt32 = 1
        let size1 = UInt32(MemoryLayout<UInt32>.size)
        AudioUnitSetProperty(u, kAudioOutputUnitProperty_EnableIO, kAudioUnitScope_Input, 1, &one, size1)
        AudioUnitSetProperty(u, kAudioOutputUnitProperty_EnableIO, kAudioUnitScope_Output, 0, &one, size1)
        var fmt = AudioStreamBasicDescription(mSampleRate: Self.rate, mFormatID: kAudioFormatLinearPCM,
                                              mFormatFlags: kLinearPCMFormatFlagIsSignedInteger | kLinearPCMFormatFlagIsPacked,
                                              mBytesPerPacket: 2, mFramesPerPacket: 1, mBytesPerFrame: 2, mChannelsPerFrame: 1,
                                              mBitsPerChannel: 16, mReserved: 0)
        let fmtSize = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
        AudioUnitSetProperty(u, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Input, 0, &fmt, fmtSize)
        AudioUnitSetProperty(u, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Output, 1, &fmt, fmtSize)
        let me = Unmanaged.passUnretained(self).toOpaque()
        var input = AURenderCallbackStruct(inputProc: { ref, flags, ts, bus, frames, _ in
            Unmanaged<CallAudioDevice>.fromOpaque(ref).takeUnretainedValue().onInput(flags, ts, bus, frames)
        }, inputProcRefCon: me)
        AudioUnitSetProperty(u, kAudioOutputUnitProperty_SetInputCallback, kAudioUnitScope_Global, 1, &input, UInt32(MemoryLayout<AURenderCallbackStruct>.size))
        var render = AURenderCallbackStruct(inputProc: { ref, flags, ts, bus, frames, io in
            Unmanaged<CallAudioDevice>.fromOpaque(ref).takeUnretainedValue().onOutput(flags, ts, bus, frames, io)
        }, inputProcRefCon: me)
        AudioUnitSetProperty(u, kAudioUnitProperty_SetRenderCallback, kAudioUnitScope_Input, 0, &render, UInt32(MemoryLayout<AURenderCallbackStruct>.size))
        guard AudioUnitInitialize(u) == noErr else { AudioComponentInstanceDispose(u); return false }
        unit = u
        return true
    }

    /// Start or stop the unit to match what WebRTC wants and the session allows.
    private func update() {
        lock.lock()
        let should = sessionActive && (wantPlayout || wantRecording)
        let u = unit
        let changed = should != running
        if changed { running = should }
        lock.unlock()
        guard changed, let u else { return }
        if should { AudioOutputUnitStart(u) } else { AudioOutputUnitStop(u) }
    }

    private func onInput(_ flags: UnsafeMutablePointer<AudioUnitRenderActionFlags>, _ ts: UnsafePointer<AudioTimeStamp>, _ bus: UInt32, _ frames: UInt32) -> OSStatus {
        guard let u = unit, Int(frames) <= Self.maxFrames else { return noErr }
        var list = AudioBufferList(mNumberBuffers: 1, mBuffers: AudioBuffer(mNumberChannels: 1, mDataByteSize: frames * 2, mData: UnsafeMutableRawPointer(inputBuffer)))
        let status = AudioUnitRender(u, flags, ts, bus, frames, &list)
        guard status == noErr else { return status }
        tap?.append(.me, inputBuffer, Int(frames))
        guard wantRecording, let d = delegate else { return noErr }
        return d.deliverRecordedData(flags, ts, Int(bus), frames, &list, nil, nil)
    }

    private func onOutput(_ flags: UnsafeMutablePointer<AudioUnitRenderActionFlags>, _ ts: UnsafePointer<AudioTimeStamp>, _ bus: UInt32, _ frames: UInt32, _ io: UnsafeMutablePointer<AudioBufferList>?) -> OSStatus {
        guard let io else { return noErr }
        let buffers = UnsafeMutableAudioBufferListPointer(io)
        guard wantPlayout, let d = delegate else {
            for b in buffers { if let p = b.mData { memset(p, 0, Int(b.mDataByteSize)) } }
            flags.pointee.insert(.unitRenderAction_OutputIsSilence)
            return noErr
        }
        let status = d.getPlayoutData(flags, ts, Int(bus), frames, io)
        if status == noErr, let p = buffers.first?.mData {
            tap?.append(.them, p.assumingMemoryBound(to: Int16.self), Int(frames))
        }
        return status
    }
}
