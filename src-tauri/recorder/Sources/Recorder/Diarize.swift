// Speaker separation ("diarization") for the meeting audio track, on this Mac with FluidAudio's
// pyannote-style pipeline on the Neural Engine. Prints who spoke when; the app matches that up
// with the transcript lines.
//
//   dev-inbox-recorder diarize <audio file> <models dir>
//     -> {"speaker","start","end"} lines, a {"event":"voice","speaker","embedding"} line per speaker
//        (a unit-length voice fingerprint, for recognising them in later meetings), then
//        {"event":"done","speakers":n}

import FluidAudio
import Foundation

func diarize(path: String, modelsDir: String) async {
    do {
        let manager = OfflineDiarizerManager(config: .default)
        // Downloads (once, ~tens of MB) and compiles the models into the app's own folder if missing.
        try await manager.prepareModels(directory: URL(fileURLWithPath: modelsDir, isDirectory: true))
        let result = try await manager.process(URL(fileURLWithPath: path))
        var speakers = Set<String>()
        for segment in result.segments {
            speakers.insert(segment.speakerId)
            emit([
                "speaker": segment.speakerId,
                "start": Double(segment.startTimeSeconds),
                "end": Double(segment.endTimeSeconds),
            ])
        }
        for (speaker, embedding) in result.speakerDatabase ?? [:] where speakers.contains(speaker) {
            let norm = sqrt(embedding.reduce(0) { $0 + $1 * $1 })
            guard norm > 0 else { continue }
            emit(["event": "voice", "speaker": speaker, "embedding": embedding.map { Double($0 / norm) }])
        }
        emit(["event": "done", "speakers": speakers.count])
    } catch {
        fail("Couldn't tell speakers apart: \(error.localizedDescription)")
    }
}
