// General text generation with Apple Intelligence's on-device model, for Dev Inbox's AI features
// (asking about your history, day and week summaries, CI explanations…). The app does the retrieval
// and keeps prompts small; this just runs them.
//
//   dev-inbox-recorder generate <request.json>   {"instructions": "...", "prompt": "..."}
//     -> {"event":"text","text":"..."}, or {"event":"error","message":"..."}

import Foundation
import FoundationModels

@available(macOS 26, *)
func generate(requestPath: String) async {
    if let reason = summaryUnavailableReason() { fail(reason) }
    guard let data = FileManager.default.contents(atPath: requestPath),
          let request = try? JSONSerialization.jsonObject(with: data) as? [String: String],
          let prompt = request["prompt"] else { fail("Couldn't read the request") }
    do {
        // Your own data being summarised or explained: the permissive guardrails avoid refusing ordinary work talk.
        let model = SystemLanguageModel(guardrails: .permissiveContentTransformations)
        let session = LanguageModelSession(model: model, instructions: request["instructions"] ?? "")
        let response = try await session.respond(to: prompt)
        emit(["event": "text", "text": response.content])
    } catch {
        let message = String(describing: error)
        if message.localizedCaseInsensitiveContains("context") {
            fail("That's too much for Apple Intelligence to read at once — try a narrower question, or use Claude (Settings → AI)")
        }
        fail("Apple Intelligence couldn't answer: \(error.localizedDescription)")
    }
}
