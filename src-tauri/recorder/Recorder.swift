// dev-inbox-recorder: records a meeting (microphone + system audio, as two files) and
// transcribes audio on-device with Apple's SpeechTranscriber, and summarises transcripts with Apple
// Intelligence's on-device model. Driven by the Dev Inbox app over stdin/stdout; every line it prints
// is one JSON object.
//
//   dev-inbox-recorder record <dir>              -> <dir>/mic.m4a, <dir>/system.m4a; "stop" on stdin ends it
//   dev-inbox-recorder transcribe <file> [locale] -> {"start","end","text"} lines, then {"event":"done"}
//   dev-inbox-recorder summarise <transcript.txt> -> {"event":"summary","overview","decisions","actionItems"}
//   dev-inbox-recorder check [locale]             -> {"speech":bool,"locale":...,"summary":bool,"summaryReason":...}

import AVFAudio
import AVFoundation
import CoreAudio
import Foundation
import FoundationModels
import Speech

// MARK: - Output

/// One JSON line to the app. If the app has gone away the write fails quietly (no SIGPIPE, no exception),
/// so a recording still finishes and closes its files.
func emit(_ object: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: object),
          let line = String(data: data, encoding: .utf8) else { return }
    try? FileHandle.standardOutput.write(contentsOf: Data((line + "\n").utf8))
}

func fail(_ message: String) -> Never {
    emit(["event": "error", "message": message])
    exit(1)
}

/// Compact AAC, mono for the mic and stereo for system audio.
func aacSettings(sampleRate: Double, channels: AVAudioChannelCount) -> [String: Any] {
    [
        AVFormatIDKey: kAudioFormatMPEG4AAC,
        AVSampleRateKey: sampleRate,
        AVNumberOfChannelsKey: channels,
        AVEncoderBitRateKey: channels > 1 ? 64_000 : 32_000,
    ]
}

/// Root-mean-square level of a buffer, 0...1, for the app's level meters.
func level(_ buffer: AVAudioPCMBuffer) -> Float {
    guard let data = buffer.floatChannelData, buffer.frameLength > 0 else { return 0 }
    var sum: Float = 0
    let n = Int(buffer.frameLength)
    for i in 0..<n { sum += data[0][i] * data[0][i] }
    return min(1, sqrt(sum / Float(n)) * 4)
}

// MARK: - Microphone

final class MicRecorder {
    private let engine = AVAudioEngine()
    private var file: AVAudioFile?
    private(set) var peak: Float = 0

    func start(to url: URL) throws {
        let input = engine.inputNode
        let format = input.outputFormat(forBus: 0)
        guard format.sampleRate > 0 else { throw NSError(domain: "mic", code: 1, userInfo: [NSLocalizedDescriptionKey: "No microphone available"]) }
        file = try AVAudioFile(
            forWriting: url,
            settings: aacSettings(sampleRate: format.sampleRate, channels: 1),
            commonFormat: .pcmFormatFloat32,
            interleaved: false
        )
        let mono = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: format.sampleRate, channels: 1, interleaved: false)!
        let converter = AVAudioConverter(from: format, to: mono)
        input.installTap(onBus: 0, bufferSize: 4096, format: format) { [weak self] buffer, _ in
            guard let self, let file = self.file else { return }
            var out = buffer
            if format.channelCount != 1, let converter,
               let converted = AVAudioPCMBuffer(pcmFormat: mono, frameCapacity: buffer.frameCapacity) {
                var fed = false
                converter.convert(to: converted, error: nil) { _, status in
                    if fed { status.pointee = .noDataNow; return nil }
                    fed = true
                    status.pointee = .haveData
                    return buffer
                }
                out = converted
            }
            try? file.write(from: out)
            self.peak = max(self.peak, level(out))
        }
        engine.prepare()
        try engine.start()
    }

    func stop() {
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        file = nil // closes the file
    }

    func takePeak() -> Float {
        defer { peak = 0 }
        return peak
    }
}

// MARK: - System audio (Core Audio process tap)

final class SystemRecorder {
    private var tapID = AudioObjectID(kAudioObjectUnknown)
    private var aggregateID = AudioObjectID(kAudioObjectUnknown)
    private var procID: AudioDeviceIOProcID?
    private var file: AVAudioFile?
    private let queue = DispatchQueue(label: "system-audio")
    private(set) var peak: Float = 0

