import { useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { listen } from "@tauri-apps/api/event";
import {
  api,
  type Meeting,
  type MeetingAction,
  type MeetingBrief,
  type MeetingSearchResult,
  type MeetingSummary,
  type RecordingStatus,
  type Segment,
} from "../lib/api";
import { setActionDone } from "./MeetingActions";
import { dayKey } from "../lib/day";
import { clock, friendlyDate } from "../lib/time";
import { toast } from "../lib/toast";
import { Icon } from "./Icon";

const CONSENT_NOTICE =
  "Heads up: I'm recording this meeting for my own notes. The recording and transcript stay on my machine.";

function mmss(total: number): string {
  const s = Math.max(0, Math.floor(total));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = String(s % 60).padStart(2, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${sec}` : `${m}:${sec}`;
}

/** How long a meeting lasted, e.g. "28 min" or "1 h 05 min". */
function lasted(total: number): string {
  const s = Math.max(0, Math.round(total));
  if (s < 60) return `${s} s`;
  const m = Math.round(s / 60);
  return m < 60 ? `${m} min` : `${Math.floor(m / 60)} h ${String(m % 60).padStart(2, "0")} min`;
}

/** Record meetings (mic + call audio), transcribed on this Mac. Recording only ever starts when you press Record. */
export function MeetingsView({
  version,
  recording,
  target,
  onOpenDay,
}: {
  version: number;
  recording: RecordingStatus;
  /** Open this meeting (e.g. clicked in the Day view), search for something (from ⌘K), or show a
   *  calendar meeting's brief (from its reminder). */
  target?: { id?: string; query?: string; at?: number | null; brief?: string } | null;
  onOpenDay: (day: string) => void;
}) {
  const [meetings, setMeetings] = useState<Meeting[] | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [title, setTitle] = useState("");
  const [busy, setBusy] = useState(false);
  const [levels, setLevels] = useState({ mic: 0, system: 0, seconds: 0 });
  const [warnings, setWarnings] = useState<string[]>([]);
  const [captions, setCaptions] = useState<Captions>(NO_CAPTIONS);
  // Calendar meetings coming up, and the one whose brief is open.
  const [upcoming, setUpcoming] = useState<MeetingBrief[]>([]);
  const [brief, setBrief] = useState<MeetingBrief | null>(null);
  useEffect(() => {
    const load = () => api.calendarUpcoming().then(setUpcoming).catch(() => {});
    load();
    const t = setInterval(load, 60_000);
    return () => clearInterval(t);
  }, [version]);
  const openBrief = (b: MeetingBrief) => {
    setSelected(null);
    setBrief(b);
  };
  useEffect(() => {
    if (!target?.brief) return;
    api.calendarBrief(target.brief).then((b) => b && openBrief(b), (e) => toast(String(e), true));
  }, [target]);

  useEffect(() => {
    if (target?.id) setSelected(target.id);
    if (target?.at != null) setJump({ at: target.at, n: Date.now() });
    if (target?.query !== undefined) setQuery(target.query);
  }, [target]);

  // Search: debounced, results replace the list while there's a query.
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<MeetingSearchResult[] | null>(null);
  const [jump, setJump] = useState<{ at: number; n: number } | null>(null);
  useEffect(() => {
    if (!query.trim()) return setResults(null);
    const t = setTimeout(() => {
      api.searchMeetings(query).then(setResults, (e) => toast(String(e), true));
    }, 180);
    return () => clearTimeout(t);
  }, [query, version]);
  const openHit = (id: string, at?: number | null) => {
    setSelected(id);
    setJump(at == null ? null : { at, n: Date.now() });
  };

  useEffect(() => {
    api.listMeetings().then(setMeetings).catch((e) => toast(String(e), true));
  }, [version]);

  useEffect(() => {
    const subs = [
      listen<{ mic: number; system: number; seconds: number }>("recording-level", (e) => setLevels(e.payload)),
      listen<Caption>("recording-caption", (e) => setCaptions((c) => addCaption(c, e.payload))),
      listen<{ event: string; message?: string }>("recording-event", (e) => {
        if ((e.payload.event === "warning" || e.payload.event === "info") && e.payload.message)
          setWarnings((w) => [...w, e.payload.message!]);
        if (e.payload.event === "error") toast(e.payload.message ?? "Recording failed", true);
      }),
    ];
    return () => subs.forEach((p) => p.then((f) => f()));
  }, []);

  useEffect(() => {
    if (!recording.recording) setLevels({ mic: 0, system: 0, seconds: 0 });
    setCaptions(NO_CAPTIONS);
  }, [recording.recording]);

  const start = async () => {
    setBusy(true);
    setWarnings([]);
    try {
      const s = await api.startRecording(title);
      setTitle("");
      if (s.meetingId) setSelected(null);
    } catch (e) {
      toast(String(e), true);
    } finally {
      setBusy(false);
    }
  };
  const stop = async () => {
    setBusy(true);
    try {
      const id = recording.meetingId;
      await api.stopRecording();
      if (id) setSelected(id);
      toast("Recording saved — transcribing on this Mac…");
    } catch (e) {
      toast(String(e), true);
    } finally {
      setBusy(false);
    }
  };
  const copyNotice = async () => {
    try {
      await navigator.clipboard.writeText(CONSENT_NOTICE);
      toast("Notice copied — paste it into the meeting chat");
    } catch {
      toast(CONSENT_NOTICE);
    }
  };

  const meeting = meetings?.find((m) => m.id === selected) ?? null;
  const shownBrief = meeting ? null : brief;

  return (
    <div className={`workspace${meeting || shownBrief ? " has-selection" : ""}`}>
      <section className="pane-list">
        <div className="pane-header">
          <div className="headline compact">
            <h1>Meetings</h1>
            <p className="muted">Recorded and transcribed on this Mac, with AI notes (Settings → Meetings).</p>
          </div>

          <div className={`recorder${recording.recording ? " live" : ""}`}>
            {recording.recording ? (
              <>
                <div className="rec-top">
                  <span className="rec-dot" />
                  <span className="strong">Recording</span>
                  <span className="rec-time">{mmss(levels.seconds)}</span>
                  <span className="grow" />
                  <button className="danger" disabled={busy} onClick={stop}>
                    <Icon name="stop" size={13} /> Stop
                  </button>
                </div>
                <Meter label="You" value={levels.mic} />
                <Meter label="Meeting audio" value={levels.system} />
                <LiveCaptions captions={captions} />
                {warnings.map((w, i) => (
                  <p key={i} className="warn small">
                    {w}
                  </p>
                ))}
              </>
            ) : (
              <>
                <div className="rec-top">
                  <input
                    className="grow"
                    placeholder="What's the meeting? e.g. IO-2641 refinement"
                    value={title}
                    onChange={(e) => setTitle(e.target.value)}
                    onKeyDown={(e) => e.key === "Enter" && start()}
                  />
                  <button className="primary rec-btn" disabled={busy} onClick={start}>
                    <Icon name="mic" size={14} /> Record
                  </button>
                </div>
                <p className="muted small consent">
                  Let people know you're recording.{" "}
                  <button className="link" onClick={copyNotice}>
                    Copy a notice for the chat
                  </button>
                </p>
              </>
            )}
          </div>
          <div className="filters meeting-search">
            <div className="search-box">
              <Icon name="search" size={14} />
              <input
                placeholder="Search transcripts and notes"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                onKeyDown={(e) => e.key === "Escape" && setQuery("")}
              />
              {query && (
                <button className="ghost icon-only" title="Clear" onClick={() => setQuery("")}>
                  <Icon name="x" size={12} />
                </button>
              )}
            </div>
          </div>
        </div>

        <div className="pane-scroll">
          {results && <SearchResults results={results} selected={selected} onOpen={openHit} />}
          {!results && upcoming.length > 0 && (
            <>
              <div className="section-head static">
                <span className="section-title">Up next</span>
                <span className="count">{upcoming.length}</span>
              </div>
              {upcoming.map((b) => (
                <div
                  key={b.event.uid}
                  className={`row${shownBrief?.event.uid === b.event.uid ? " selected" : ""}`}
                  role="button"
                  onClick={() => openBrief(b)}
                >
                  <span className="state-icon tone-blue">
                    <Icon name="calendar" size={14} />
                  </span>
                  <div className="row-main">
                    <div className="row-title">
                      {b.ticket && !b.event.title.toUpperCase().includes(b.ticket.key) && (
                        <span className="ticket-key">{b.ticket.key}</span>
                      )}
                      <span className="title">{b.event.title || "Untitled meeting"}</span>
                    </div>
                    <div className="row-sub">
                      <span>
                        {friendlyDate(b.event.start)} {clock(b.event.start)}
                      </span>
                      {fromNow(b.event.start, b.event.end) && (
                        <>
                          <span className="sep">·</span>
                          <span>{fromNow(b.event.start, b.event.end)}</span>
                        </>
                      )}
                      {b.openActions.length > 0 && (
                        <>
                          <span className="sep">·</span>
                          <span className="reason">
                            {b.openActions.length} open from last time
                          </span>
                        </>
                      )}
                    </div>
                  </div>
                </div>
              ))}
              {meetings && meetings.length > 0 && (
                <div className="section-head static">
                  <span className="section-title">Recorded</span>
                </div>
              )}
            </>
          )}
          {!results && meetings?.length === 0 && (
            <div className="empty">No meetings yet. Press Record when one starts — your mic and the call's audio are saved.</div>
          )}
          {!results && meetings?.map((m) => (
            <div
              key={m.id}
              className={`row${selected === m.id ? " selected" : ""}`}
              role="button"
              onClick={() => {
                setBrief(null);
                setSelected(m.id);
              }}
            >
              <span className={`state-icon tone-${m.status === "failed" ? "red" : m.status === "done" ? "violet" : "yellow"}`}>
                <Icon name={m.status === "recording" ? "mic" : m.status === "transcribing" ? "loader" : m.status === "failed" ? "alert" : "message"} size={14} />
              </span>
              <div className="row-main">
                <div className="row-title">
                  {m.ticket && !m.title.toUpperCase().includes(m.ticket) && <span className="ticket-key">{m.ticket}</span>}
                  <span className="title">{m.title}</span>
                </div>
                <div className="row-sub">
                  <span>
                    {friendlyDate(m.startedAt)} {clock(m.startedAt)}
                  </span>
                  {m.seconds != null && (
                    <>
                      <span className="sep">·</span>
                      <span>{lasted(m.seconds)}</span>
                    </>
                  )}
                  <span className="sep">·</span>
                  <span className="reason">
                    {m.status === "recording"
                      ? "Recording…"
                      : m.status === "transcribing"
                        ? "Transcribing…"
                        : m.status === "failed"
                          ? m.error ?? "Failed"
                          : (m.summary?.overview ?? m.preview ?? "No speech found")}
                  </span>
                </div>
              </div>
            </div>
          ))}
        </div>
      </section>

      {meeting ? (
        <MeetingDetail
          key={meeting.id}
          meeting={meeting}
          version={version}
          jump={jump}
          onClose={() => setSelected(null)}
          onOpenDay={onOpenDay}
        />
      ) : shownBrief ? (
        <BriefPane
          key={shownBrief.event.uid}
          brief={shownBrief}
          recording={recording.recording}
          onClose={() => setBrief(null)}
          onOpenMeeting={(id) => {
            setBrief(null);
            setSelected(id);
          }}
        />
      ) : (
        <aside className="detail empty-detail">
          <div className="empty-art">
            <Icon name="mic" size={26} />
          </div>
          <p className="strong">Select a meeting</p>
          <p className="muted small">
            Read the transcript, jump to any moment in the audio, and link it to a ticket so it counts in your Day view.
          </p>
        </aside>
      )}
    </div>
  );
}

/** "in 2h 5m", "now", "started 5m ago"; nothing more than 12 hours ahead (the date says enough). */
function fromNow(start: string, end: string): string | null {
  const now = Date.now();
  const s = new Date(start).getTime();
  const e = new Date(end).getTime();
  const span = (ms: number) => {
    const m = Math.round(ms / 60_000);
    return m >= 60 ? `${Math.floor(m / 60)}h${m % 60 ? ` ${m % 60}m` : ""}` : `${m}m`;
  };
  if (s - now > 12 * 3_600_000) return null;
  if (s - now > 60_000) return `in ${span(s - now)}`;
  if (now < e) return now - s < 60_000 ? "now" : `started ${span(now - s)} ago`;
  return "ended";
}

/** Before a calendar meeting: its ticket and that ticket's open PRs, how the last one went, and Record. */
function BriefPane({
  brief,
  recording,
  onClose,
  onOpenMeeting,
}: {
  brief: MeetingBrief;
  recording: boolean;
  onClose: () => void;
  onOpenMeeting: (id: string) => void;
}) {
  const e = brief.event;
  const [busy, setBusy] = useState(false);
  const names = e.attendeeNames;
  const with_ =
    names.length === 0
      ? null
      : names.length <= 3
        ? names.join(", ")
        : `${names.slice(0, 3).join(", ")} and ${names.length - 3} more`;
  const record = async () => {
    setBusy(true);
    try {
      await api.calendarRecord(e.uid);
      toast(`Recording “${e.title}”`);
    } catch (err) {
      toast(String(err), true);
    } finally {
      setBusy(false);
    }
  };
  return (
    <aside className="detail">
      <button className="detail-close ghost" onClick={onClose} title="Close">
        <Icon name="x" />
      </button>
      <div className="detail-body">
        <div className="detail-meta">
          <Icon name="calendar" size={14} />
          <span>
            {friendlyDate(e.start)} {clock(e.start)}–{clock(e.end)}
          </span>
          {fromNow(e.start, e.end) && (
            <>
              <span className="sep">·</span>
              <span>{fromNow(e.start, e.end)}</span>
            </>
          )}
          <span className="sep">·</span>
          <span>{e.calendar}</span>
        </div>
        <h2 className="brief-title">{e.title || "Untitled meeting"}</h2>
        {with_ && <p className="muted small">With {with_}</p>}
        <div className="actions brief-buttons">
          <button className="primary" disabled={busy || recording} onClick={record} title={recording ? "Already recording" : ""}>
            <Icon name="mic" size={14} /> Record
          </button>
          {e.videoUrl && <button onClick={() => openUrl(e.videoUrl!)}>Join call</button>}
        </div>

        {brief.ticket && (
          <section className="brief-section">
            <h3>Ticket</h3>
            <div
              className="brief-ticket"
              role="button"
              onClick={() => brief.ticket?.url && openUrl(brief.ticket.url)}
              title="Open in Jira"
            >
              <span className="ticket-key">{brief.ticket.key}</span>
              <span className="grow">{brief.ticket.summary ?? "Not one of your Jira tickets"}</span>
              {brief.ticket.status && <span className="pill">{brief.ticket.status}</span>}
            </div>
            {brief.ticket.prs.map((p) => (
              <div key={p.id} className="brief-pr" role="button" onClick={() => openUrl(p.url)}>
                <Icon name="pr" size={13} />
                <span className="grow">{p.title}</span>
                <span className="muted small">
                  {p.repo.split("/").pop()}#{p.number}
                </span>
              </div>
            ))}
          </section>
        )}

        {brief.last && (
          <section className="brief-section">
            <h3>
              Last time · {friendlyDate(brief.last.startedAt)}{" "}
              <button className="link small" onClick={() => onOpenMeeting(brief.last!.id)}>
                open notes
              </button>
            </h3>
            {brief.last.summary?.overview && <p className="small">{brief.last.summary.overview}</p>}
            {brief.openActions.length > 0 && (
              <ul className="brief-actions">
                {brief.openActions.map((a) => (
                  <li key={a.id}>
                    <span className="muted small">{OWNER[a.owner] ?? a.owner}:</span> {a.text}
                  </li>
                ))}
              </ul>
            )}
          </section>
        )}

        {!brief.ticket && !brief.last && (
          <p className="muted small">
            Nothing to brief yet. Put a ticket key in the event's title (“BILL-142 sync”) to bring in the ticket and its
            PRs; once you record one, next time shows its notes and open action items.
          </p>
        )}

        {e.notes.trim() && (
          <details className="brief-notes">
            <summary className="small">Invite notes</summary>
            <p className="small">{e.notes}</p>
          </details>
        )}
      </div>
    </aside>
  );
}

const OWNER: Record<string, string> = { you: "You", others: "Others", unclear: "—" };

/** "claude-haiku-5-5" → "Claude Haiku 5.5"; "apple" → Apple Intelligence. */
function writtenBy(by?: string | null): string {
  if (!by || by === "apple") return "Apple Intelligence on this Mac";
  const m = by.match(/^claude-([a-z]+)-(\d+)-(\d+)$/);
  return m ? `Claude ${m[1].charAt(0).toUpperCase()}${m[1].slice(1)} ${m[2]}.${m[3]}` : "Claude";
}

function notesText(s: MeetingSummary): string {
  const parts = [s.overview];
  if (s.decisions.length) parts.push(`Decisions\n${s.decisions.map((d) => `• ${d}`).join("\n")}`);
  if (s.actionItems.length) parts.push(`Action items\n${s.actionItems.map((a) => `• ${OWNER[a.owner] ?? a.owner}: ${a.text}`).join("\n")}`);
  return parts.join("\n\n");
}

/** AI notes for a meeting: overview, decisions, action items — or where they're up to. */
function MeetingNotes({
  meeting,
  actions,
  hasTranscript,
}: {
  meeting: Meeting;
  /** Progress on the action items (yours can be ticked off; they also show in Today). */
  actions: MeetingAction[];
  hasTranscript: boolean;
}) {
  const summarise = () => api.summariseMeeting(meeting.id).catch((e) => toast(String(e), true));
  const who = writtenBy(meeting.summaryBy);
  if (meeting.summaryStatus === "summarising") {
    return (
      <div className="callout">
        <Icon name="loader" size={14} className="spin" />
        <span>Writing notes…</span>
      </div>
    );
  }
  if (meeting.summaryStatus === "failed") {
    return (
      <div className="callout error-callout">
        <Icon name="alert" size={14} />
        <span className="grow">{meeting.summaryError ?? "Couldn't write notes"}</span>
        <button className="link" onClick={summarise}>
          Try again
        </button>
      </div>
    );
  }
  const s = meeting.summary;
  if (!s) {
    if (meeting.status !== "done" || !hasTranscript) return null;
    return (
      <div className="notes-empty">
        <button className="link small" onClick={summarise}>
          <Icon name="sparkle" size={13} /> Write notes for this meeting
        </button>
      </div>
    );
  }
  return (
    <section className="notes">
      <div className="transcript-head">
        <div className="group-label">Notes</div>
        <span className="grow" />
        <button
          className="ghost icon-only"
          title="Copy notes"
          onClick={() => navigator.clipboard.writeText(`${meeting.title}\n\n${notesText(s)}\n`).then(() => toast("Notes copied"))}
        >
          <Icon name="copy" size={14} />
        </button>
      </div>
      <p className="notes-overview">{s.overview}</p>
      {s.decisions.length > 0 && (
        <>
          <div className="notes-label">Decisions</div>
          <ul className="notes-list">
            {s.decisions.map((d, i) => (
              <li key={i}>{d}</li>
            ))}
          </ul>
        </>
      )}
      {s.actionItems.length > 0 && (
        <>
          <div className="notes-label">Action items</div>
          <ul className="notes-list notes-actions">
            {s.actionItems.map((a, i) => {
              const tracked = a.owner === "you" ? actions.find((x) => x.text === a.text.trim()) : undefined;
              return (
                <li key={i} className={tracked?.done ? "done" : undefined}>
                  {tracked ? (
                    <input
                      type="checkbox"
                      checked={tracked.done}
                      title={tracked.done ? "Done" : "Mark done (it's also in Today)"}
                      onChange={(e) => void setActionDone(tracked, e.target.checked)}
                    />
                  ) : (
                    <span className="no-check" />
                  )}
                  <span className={`owner owner-${a.owner}`}>{OWNER[a.owner] ?? a.owner}</span>
                  <span>{a.text}</span>
                </li>
              );
            })}
          </ul>
        </>
      )}
      <p className="muted small notes-by">
        <Icon name="sparkle" size={12} /> Written by {who}. AI notes can be wrong — check the transcript.{" "}
        <button className="link small" onClick={summarise}>
          Rewrite
        </button>
      </p>
    </section>
  );
}

interface Caption {
  speaker: "you" | "others";
  text: string;
  /** false while the recogniser may still change it */
  final: boolean;
  start: number;
  /** When it first arrived (ms). The two tracks' own clocks drift apart, so echoes are matched on this. */
  at?: number;
}
interface Captions {
  /** Settled lines, oldest first (only the last few are kept). */
  done: Caption[];
  /** The line each speaker is in the middle of. */
  live: Partial<Record<Caption["speaker"], Caption>>;
}
const NO_CAPTIONS: Captions = { done: [], live: {} };

function addCaption(c: Captions, incoming: Caption): Captions {
  const live = { ...c.live };
  const cap = { ...incoming, at: live[incoming.speaker]?.at ?? Date.now() };
  if (cap.final) {
    delete live[cap.speaker];
    return { done: [...c.done, cap].slice(-6), live };
  }
  live[cap.speaker] = cap;
  return { done: c.done, live };
}

function words(text: string): Set<string> {
  return new Set(
    text
      .toLowerCase()
      .split(/[^\p{L}\p{N}]+/u)
      .filter((w) => w.length > 2),
  );
}

/** Your mic can hear the call (speakers, or headphones loud enough to leak), so a "You" line that
 * repeats what "Others" said moments ago is the call, not you. Same rule as the saved transcript. */
function isEcho(line: Caption, all: Caption[]): boolean {
  if (line.speaker !== "you") return false;
  const mine = words(line.text);
  if (mine.size === 0) return false;
  return all.some((o) => {
    if (o.speaker !== "others" || Math.abs((o.at ?? 0) - (line.at ?? 0)) > 10_000) return false;
    const theirs = words(o.text);
    const shared = [...mine].filter((w) => theirs.has(w)).length;
    return shared >= Math.min(2, mine.size) && shared / Math.max(1, Math.min(mine.size, theirs.size)) >= 0.6;
  });
}

/** What's being said right now; the proper transcript is made from the recording afterwards. */
function LiveCaptions({ captions }: { captions: Captions }) {
  const all = [...captions.done, ...Object.values(captions.live)];
  const lines = all
    .filter((l) => !isEcho(l, all))
    .sort((a, b) => a.start - b.start)
    .slice(-4);
  if (lines.length === 0) return null;
  return (
    <div className="captions" aria-live="polite">
      {lines.map((l, i) => (
        <div key={`${l.speaker}-${l.start}-${i}`} className={`caption speaker-${l.speaker}${l.final ? "" : " volatile"}`}>
          <span className="line-who">{l.speaker === "you" ? "You" : "Others"}</span>
          <span>{l.text}</span>
        </div>
      ))}
    </div>
  );
}

function Meter({ label, value }: { label: string; value: number }) {
  return (
    <div className="meter">
      <span className="muted small">{label}</span>
      <span className="meter-bar">
        <span style={{ width: `${Math.round(Math.min(1, value) * 100)}%` }} />
      </span>
    </div>
  );
}

/** Snippet text with the search matches (\u0002 … \u0003) highlighted. */
function Highlighted({ text }: { text: string }) {
  return (
    <>
      {text.split(/(\u0002[^\u0003]*\u0003)/).map((part, i) =>
        part.startsWith("\u0002") ? <mark key={i}>{part.slice(1, -1)}</mark> : part,
      )}
    </>
  );
}

function SearchResults({
  results,
  selected,
  onOpen,
}: {
  results: MeetingSearchResult[];
  selected: string | null;
  onOpen: (id: string, at?: number | null) => void;
}) {
  if (results.length === 0) return <div className="empty">No meeting mentions that.</div>;
  return (
    <>
      {results.map((r) => (
        <div key={r.meetingId} className={`search-result${selected === r.meetingId ? " selected" : ""}`}>
          <div className="row" role="button" onClick={() => onOpen(r.meetingId)}>
            <span className="state-icon tone-violet">
              <Icon name="mic" size={14} />
            </span>
            <div className="row-main">
              <div className="row-title">
                {r.ticket && !r.title.toUpperCase().includes(r.ticket) && <span className="ticket-key">{r.ticket}</span>}
                <span className="title">{r.title}</span>
              </div>
              <div className="row-sub">
                <span>
                  {friendlyDate(r.startedAt)} {clock(r.startedAt)}
                </span>
              </div>
            </div>
          </div>
          {r.hits
            .filter((h) => h.kind !== "title")
            .map((h, i) => (
              <div
                key={i}
                className="search-hit"
                role="button"
                onClick={() => onOpen(r.meetingId, h.start)}
                title={h.kind === "line" ? "Open at this line" : "Open the notes"}
              >
                <span className="line-time">{h.kind === "line" && h.start != null ? mmss(h.start) : "notes"}</span>
                {h.kind === "line" && <span className="hit-who">{h.speaker}</span>}
                <span className="hit-text">
                  <Highlighted text={h.snippet} />
                </span>
              </div>
            ))}
        </div>
      ))}
    </>
  );
}

/** "You", the name you gave a voice, "Speaker 2", or "Others". */
function speakerLabel(s: Segment, names: Record<string, string>): string {
  if (s.speaker === "you") return "You";
  if (s.voice) return names[s.voice] ?? `Speaker ${s.voice}`;
  return "Others";
}

function MeetingDetail({
  meeting,
  version,
  jump,
  onClose,
  onOpenDay,
}: {
  meeting: Meeting;
  version: number;
  /** Scroll to (and flash) the transcript line starting at `at`, e.g. from a search result. */
  jump?: { at: number; n: number } | null;
  onClose: () => void;
  onOpenDay: (day: string) => void;
}) {
  const [segments, setSegments] = useState<Segment[] | null>(null);
  const [speakers, setSpeakers] = useState<Record<string, string>>({});
  const [recognised, setRecognised] = useState<string[]>([]);
  const [renaming, setRenaming] = useState<{ voice: string; name: string } | null>(null);
  const body = useRef<HTMLDivElement>(null);
  const [actions, setActions] = useState<MeetingAction[]>([]);
  const [title, setTitle] = useState(meeting.title);
  const [ticket, setTicket] = useState(meeting.ticket ?? "");
  const [find, setFind] = useState("");
  const [playing, setPlaying] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const mic = useRef<HTMLAudioElement>(null);
  const system = useRef<HTMLAudioElement>(null);

  // Jump to a line (from search) once the transcript is there.
  useEffect(() => {
    if (!jump || !segments) return;
    const line = body.current?.querySelector<HTMLElement>(`[data-start="${jump.at}"]`);
    if (!line) return;
    line.scrollIntoView({ block: "center", behavior: "smooth" });
    line.classList.add("flash");
    const t = setTimeout(() => line.classList.remove("flash"), 2000);
    return () => clearTimeout(t);
  }, [jump, segments]);

  useEffect(() => {
    api
      .getMeeting(meeting.id)
      .then((d) => {
        setSegments(d.segments);
        setActions(d.actions);
        setSpeakers(d.speakers);
        setRecognised(d.recognised);
      })
      .catch((e) => toast(String(e), true));
  }, [meeting.id, meeting.status, version]);

  const sources = (meeting.sources ?? "").split(",").filter(Boolean);
  const audio = (name: string) => (meeting.audioDir ? convertFileSrc(`${meeting.audioDir}/${name}`) : undefined);
  const players = () => [mic.current, system.current].filter((p): p is HTMLAudioElement => !!p);

  const seek = (t: number) => {
    for (const p of players()) {
      p.currentTime = t;
      void p.play().catch(() => {});
    }
    setPlaying(true);
  };
  const toggle = () => {
    for (const p of players()) {
      if (playing) p.pause();
      else void p.play().catch(() => {});
    }
    setPlaying(!playing);
  };

  const save = async () => {
    try {
      await api.updateMeeting(meeting.id, title, ticket.trim().toUpperCase() || null);
      toast("Saved");
    } catch (e) {
      toast(String(e), true);
    }
  };
  const copyTranscript = async () => {
    if (!segments) return;
    const text = segments.map((s) => `[${mmss(s.start)}] ${speakerLabel(s, speakers)}: ${s.text}`).join("\n");
    const notes = meeting.summary ? `${notesText(meeting.summary)}\n\nTranscript\n\n` : "";
    await navigator.clipboard.writeText(`${meeting.title}\n\n${notes}${text}\n`).catch(() => {});
    toast("Transcript copied");
  };

  const shown = useMemo(() => {
    const q = find.trim().toLowerCase();
    return (segments ?? []).filter((s) => !q || s.text.toLowerCase().includes(q));
  }, [segments, find]);
  const dirty = title !== meeting.title || (ticket.trim().toUpperCase() || null) !== (meeting.ticket ?? null);

  return (
    <aside className="detail">
      <button className="detail-close ghost" onClick={onClose} title="Close">
        <Icon name="x" />
      </button>
      <div className="detail-body" ref={body}>
        <div className="detail-meta">
          <Icon name="mic" size={14} />
          <span>
            {friendlyDate(meeting.startedAt)} {clock(meeting.startedAt)}
          </span>
          {meeting.seconds != null && (
            <>
              <span className="sep">·</span>
              <span>{lasted(meeting.seconds)}</span>
            </>
          )}
          <span className="sep">·</span>
          <button className="link small" onClick={() => onOpenDay(dayKey(new Date(meeting.startedAt)))}>
            show in Day view
          </button>
        </div>
        <input className="title-input" value={title} onChange={(e) => setTitle(e.target.value)} />
        <div className="meeting-fields">
          <label className="field">
            <span>Counts towards</span>
            <input
              className="grow mono"
              placeholder="Ticket, e.g. IO-2641"
              value={ticket}
              onChange={(e) => setTicket(e.target.value)}
            />
          </label>
          {dirty && (
            <button className="primary" onClick={save}>
              Save
            </button>
          )}
        </div>

        {meeting.status === "transcribing" && (
          <div className="callout">
            <Icon name="loader" size={14} className="spin" />
            <span>Transcribing on this Mac…</span>
          </div>
        )}
        {meeting.status === "failed" && (
          <div className="callout error-callout">
            <Icon name="alert" size={14} />
            <span className="grow">{meeting.error ?? "Something went wrong"}</span>
            {meeting.hasAudio && (
              <button className="link" onClick={() => api.retranscribeMeeting(meeting.id).catch((e) => toast(String(e), true))}>
                Try again
              </button>
            )}
          </div>
        )}

        {meeting.hasAudio && meeting.status !== "recording" && sources.length > 0 && (
          <div className="player">
            <button className="icon-only" onClick={toggle} title={playing ? "Pause" : "Play"}>
              <Icon name={playing ? "pause" : "play"} size={14} />
            </button>
            <span className="muted small">Click a line to play from there</span>
            {sources.includes("mic") && <audio ref={mic} src={audio("mic.m4a")} preload="metadata" onEnded={() => setPlaying(false)} />}
            {sources.includes("system") && (
              <audio ref={system} src={audio("system.m4a")} preload="metadata" onEnded={() => setPlaying(false)} />
            )}
          </div>
        )}
        {!meeting.hasAudio && meeting.status === "done" && (
          <p className="muted small">The audio has been deleted (Settings → Meetings); the transcript is kept.</p>
        )}

        {meeting.suggestedTicket && !meeting.ticket && (
          <div className="callout suggestion">
            <Icon name="sparkle" size={14} />
            <span className="grow">
              Looks like <span className="ticket-key">{meeting.suggestedTicket}</span>
              {meeting.suggestedTitle && <span className="muted"> {meeting.suggestedTitle}</span>}
            </span>
            <button
              className="link"
              onClick={() => {
                setTicket(meeting.suggestedTicket!);
                api.linkMeeting(meeting.id, meeting.suggestedTicket!).catch((e) => toast(String(e), true));
              }}
            >
              Link
            </button>
            <button className="link muted" onClick={() => api.dismissSuggestion(meeting.id).catch((e) => toast(String(e), true))}>
              Not this
            </button>
          </div>
        )}

        <MeetingNotes meeting={meeting} actions={actions} hasTranscript={!!segments?.length} />

        <div className="timeline">
          <div className="transcript-head">
            <div className="group-label">Transcript</div>
            <span className="grow" />
            {segments && segments.length > 0 && (
              <>
                <div className="search-box small-search">
                  <Icon name="search" size={12} />
                  <input placeholder="Find" value={find} onChange={(e) => setFind(e.target.value)} />
                </div>
                <button className="ghost icon-only" onClick={copyTranscript} title="Copy transcript">
                  <Icon name="copy" size={14} />
                </button>
              </>
            )}
          </div>
          {segments === null && <div className="muted small">Loading…</div>}
          {segments?.length === 0 && meeting.status === "done" && <div className="muted small">No speech was found.</div>}
          {shown.map((s, i) => (
            <div
              key={i}
              data-start={s.start}
              className={`line speaker-${s.speaker}${s.voice ? ` voice-${(Number(s.voice) - 1) % 6}` : ""}`}
              onClick={() => meeting.hasAudio && seek(s.start)}
              title={meeting.hasAudio ? "Play from here" : undefined}
            >
              <span className="line-time">{mmss(s.start)}</span>
              {renaming && s.voice === renaming.voice ? (
                <input
                  className="line-rename"
                  autoFocus
                  value={renaming.name}
                  placeholder={`Speaker ${s.voice}`}
                  onClick={(e) => e.stopPropagation()}
                  onChange={(e) => setRenaming({ ...renaming, name: e.target.value })}
                  onKeyDown={(e) => {
                    if (e.key === "Escape") setRenaming(null);
                    if (e.key === "Enter") e.currentTarget.blur();
                  }}
                  onBlur={() => {
                    const r = renaming;
                    setRenaming(null);
                    const name = r.name.trim();
                    if ((speakers[r.voice] ?? "") === name) return;
                    const next = { ...speakers };
                    if (name) next[r.voice] = name;
                    else delete next[r.voice];
                    setSpeakers(next);
                    setRecognised(recognised.filter((v) => v !== r.voice));
                    api.nameSpeaker(meeting.id, r.voice, name).catch((err) => toast(String(err), true));
                  }}
                />
              ) : (
                <span
                  className={`line-who${s.voice ? " renamable" : ""}${s.voice && recognised.includes(s.voice) ? " recognised" : ""}`}
                  title={
                    s.voice && recognised.includes(s.voice)
                      ? "Recognised from an earlier meeting — click to change"
                      : s.voice
                        ? "Name this speaker (they'll be recognised in later meetings)"
                        : undefined
                  }
                  onClick={(e) => {
                    if (!s.voice) return;
                    e.stopPropagation();
                    setRenaming({ voice: s.voice, name: speakers[s.voice] ?? "" });
                  }}
                >
                  {speakerLabel(s, speakers)}
                </span>
              )}
              <span className="line-text">{s.text}</span>
            </div>
          ))}
        </div>

        <div className="action-group">
          {confirmDelete ? (
            <button className="action danger-text" onClick={() => api.deleteMeeting(meeting.id).then(onClose)}>
              <Icon name="x" size={15} />
              <span className="grow">Really delete the recording and transcript?</span>
            </button>
          ) : (
            <button className="action" onClick={() => setConfirmDelete(true)}>
              <Icon name="x" size={15} />
              <span className="grow">Delete meeting</span>
            </button>
          )}
        </div>
      </div>
    </aside>
  );
}
