// Calendar events from the Mac's Calendar app (EventKit), which already syncs Google, Outlook/Exchange
// and iCloud calendars, so there's nothing to sign in to. Read-only.
//
//   dev-inbox-recorder calendar-status             -> {"status": "full" | "notDetermined" | "denied" | "restricted" | "writeOnly"}
//   dev-inbox-recorder calendar-access             -> asks macOS for access (once), then {"status": ...}
//   dev-inbox-recorder calendars                   -> {"calendars": [{"id","title","source","color"}]}
//   dev-inbox-recorder events <from> <to> [ids]    -> {"events": [...]}; ISO 8601 instants, ids comma-separated

import EventKit
import Foundation

func calendarStatus() -> String {
    switch EKEventStore.authorizationStatus(for: .event) {
    case .fullAccess: return "full"
    case .notDetermined: return "notDetermined"
    case .denied: return "denied"
    case .restricted: return "restricted"
    case .writeOnly: return "writeOnly"
    @unknown default: return "denied"
    }
}

func calendarAccess() async {
    if EKEventStore.authorizationStatus(for: .event) == .notDetermined {
        _ = try? await EKEventStore().requestFullAccessToEvents()
    }
    emit(["status": calendarStatus()])
}

private func hex(_ color: CGColor?) -> String {
    guard let c = color?.converted(to: CGColorSpace(name: CGColorSpace.sRGB)!, intent: .defaultIntent, options: nil),
          let p = c.components, p.count >= 3 else { return "#888888" }
    return String(format: "#%02x%02x%02x", Int(p[0] * 255), Int(p[1] * 255), Int(p[2] * 255))
}

func listCalendars() {
    guard calendarStatus() == "full" else { fail("No access to calendars") }
    let store = EKEventStore()
    let calendars = store.calendars(for: .event).map { c -> [String: Any] in
        ["id": c.calendarIdentifier, "title": c.title, "source": c.source?.title ?? "", "color": hex(c.cgColor)]
    }
    emit(["calendars": calendars])
}

/// The first video-call link in the event: its URL, location or notes.
private func videoLink(_ e: EKEvent) -> String? {
    let hosts = ["zoom.us", "meet.google.com", "teams.microsoft.com", "teams.live.com", "webex.com", "whereby.com", "around.co", "meet.jit.si", "chime.aws", "slack.com/huddle"]
    let texts = [e.url?.absoluteString, e.location, e.notes].compactMap { $0 }
    guard let detector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue) else { return nil }
    for text in texts {
        for match in detector.matches(in: text, range: NSRange(text.startIndex..., in: text)) {
            if let url = match.url, url.scheme?.hasPrefix("http") == true,
               hosts.contains(where: { url.absoluteString.lowercased().contains($0) }) {
                return url.absoluteString
            }
        }
    }
    return nil
}

private func myStatus(_ e: EKEvent) -> String {
    guard let me = e.attendees?.first(where: { $0.isCurrentUser }) else {
        // No attendee list, or you're the organiser.
        return e.organizer?.isCurrentUser == true ? "organiser" : "none"
    }
    switch me.participantStatus {
    case .accepted: return "accepted"
    case .declined: return "declined"
    case .tentative: return "tentative"
    case .pending: return "pending"
    default: return "none"
    }
}

func listEvents(from: String, to: String, ids: String?) {
    guard calendarStatus() == "full" else { fail("No access to calendars") }
    let iso = ISO8601DateFormatter()
    guard let start = iso.date(from: from), let end = iso.date(from: to) else { fail("Dates must be ISO 8601") }
    let store = EKEventStore()
    var calendars = store.calendars(for: .event)
    if let ids, !ids.isEmpty {
        let wanted = Set(ids.split(separator: ",").map(String.init))
        calendars = calendars.filter { wanted.contains($0.calendarIdentifier) }
        if calendars.isEmpty {
            emit(["events": []])
            return
        }
    }
    let predicate = store.predicateForEvents(withStart: start, end: end, calendars: calendars)
    let events = store.events(matching: predicate).map { e -> [String: Any] in
        let others = (e.attendees ?? []).filter { !$0.isCurrentUser }
        return [
            // Recurring events share an identifier: the start makes each occurrence unique.
            "uid": "\(e.calendarItemIdentifier)@\(iso.string(from: e.startDate))",
            "calendarId": e.calendar.calendarIdentifier,
            "calendar": e.calendar.title,
            "title": e.title ?? "",
            "start": iso.string(from: e.startDate),
            "end": iso.string(from: e.endDate),
            "allDay": e.isAllDay,
            "location": e.location ?? "",
            "notes": String((e.notes ?? "").prefix(2000)),
            "videoUrl": videoLink(e) ?? NSNull(),
            "attendees": others.count,
            "attendeeNames": others.prefix(12).compactMap { $0.name },
            "organiser": e.organizer?.name ?? NSNull(),
            "myStatus": myStatus(e),
            "free": e.availability == .free,
        ]
    }
    emit(["events": events])
}
