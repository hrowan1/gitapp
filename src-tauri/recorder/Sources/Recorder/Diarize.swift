// Speaker separation ("diarization") for the meeting audio track, on this Mac with FluidAudio's
// pyannote-style pipeline on the Neural Engine. Prints who spoke when; the app matches that up
// with the transcript lines.
//
//   dev-inbox-recorder diarize <audio file> <models dir>
//     -> {"speaker","start","end"} lines, then {"event":"done","speakers":n}

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
        emit(["event": "done", "speakers": speakers.count])
    } catch {
        fail("Couldn't tell speakers apart: \(error.localizedDescription)")
    }
}