    private func defaultOutputUID() throws -> String {
        var device = AudioObjectID(kAudioObjectUnknown)
        var size = UInt32(MemoryLayout<AudioObjectID>.size)
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioHardwarePropertyDefaultSystemOutputDevice,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        try check(AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &device), "default output")
        var uid: CFString = "" as CFString
        size = UInt32(MemoryLayout<CFString>.size)
        address.mSelector = kAudioDevicePropertyDeviceUID
        try withUnsafeMutablePointer(to: &uid) { ptr in
            try check(AudioObjectGetPropertyData(device, &address, 0, nil, &size, ptr), "output UID")
        }
        return uid as String
    }

    private func check(_ status: OSStatus, _ what: String) throws {
        if status != noErr {
            throw NSError(domain: "coreaudio", code: Int(status), userInfo: [NSLocalizedDescriptionKey: "\(what) failed (\(status))"])
        }
    }

    func start(to url: URL) throws {
        // Everything the Mac plays (the call), not muted for the user, private to this process.
        let description = CATapDescription(stereoGlobalTapButExcludeProcesses: [])
        description.uuid = UUID()
        description.muteBehavior = .unmuted
        description.isPrivate = true
        description.name = "Dev Inbox meeting audio"
        try check(AudioHardwareCreateProcessTap(description, &tapID), "Creating the audio tap")

        var asbd = AudioStreamBasicDescription()
        var size = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioTapPropertyFormat,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        try check(AudioObjectGetPropertyData(tapID, &address, 0, nil, &size, &asbd), "Reading the tap format")
        guard let format = AVAudioFormat(streamDescription: &asbd) else {
            throw NSError(domain: "coreaudio", code: 2, userInfo: [NSLocalizedDescriptionKey: "Unsupported tap format"])
        }

        let outputUID = try defaultOutputUID()
        let aggregate: [String: Any] = [
            kAudioAggregateDeviceNameKey: "Dev Inbox recorder",
            kAudioAggregateDeviceUIDKey: UUID().uuidString,
            kAudioAggregateDeviceMainSubDeviceKey: outputUID,
            kAudioAggregateDeviceIsPrivateKey: true,
            kAudioAggregateDeviceIsStackedKey: false,
            kAudioAggregateDeviceTapAutoStartKey: true,
            kAudioAggregateDeviceSubDeviceListKey: [[kAudioSubDeviceUIDKey: outputUID]],
            kAudioAggregateDeviceTapListKey: [[kAudioSubTapDriftCompensationKey: true, kAudioSubTapUIDKey: description.uuid.uuidString]],
        ]
        try check(AudioHardwareCreateAggregateDevice(aggregate as CFDictionary, &aggregateID), "Creating the capture device")

        file = try AVAudioFile(
            forWriting: url,
            settings: aacSettings(sampleRate: format.sampleRate, channels: min(2, format.channelCount)),
            commonFormat: .pcmFormatFloat32,
            interleaved: format.isInterleaved
        )
        try check(
            AudioDeviceCreateIOProcIDWithBlock(&procID, aggregateID, queue) { [weak self] _, input, _, _, _ in
                guard let self, let file = self.file,
                      let buffer = AVAudioPCMBuffer(pcmFormat: format, bufferListNoCopy: input, deallocator: nil) else { return }
                try? file.write(from: buffer)
                self.peak = max(self.peak, level(buffer))
            },
            "Starting the audio callback"
        )
        try check(AudioDeviceStart(aggregateID, procID), "Starting capture")
    }

    func stop() {
        if aggregateID != kAudioObjectUnknown {
            AudioDeviceStop(aggregateID, procID)
            if let procID { AudioDeviceDestroyIOProcID(aggregateID, procID) }
            AudioHardwareDestroyAggregateDevice(aggregateID)
        }
        if tapID != kAudioObjectUnknown { AudioHardwareDestroyProcessTap(tapID) }
        queue.sync { file = nil }
    }

    func takePeak() -> Float {
        defer { peak = 0 }
        return peak
    }
}

// MARK: - Commands

