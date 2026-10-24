import Foundation
import AVFoundation
import AudioToolbox
import CoreAudio
import Darwin

private struct HelperError: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}

private func checked(_ status: OSStatus, _ operation: String) throws {
    guard status == noErr else { throw HelperError("\(operation): OSStatus \(status)") }
}

private func systemError(_ operation: String) -> HelperError {
    HelperError("\(operation): \(String(cString: strerror(errno)))")
}

private func diagnostic(_ message: String) {
    try? FileHandle.standardError.write(contentsOf: Data((message + "\n").utf8))
}

private func writeAll(_ fd: Int32, _ data: Data) throws {
    try data.withUnsafeBytes { bytes in
        var written = 0
        while written < bytes.count {
            let count = Darwin.write(fd, bytes.baseAddress!.advanced(by: written), bytes.count - written)
            if count < 0 && errno == EINTR { continue }
            guard count > 0 else { throw systemError("write") }
            written += count
        }
    }
}

private func emit(_ object: [String: Any]) throws {
    var data = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
    data.append(10)
    try writeAll(STDOUT_FILENO, data)
}

// Walk directory descriptors: neither the output directory nor an ancestor may be a symlink.
private func openDirectory(_ path: String) throws -> Int32 {
    guard !path.isEmpty else { throw HelperError("An output directory is required") }
    var fd = Darwin.open(path.hasPrefix("/") ? "/" : ".", O_RDONLY | O_DIRECTORY | O_CLOEXEC)
    guard fd >= 0 else { throw systemError("open directory") }
    do {
        for component in path.split(separator: "/") where component != "." {
            let next = openat(fd, String(component), O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
            guard next >= 0 else { throw systemError("open directory component \(component)") }
            Darwin.close(fd)
            fd = next
        }
        return fd
    } catch {
        Darwin.close(fd)
        throw error
    }
}

// Core Audio writes through the exclusively-created descriptor, never reopening a pathname.
private final class CAFFile {
    let fd: Int32
    private var audioFile: AudioFileID?
    private var extendedFile: ExtAudioFileRef?

    init(fd: Int32, format: AVAudioFormat) throws {
        self.fd = fd
        guard let diskFormat = AVAudioFormat(commonFormat: .pcmFormatFloat32,
                                             sampleRate: format.sampleRate,
                                             channels: format.channelCount, interleaved: true) else {
            throw HelperError("Unsupported microphone audio format")
        }
        let context = Unmanaged.passUnretained(self).toOpaque()
        do {
            try checked(AudioFileInitializeWithCallbacks(context, { context, position, count, buffer, actual in
                let fd = Unmanaged<CAFFile>.fromOpaque(context).takeUnretainedValue().fd
                var size: Int
                repeat { size = pread(fd, buffer, Int(count), position) } while size < 0 && errno == EINTR
                actual.pointee = UInt32(max(0, size))
                return size < 0 ? OSStatus(errno) : noErr
            }, { context, position, count, buffer, actual in
                let fd = Unmanaged<CAFFile>.fromOpaque(context).takeUnretainedValue().fd
                var written = 0
                while written < Int(count) {
                    let size = pwrite(fd, buffer.advanced(by: written), Int(count) - written,
                                      position + Int64(written))
                    if size < 0 && errno == EINTR { continue }
                    if size <= 0 {
                        actual.pointee = UInt32(written)
                        return OSStatus(size < 0 ? errno : EIO)
                    }
                    written += size
                }
                actual.pointee = UInt32(written)
                return noErr
            }, { context in
                let fd = Unmanaged<CAFFile>.fromOpaque(context).takeUnretainedValue().fd
                var info = stat()
                return fstat(fd, &info) == 0 ? info.st_size : -1
            }, { context, size in
                let fd = Unmanaged<CAFFile>.fromOpaque(context).takeUnretainedValue().fd
                return ftruncate(fd, size) == 0 ? noErr : OSStatus(errno)
            }, kAudioFileCAFType, diskFormat.streamDescription, AudioFileFlags(rawValue: 0), &audioFile),
                        "create PCM CAF")
            try checked(ExtAudioFileWrapAudioFileID(audioFile!, true, &extendedFile), "open PCM writer")
            try checked(ExtAudioFileSetProperty(extendedFile!, kExtAudioFileProperty_ClientDataFormat,
                                               UInt32(MemoryLayout<AudioStreamBasicDescription>.size),
                                               format.streamDescription), "set microphone PCM format")
            var deferUpdates: UInt32 = 0
            try checked(AudioFileSetProperty(audioFile!, kAudioFilePropertyDeferSizeUpdates,
                                             UInt32(MemoryLayout<UInt32>.size), &deferUpdates),
                        "enable recoverable CAF size updates")
        } catch {
            try? finish()
            throw error
        }
    }

    func append(_ buffer: AVAudioPCMBuffer) throws {
        guard let extendedFile else { throw HelperError("Audio file is already closed") }
        try checked(ExtAudioFileWrite(extendedFile, buffer.frameLength, buffer.audioBufferList), "write PCM audio")
    }

    func finish() throws {
        var failure: Error?
        if let extendedFile {
            self.extendedFile = nil
            do { try checked(ExtAudioFileDispose(extendedFile), "finalize PCM writer") } catch { failure = error }
        }
        if let audioFile {
            self.audioFile = nil
            do { try checked(AudioFileClose(audioFile), "finalize CAF") } catch { if failure == nil { failure = error } }
        }
        if fsync(fd) != 0 && failure == nil { failure = systemError("sync audio.caf") }
        if let failure { throw failure }
    }

    deinit { if audioFile != nil { try? finish() } }
}

private final class Recording: @unchecked Sendable {
    private let lock = NSLock()
    private let audioFD: Int32
    private let timelineFD: Int32
    private var file: CAFFile?
    private var format: AVAudioFormat?
    private var generation: UInt = 0
    private var sampleRate = 48_000.0
    private var frames: Int64 = 0
    private var nextSample: AVAudioFramePosition?
    private var failure: Error?
    private var stopping = false
    private var finished = false

    init(directory: String) throws {
        let directoryFD = try openDirectory(directory)
        defer { Darwin.close(directoryFD) }
        let audio = openat(directoryFD, "audio.caf", O_CREAT | O_EXCL | O_NOFOLLOW | O_RDWR | O_CLOEXEC, 0o600)
        guard audio >= 0 else { throw systemError("create audio.caf (existing files are never overwritten)") }
        let timeline = openat(directoryFD, "timeline.jsonl", O_CREAT | O_EXCL | O_NOFOLLOW | O_WRONLY | O_CLOEXEC, 0o600)
        guard timeline >= 0 else {
            let error = systemError("create timeline.jsonl (existing files are never overwritten)")
            Darwin.close(audio)
            throw error
        }
        guard fsync(directoryFD) == 0 else {
            let error = systemError("sync recording directory")
            Darwin.close(audio)
            Darwin.close(timeline)
            throw error
        }
        audioFD = audio
        timelineFD = timeline
    }

    func configure(_ format: AVAudioFormat) throws {
        lock.lock(); defer { lock.unlock() }
        guard file == nil && !finished else { throw HelperError("Recording already configured") }
        file = try CAFFile(fd: audioFD, format: format)
        self.format = format
        sampleRate = format.sampleRate
    }

    var hasAudio: Bool {
        lock.lock(); defer { lock.unlock() }
        return frames > 0
    }

    func prepareStartupRetry() throws -> UInt {
        lock.lock(); defer { lock.unlock() }
        if let failure { throw failure }
        guard !stopping, !finished, frames == 0 else {
            throw HelperError("Microphone engine stopped after capture began; recording stopped to preserve its timeline")
        }
        // Serialize retry authorization with writes. Late buffers from a removed tap must not
        // join the next attempt, even if engine.stop() returned before their callback completed.
        generation += 1
        return generation
    }

    func append(_ buffer: AVAudioPCMBuffer, at time: AVAudioTime, generation: UInt = 0) {
        lock.lock(); defer { lock.unlock() }
        guard failure == nil, !finished else { return }
        do {
            guard buffer.frameLength > 0 else { return }
            guard generation == self.generation else {
                throw HelperError("Audio arrived across a startup restart; recording stopped to preserve its timeline")
            }
            guard let file, let format else { throw HelperError("Audio arrived before file setup") }
            guard buffer.format.isEqual(format) else {
                throw HelperError("Microphone audio format changed; recording stopped to preserve its timeline")
            }
            if time.isSampleTimeValid {
                if let nextSample, time.sampleTime != nextSample {
                    throw HelperError("Microphone dropped audio frames; recording stopped to preserve its timeline")
                }
                nextSample = time.sampleTime + Int64(buffer.frameLength)
            }
            try file.append(buffer)
            frames += Int64(buffer.frameLength)
        } catch { failure = error }
    }

    func mark(_ note: Data) throws {
        let seconds = try { () throws -> Double in
            lock.lock(); defer { lock.unlock() }
            if let failure { throw failure }
            guard !finished else { throw HelperError("Cannot mark a finalized recording") }
            return Double(frames) / sampleRate
        }()
        // The command reader serializes marks/finalization; disk sync must not hold the audio callback's lock.
        // Keep the original JSON bytes, including integer precision and nested object values.
        var line = Data("{\"seconds\":\(seconds),\"note\":".utf8)
        line.append(note)
        line.append(contentsOf: [125, 10])
        try writeAll(timelineFD, line)
        guard fsync(timelineFD) == 0 else { throw systemError("sync timeline.jsonl") }
    }

    func stop() { lock.lock(); stopping = true; lock.unlock() }
    func shouldStop() throws -> Bool {
        lock.lock(); defer { lock.unlock() }
        if let failure { throw failure }
        return stopping
    }

    func finish() throws -> Double {
        lock.lock(); defer { lock.unlock() }
        if !finished {
            // EOF/denied permission before engine setup still leaves a readable, empty CAF.
            if file == nil {
                file = try CAFFile(fd: audioFD, format: AVAudioFormat(standardFormatWithSampleRate: sampleRate, channels: 1)!)
            }
            var finalizationError: Error?
            do { try file?.finish() } catch { finalizationError = error }
            if fsync(timelineFD) != 0 && finalizationError == nil { finalizationError = systemError("sync timeline.jsonl") }
            finished = true
            if let finalizationError { throw finalizationError }
        }
        if let failure { throw failure }
        return Double(frames) / sampleRate
    }

    deinit {
        if !finished { _ = try? finish() }
        Darwin.close(audioFD)
        Darwin.close(timelineFD)
    }
}

private enum Command {
    case mark(Data)
    case stop
}

private func parseCommand(_ line: Data) throws -> Command {
    guard let object = try JSONSerialization.jsonObject(with: line) as? [String: Any],
          let command = object["command"] as? String else {
        throw HelperError("stdin command must be a JSON object with a command string")
    }
    if command == "stop" { return .stop }
    guard command == "mark", let note = object["note"], note is [String: Any] || note is NSNull else {
        throw HelperError("Expected {\"command\":\"mark\",\"note\":object-or-null} or {\"command\":\"stop\"}")
    }
    // The whole line is validated above. Locate top-level values without decoding/re-encoding note numbers.
    let bytes = Array(line)
    var depth = 0, start = 0, colon: Int?
    var quoted = false, escaped = false
    var noteBytes: Data?
    for (index, byte) in bytes.enumerated() {
        if quoted {
            if escaped { escaped = false }
            else if byte == 92 { escaped = true }
            else if byte == 34 { quoted = false }
            continue
        }
        if byte == 34 { quoted = true; continue }
        if byte == 123 || byte == 91 {
            depth += 1
            if depth == 1 { start = index + 1 }
        } else if byte == 58 && depth == 1 {
            colon = index
        } else if (byte == 44 && depth == 1) || (byte == 125 && depth == 1) {
            if let colon {
                let key = try JSONSerialization.jsonObject(with: Data(bytes[start..<colon]), options: [.fragmentsAllowed]) as? String
                if key == "note" { noteBytes = Data(bytes[(colon + 1)..<index]) }
            }
            start = index + 1
            colon = nil
            if byte == 125 { depth -= 1 }
        } else if byte == 125 || byte == 93 { depth -= 1 }
    }
    guard let noteBytes else { throw HelperError("Missing note value") }
    return .mark(noteBytes)
}

private final class CommandReader {
    private var pending = Data()
    private let fd: Int32
    init(fd: Int32 = STDIN_FILENO) { self.fd = fd }

    @discardableResult
    func pollCommands(_ recording: Recording, timeout: Int32) throws -> Bool {
        var descriptor = pollfd(fd: fd, events: Int16(POLLIN), revents: 0)
        let result = Darwin.poll(&descriptor, 1, timeout)
        if result < 0 && errno == EINTR { return false }
        guard result >= 0 else { throw systemError("poll stdin") }
        if result == 0 { return false }
        if descriptor.revents & Int16(POLLERR | POLLNVAL) != 0 { throw HelperError("stdin pipe failed") }
        var bytes = [UInt8](repeating: 0, count: 16_384)
        let count = Darwin.read(fd, &bytes, bytes.count)
        if count < 0 && errno == EINTR { return false }
        guard count >= 0 else { throw systemError("read stdin") }
        if count > 0 { pending.append(contentsOf: bytes.prefix(count)) }
        while let newline = pending.firstIndex(of: 10) {
            let line = Data(pending[..<newline])
            pending.removeSubrange(...newline)
            try consume(line, recording)
            if try recording.shouldStop() { return true }
        }
        if count == 0 {
            if !pending.isEmpty { try consume(pending, recording); pending.removeAll() }
            recording.stop()
        }
        return true
    }

    private func consume(_ line: Data, _ recording: Recording) throws {
        if line.allSatisfy({ $0 == 32 || $0 == 9 || $0 == 13 }) { return }
        switch try parseCommand(line) {
        case .mark(let note): try recording.mark(note)
        case .stop: recording.stop()
        }
    }
}

private final class PermissionReply: @unchecked Sendable {
    private let lock = NSLock()
    private var granted: Bool?
    func set(_ value: Bool) { lock.lock(); granted = value; lock.unlock() }
    func get() -> Bool? { lock.lock(); defer { lock.unlock() }; return granted }
}

private func builtInMicrophone() throws -> AudioDeviceID {
    var address = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyDevices,
                                             mScope: kAudioObjectPropertyScopeGlobal,
                                             mElement: kAudioObjectPropertyElementMain)
    var size: UInt32 = 0
    try checked(AudioObjectGetPropertyDataSize(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size), "list audio devices")
    var devices = [AudioDeviceID](repeating: 0, count: Int(size) / MemoryLayout<AudioDeviceID>.size)
    try checked(AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &devices), "read audio devices")
    for device in devices {
        address.mSelector = kAudioDevicePropertyTransportType
        var transport: UInt32 = 0
        size = UInt32(MemoryLayout<UInt32>.size)
        try checked(AudioObjectGetPropertyData(device, &address, 0, nil, &size, &transport), "read audio transport")
        guard transport == kAudioDeviceTransportTypeBuiltIn else { continue }
        address.mSelector = kAudioDevicePropertyStreamConfiguration
        address.mScope = kAudioDevicePropertyScopeInput
        defer { address.mScope = kAudioObjectPropertyScopeGlobal }
        try checked(AudioObjectGetPropertyDataSize(device, &address, 0, nil, &size), "read built-in input channels")
        let storage = UnsafeMutableRawPointer.allocate(byteCount: Int(size), alignment: MemoryLayout<AudioBufferList>.alignment)
        defer { storage.deallocate() }
        try checked(AudioObjectGetPropertyData(device, &address, 0, nil, &size, storage), "read built-in input format")
        let buffers = UnsafeMutableAudioBufferListPointer(storage.assumingMemoryBound(to: AudioBufferList.self))
        if buffers.contains(where: { $0.mNumberChannels > 0 }) { return device }
    }
    throw HelperError("No built-in microphone found; refusing to use an external or default microphone")
}

