import { useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { api, type Meeting, type MeetingAction, type MeetingSummary, type RecordingStatus, type Segment } from "../lib/api";
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
  /** Open this meeting (e.g. clicked in the Day view). */
  target?: { id: string } | null;
  onOpenDay: (day: string) => void;
}) {
  const [meetings, setMeetings] = useState<Meeting[] | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [title, setTitle] = useState("");
  const [busy, setBusy] = useState(false);
  const [levels, setLevels] = useState({ mic: 0, system: 0, seconds: 0 });
  const [warnings, setWarnings] = useState<string[]>([]);
  useEffect(() => {
    if (target) setSelected(target.id);
  }, [target]);

  useEffect(() => {
    api.listMeetings().then(setMeetings).catch((e) => toast(String(e), true));
  }, [version]);

  useEffect(() => {
    const subs = [
      listen<{ mic: number; system: number; seconds: number }>("recording-level", (e) => setLevels(e.payload)),
      listen<{ event: string; message?: string }>("recording-event", (e) => {
        if (e.payload.event === "warning" && e.payload.message) setWarnings((w) => [...w, e.payload.message!]);
        if (e.payload.event === "error") toast(e.payload.message ?? "Recording failed", true);
      }),
    ];
    return () => subs.forEach((p) => p.then((f) => f()));
  }, []);

  useEffect(() => {
    if (!recording.recording) setLevels({ mic: 0, system: 0, seconds: 0 });
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

  return (
    <div className={`workspace${meeting ? " has-selection" : ""}`}>
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
        </div>

        <div className="pane-scroll">
          {meetings?.length === 0 && (
            <div className="empty">No meetings yet. Press Record when one starts — your mic and the call's audio are saved.</div>
          )}
          {meetings?.map((m) => (
            <div
              key={m.id}
              className={`row${selected === m.id ? " selected" : ""}`}
              role="button"
              onClick={() => setSelected(m.id)}
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
        <MeetingDetail key={meeting.id} meeting={meeting} version={version} onClose={() => setSelected(null)} onOpenDay={onOpenDay} />
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

const OWNER: Record<string, string> = { you: "You", others: "Others", unclear: "—" };

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
  const who = meeting.summaryBy === "claude" ? "Claude" : "Apple Intelligence on this Mac";
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

function MeetingDetail({
  meeting,
  version,
  onClose,
  onOpenDay,
}: {
  meeting: Meeting;
  version: number;
  onClose: () => void;
  onOpenDay: (day: string) => void;
}) {
  const [segments, setSegments] = useState<Segment[] | null>(null);
  const [actions, setActions] = useState<MeetingAction[]>([]);
  const [title, setTitle] = useState(meeting.title);
  const [ticket, setTicket] = useState(meeting.ticket ?? "");
  const [find, setFind] = useState("");
  const [playing, setPlaying] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const mic = useRef<HTMLAudioElement>(null);
  const system = useRef<HTMLAudioElement>(null);

  useEffect(() => {
    api
      .getMeeting(meeting.id)
      .then((d) => {
        setSegments(d.segments);
        setActions(d.actions);
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
    const text = segments.map((s) => `[${mmss(s.start)}] ${s.speaker === "you" ? "You" : "Others"}: ${s.text}`).join("\n");
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
      <div className="detail-body">
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
              className={`line speaker-${s.speaker}`}
              onClick={() => meeting.hasAudio && seek(s.start)}
              title={meeting.hasAudio ? "Play from here" : undefined}
            >
              <span className="line-time">{mmss(s.start)}</span>
              <span className="line-who">{s.speaker === "you" ? "You" : "Others"}</span>
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