func record(dir: String) {
    let folder = URL(fileURLWithPath: dir)
    try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
    let mic = MicRecorder()
    let system = SystemRecorder()
    let started = Date()

    // Microphone permission (the prompt is attributed to the Dev Inbox app).
    let semaphore = DispatchSemaphore(value: 0)
    var micAllowed = false
    AVCaptureDevice.requestAccess(for: .audio) { granted in
        micAllowed = granted
        semaphore.signal()
    }
    semaphore.wait()

    var sources: [String] = []
    if micAllowed {
        do {
            try mic.start(to: folder.appendingPathComponent("mic.m4a"))
            sources.append("mic")
        } catch {
            emit(["event": "warning", "message": "Microphone: \(error.localizedDescription)"])
        }
    } else {
        emit(["event": "warning", "message": "Microphone access was denied. Allow it in System Settings → Privacy & Security → Microphone."])
    }
    do {
        try system.start(to: folder.appendingPathComponent("system.m4a"))
        sources.append("system")
    } catch {
        emit(["event": "warning", "message": "Meeting audio: \(error.localizedDescription). Allow it in System Settings → Privacy & Security → Screen & System Audio Recording."])
    }
    if sources.isEmpty { fail("Nothing could be recorded") }
    emit(["event": "started", "sources": sources])

    // Level meters for the app.
    let timer = DispatchSource.makeTimerSource(queue: .global())
    timer.schedule(deadline: .now() + 0.5, repeating: 0.5)
    timer.setEventHandler {
        emit(["event": "level", "mic": mic.takePeak(), "system": system.takePeak(), "seconds": Int(Date().timeIntervalSince(started))])
    }
    timer.resume()

    // Run until the app says "stop" (or goes away and closes our stdin).
    while let line = readLine() {
        if line.trimmingCharacters(in: .whitespaces) == "stop" { break }
    }
    timer.cancel()
    mic.stop()
    system.stop()
    emit(["event": "stopped", "seconds": Int(Date().timeIntervalSince(started)), "sources": sources])
}

@available(macOS 26, *)
func transcribe(path: String, localeID: String) async {
    guard SpeechTranscriber.isAvailable else { fail("On-device transcription isn't available on this Mac") }
    guard let locale = await SpeechTranscriber.supportedLocale(equivalentTo: Locale(identifier: localeID)) else {
        fail("Transcription doesn't support \(localeID)")
    }
    let transcriber = SpeechTranscriber(locale: locale, preset: .transcription)
    do {
        // First use of a language downloads its model (once, managed by macOS).
        if let request = try await AssetInventory.assetInstallationRequest(supporting: [transcriber]) {
            emit(["event": "downloading", "message": "Downloading the \(locale.identifier) speech model…"])
            try await request.downloadAndInstall()
        }
        let file = try AVAudioFile(forReading: URL(fileURLWithPath: path))
        let analyzer = SpeechAnalyzer(modules: [transcriber])
        let collector = Task {
            var count = 0
            for try await result in transcriber.results {
                let text = String(result.text.characters).trimmingCharacters(in: .whitespacesAndNewlines)
                if text.isEmpty { continue }
                emit([
                    "start": result.range.start.seconds,
                    "end": CMTimeRangeGetEnd(result.range).seconds,
                    "text": text,
                ])
                count += 1
            }
            return count
        }
        if let last = try await analyzer.analyzeSequence(from: file) {
            try await analyzer.finalizeAndFinish(through: last)
        } else {
            await analyzer.cancelAndFinishNow()
        }
        let count = try await collector.value
        emit(["event": "done", "segments": count])
    } catch {
        fail("Transcription failed: \(error.localizedDescription)")
    }
}

@available(macOS 26, *)
func checkSpeech(localeID: String) async {
    let locale = await SpeechTranscriber.supportedLocale(equivalentTo: Locale(identifier: localeID))
    emit([
        "speech": SpeechTranscriber.isAvailable && locale != nil,
        "locale": locale?.identifier ?? NSNull(),
        "summary": summaryUnavailableReason() == nil,
        "summaryReason": summaryUnavailableReason() ?? NSNull(),
    ])
}

// MARK: - Summaries (Apple Intelligence, on-device)

@available(macOS 26, *)
@Generable
struct ActionItem {
    @Guide(description: "Who agreed to do it: you (the person who recorded the meeting), others, or unclear", .anyOf(["you", "others", "unclear"]))
    var owner: String
    @Guide(description: "What will be done, as one short sentence")
    var text: String
}

@available(macOS 26, *)
@Generable
struct MeetingNotes {
    @Guide(description: "Two to four sentences on what the meeting was about and where it ended up")
    var overview: String
    @Guide(description: "Decisions that were made, each as one short sentence; empty if none")
    var decisions: [String]
    @Guide(description: "Things someone agreed to do; empty if none")
    var actionItems: [ActionItem]
}

/// Why on-device summaries can't run on this Mac right now, in words; nil when they can.
@available(macOS 26, *)
func summaryUnavailableReason() -> String? {
    switch SystemLanguageModel.default.availability {
    case .available: return nil
    case .unavailable(.deviceNotEligible): return "This Mac doesn't support Apple Intelligence"
    case .unavailable(.appleIntelligenceNotEnabled): return "Turn on Apple Intelligence in System Settings to summarise on this Mac"
    case .unavailable(.modelNotReady): return "Apple Intelligence is still downloading its model — try again later"
    case .unavailable: return "Apple Intelligence isn't available right now"
    }
}