private func capture(_ recording: Recording, reader: CommandReader) throws {
    while try reader.pollCommands(recording, timeout: 0) {
        if try recording.shouldStop() { return }
    }
    switch AVCaptureDevice.authorizationStatus(for: .audio) {
    case .authorized: break
    case .notDetermined:
        let reply = PermissionReply()
        AVCaptureDevice.requestAccess(for: .audio) { reply.set($0) }
        while reply.get() == nil {
            try reader.pollCommands(recording, timeout: 50)
            if try recording.shouldStop() { return }
        }
        guard reply.get() == true else { throw HelperError("Microphone access was denied") }
    default:
        throw HelperError("Microphone access is denied or restricted; enable it in System Settings > Privacy & Security > Microphone")
    }
    if try recording.shouldStop() { return }
    var device = try builtInMicrophone()
    let engine = AVAudioEngine()
    let input = engine.inputNode
    guard let unit = input.audioUnit else { throw HelperError("Input audio unit is unavailable") }
    try checked(AudioUnitSetProperty(unit, kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global, 0,
                                     &device, UInt32(MemoryLayout<AudioDeviceID>.size)), "select built-in microphone")
    var selected: AudioDeviceID = 0
    var size = UInt32(MemoryLayout<AudioDeviceID>.size)
    try checked(AudioUnitGetProperty(unit, kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global, 0,
                                     &selected, &size), "verify microphone selection")
    guard selected == device else { throw HelperError("Audio unit did not select the built-in microphone") }
    // Selecting a device updates the hardware format, but the output bus can retain the
    // previous device's format. installTap applies this hardware format to the output bus.
    let format = input.inputFormat(forBus: 0)
    guard format.sampleRate > 0, format.channelCount > 0 else { throw HelperError("Built-in microphone has no input format") }
    try recording.configure(format)
    var tapInstalled = false
    func installTap(generation: UInt) {
        input.installTap(onBus: 0, bufferSize: 4096, format: format) { buffer, time in
            recording.append(buffer, at: time, generation: generation)
        }
        tapInstalled = true
    }
    func verifyInput() throws {
        try checked(AudioUnitGetProperty(unit, kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global, 0,
                                         &selected, &size), "verify active microphone")
        guard selected == device else { throw HelperError("Selected microphone changed; recording stopped to preserve its timeline") }
        guard input.inputFormat(forBus: 0).isEqual(format), input.outputFormat(forBus: 0).isEqual(format) else {
            throw HelperError("Microphone input format changed; recording stopped to preserve its timeline")
        }
    }
    defer {
        engine.stop()
        if tapInstalled { input.removeTap(onBus: 0) }
    }
    installTap(generation: 0)
    while try reader.pollCommands(recording, timeout: 0) {
        if try recording.shouldStop() { return }
    }
    let startupDeadline = ProcessInfo.processInfo.systemUptime + 5.0
    var retries = 0
    var ready = false
    try engine.start()
    while !(try recording.shouldStop()) {
        // Configuration notifications can describe startup/output changes, not lost input.
        // Inspect actual state on this thread instead of failing from the engine's notification queue.
        try verifyInput()
        if !engine.isRunning {
            engine.stop()
            input.removeTap(onBus: 0)
            tapInstalled = false
            let generation = try recording.prepareStartupRetry()
            guard retries < 2, ProcessInfo.processInfo.systemUptime < startupDeadline else {
                throw HelperError("Microphone did not stabilize during startup; please retry the meeting")
            }
            retries += 1
            try reader.pollCommands(recording, timeout: 50)
            if try recording.shouldStop() { return }
            try verifyInput()
            installTap(generation: generation)
            try engine.start()
            continue
        }
        if !ready {
            if recording.hasAudio {
                try emit(["event": "ready"])
                ready = true
            } else if ProcessInfo.processInfo.systemUptime >= startupDeadline {
                throw HelperError("Microphone started but delivered no audio within five seconds")
            }
        }
        try reader.pollCommands(recording, timeout: 50)
    }
}