let summaryInstructions = """
    You write notes for a meeting from its transcript. Lines are labelled "You" (the person who recorded the \
    meeting) or "Others" (everyone else on the call). Only use what is in the transcript; don't invent names, \
    numbers or decisions. Transcripts come from speech recognition, so expect misheard words.
    """

/// Split lines into chunks of at most `limit` characters.
func chunks(_ lines: [String], limit: Int) -> [String] {
    var out: [String] = []
    var current = ""
    for line in lines {
        if !current.isEmpty && current.count + line.count + 1 > limit {
            out.append(current)
            current = ""
        }
        current += (current.isEmpty ? "" : "\n") + line
    }
    if !current.isEmpty { out.append(current) }
    return out
}

@available(macOS 26, *)
func notesText(_ n: MeetingNotes) -> String {
    var lines = ["Overview: \(n.overview)"]
    lines += n.decisions.map { "Decision: \($0)" }
    lines += n.actionItems.map { "Action (\($0.owner)): \($0.text)" }
    return lines.joined(separator: "\n")
}

@available(macOS 26, *)
func generate(_ prompt: String) async throws -> MeetingNotes {
    // Summarising your own content: the permissive guardrails stop ordinary meeting talk being refused.
    let model = SystemLanguageModel(guardrails: .permissiveContentTransformations)
    let session = LanguageModelSession(model: model, instructions: summaryInstructions)
    return try await session.respond(to: prompt, generating: MeetingNotes.self).content
}

/// The on-device model has a small context, so long meetings are summarised in parts, then the parts' notes
/// are combined (in rounds, if there are many).
@available(macOS 26, *)
func summarise(path: String) async {
    if let reason = summaryUnavailableReason() { fail(reason) }
    guard let text = try? String(contentsOfFile: path, encoding: .utf8) else { fail("Couldn't read the transcript") }
    let lines = text.split(separator: "\n").map(String.init).filter { !$0.isEmpty }
    guard !lines.isEmpty else { fail("The transcript is empty") }
    var limit = 6000
    for attempt in 0..<3 {
        do {
            let parts = chunks(lines, limit: limit)
            var notes: [MeetingNotes] = []
            for (i, part) in parts.enumerated() {
                let header = parts.count == 1 ? "Transcript:" : "Part \(i + 1) of \(parts.count) of the transcript:"
                notes.append(try await generate("\(header)\n\n\(part)"))
            }
            while notes.count > 1 {
                let groups = chunks(notes.map(notesText), limit: limit)
                var merged: [MeetingNotes] = []
                for group in groups {
                    merged.append(try await generate(
                        "These are notes on consecutive parts of one meeting. Combine them into notes for the whole meeting, keeping every decision and action item once:\n\n\(group)"))
                }
                if merged.count >= notes.count { break }
                notes = merged
            }
            let n = notes[0]
            emit([
                "event": "summary",
                "overview": n.overview,
                "decisions": n.decisions,
                "actionItems": n.actionItems.map { ["owner": $0.owner, "text": $0.text] },
            ])
            return
        } catch {
            let message = String(describing: error)
            // Too long for the model's context: try again with smaller parts.
            if attempt < 2 && message.localizedCaseInsensitiveContains("context") {
                limit /= 2
                continue
            }
            fail("Couldn't summarise on this Mac: \(error.localizedDescription)")
        }
    }
}

@main
struct Main {
    static func main() async {
        signal(SIGPIPE, SIG_IGN)
        let args = CommandLine.arguments
        guard args.count >= 2 else { fail("usage: dev-inbox-recorder record <dir> | transcribe <file> [locale] | check [locale]") }
        let locale = Locale.current.identifier
        switch args[1] {
        case "record" where args.count >= 3:
            record(dir: args[2])
        case "transcribe" where args.count >= 3:
            if #available(macOS 26, *) {
                await transcribe(path: args[2], localeID: args.count >= 4 ? args[3] : locale)
            } else {
                fail("Transcription needs macOS 26 or later")
            }
        case "summarise" where args.count >= 3:
            if #available(macOS 26, *) {
                await summarise(path: args[2])
            } else {
                fail("Summarising on this Mac needs macOS 26 or later")
            }
        case "check":
            if #available(macOS 26, *) {
                await checkSpeech(localeID: args.count >= 3 ? args[2] : locale)
            } else {
                emit(["speech": false, "locale": NSNull()])
            }
        default:
            fail("unknown command \(args[1])")
        }
    }
}