private func record(_ directory: String) throws {
    let recording = try Recording(directory: directory)
    let reader = CommandReader()
    let signals = [SIGINT, SIGTERM].map { number -> DispatchSourceSignal in
        Darwin.signal(number, SIG_IGN)
        let source = DispatchSource.makeSignalSource(signal: number, queue: .global())
        source.setEventHandler { recording.stop() }
        source.resume()
        return source
    }
    defer { for source in signals { source.cancel() } }
    do {
        try capture(recording, reader: reader)
        let duration = try recording.finish()
        try emit(["event": "stopped", "seconds": duration])
    } catch {
        // Finalize once; a capture failure retained by Recording must not be reported twice.
        _ = try recording.finish()
        throw error
    }
}

// Synthesized PCM only: never constructs an input engine or requests permissions.
private func selfTest() throws {
    guard let resolved = realpath(FileManager.default.temporaryDirectory.path, nil) else {
        throw systemError("resolve temporary directory")
    }
    defer { free(resolved) }
    let root = URL(fileURLWithPath: String(cString: resolved)).appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false)
    defer { try? FileManager.default.removeItem(at: root) }
    func expect(_ condition: Bool, _ message: String) throws {
        guard condition else { throw HelperError("self-test: \(message)") }
    }
    let recording = try Recording(directory: root.path)
    let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 1)!
    try recording.configure(format)
    try expect(!recording.hasAudio, "readiness requires written PCM")
    let generation = try recording.prepareStartupRetry()
    let command = Data("{\"command\":\"mark\",\"note\":{\"text\":\"Original note\"}}".utf8)
    guard case .mark(let note) = try parseCommand(command) else { throw HelperError("invalid mark") }
    try recording.mark(note)
    let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 4800)!
    buffer.frameLength = 4800
    buffer.floatChannelData![0].initialize(repeating: 0, count: 4800)
    recording.append(buffer, at: AVAudioTime(sampleTime: 0, atRate: 48_000), generation: generation)
    try expect(recording.hasAudio, "PCM makes recording ready")
    try recording.mark(Data("null".utf8))
    recording.append(buffer, at: AVAudioTime(sampleTime: 4800, atRate: 48_000), generation: generation)
    let growing = try AVAudioFile(forReading: root.appendingPathComponent("audio.caf"))
    try expect(growing.length == 9600, "CAF must be recoverable before stopping")
    var restartBlocked = false
    do { _ = try recording.prepareStartupRetry() } catch { restartBlocked = true }
    try expect(restartBlocked, "cannot restart after committing PCM")
    try expect(abs(try recording.finish() - 0.2) < 0.000001, "duration follows sample clock")
    let timeline = try String(contentsOf: root.appendingPathComponent("timeline.jsonl"), encoding: .utf8)
    let markers = try timeline.split(separator: "\n").map {
        try JSONSerialization.jsonObject(with: Data($0.utf8)) as! [String: Any]
    }
    try expect(markers.count == 2, "one marker per presentation change")
    try expect(markers[0]["seconds"] as? Double == 0, "first marker at zero")
    try expect(markers[1]["seconds"] as? Double == 0.1, "marker follows written samples")
    try expect(timeline.contains("Original note"), "marker keeps original note")
    var overwriteBlocked = false
    do { _ = try Recording(directory: root.path) } catch { overwriteBlocked = true }
    try expect(overwriteBlocked, "existing recording cannot be overwritten")
    diagnostic("self-test passed: PCM recovery, readiness, retry safety and sample-time markers")
}

@main
private struct PostgameAudio {
    static func main() {
        Darwin.signal(SIGPIPE, SIG_IGN)
        do {
            let args = Array(CommandLine.arguments.dropFirst())
            guard let command = args.first else { throw usage }
            switch command {
            case "record" where args.count == 2:
                try record(args[1])
            case "self-test" where args.count == 1:
                try selfTest()
            default: throw usage
            }
        } catch {
            diagnostic("postgame-audio: \(error)")
            exit(1)
        }
    }

    private static let usage = HelperError("Usage: helper record DIRECTORY | self-test")
}
