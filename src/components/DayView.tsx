import { useCallback, useEffect, useMemo, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, type DayEvent, type DayReport, type Meeting, type TicketMove, type TimeBlock } from "../lib/api";
import {
  addDays,
  dayBounds,
  dayKey,
  duration,
  eventSummary,
  hhmm,
  hueFor,
  longDate,
  parseDay,
  summaryText,
  ticketOf,
  weekOf,
  withTime,
} from "../lib/day";
import { toast } from "../lib/toast";
import { Icon, type IconName } from "./Icon";

const PX_PER_HOUR = 54;

const EVENT_ICON: Record<string, IconName> = {
  committed: "code",
  local_commit: "code",
  pushed: "branch",
  reviewed: "eye",
  commented: "message",
  opened: "pr",
  merged: "merge",
  edit: "pencil",
  jira: "ticket",
  meeting: "mic",
};

const MOVE_ICON: Record<string, IconName> = {
  status: "chevronRight",
  assignee: "today",
  resolution: "checkCircle",
  comment: "message",
  created: "ticket",
};
const CATEGORY_TONE: Record<string, string> = { new: "gray", indeterminate: "blue", done: "green" };

function describeMove(m: TicketMove): string {
  const who = m.actorIsMe ? "You" : (m.actor ?? "Someone");
  switch (m.kind) {
    case "status":
      return `${who} moved it ${m.detail}`;
    case "created":
      return `${who} created it`;
    case "comment":
      return `${who} ${m.detail.charAt(0).toLowerCase()}${m.detail.slice(1)}`;
    case "assignee": {
      const to = m.detail.match(/^Assigned to (.+)$/)?.[1];
      if (!to) return `${who} unassigned it`;
      if (m.toMe) return m.actorIsMe ? "You took it" : `${who} assigned it to you`;
      return `${who} ${m.actorIsMe ? "handed it to" : "assigned it to"} ${to}`;
    }
    default:
      return `${who}: ${m.detail}`;
  }
}

/** Moves grouped by ticket, in the order each ticket first moved that day. */
function movesByTicket(moves: TicketMove[]): TicketMove[][] {
  const groups = new Map<string, TicketMove[]>();
  for (const m of moves) {
    if (!groups.has(m.key)) groups.set(m.key, []);
    groups.get(m.key)!.push(m);
  }
  return [...groups.values()];
}

/** What you worked on, for how long, on any day; editable, and shaped for Jira worklogs. */
export function DayView({
  version,
  target,
  onOpenTicket,
  onOpenMeeting,
}: {
  version: number;
  target?: { day: string } | null;
  onOpenTicket: (key: string) => void;
  onOpenMeeting: (id: string) => void;
}) {
  const today = dayKey(new Date());
  const [day, setDay] = useState(target?.day ?? today);
  // Jump to a requested date (e.g. clicked in a ticket's history).
  useEffect(() => {
    if (target) setDay(target.day);
  }, [target]);
  const [reports, setReports] = useState<Record<string, DayReport>>({});
  const [editing, setEditing] = useState<{ block: TimeBlock; index: number | null } | null>(null);
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const week = useMemo(() => weekOf(day), [day]);

  const load = useCallback(async (days: string[]) => {
    const loaded = await Promise.all(
      days.map(async (d) => {
        const { start, end } = dayBounds(d);
        try {
          return [d, await api.getDay(d, start, end)] as const;
        } catch (e) {
          toast(String(e), true);
          return null;
        }
      }),
    );
    setReports((prev) => {
      const next = { ...prev };
      for (const r of loaded) if (r) next[r[0]] = r[1];
      return next;
    });
  }, []);

  useEffect(() => {
    load(week.filter((d) => d <= today));
  }, [week, today, version, load]);

  const report = reports[day];
  const maxWeek = Math.max(60, ...week.map((d) => reports[d]?.totalMinutes ?? 0));

  const saveBlocks = async (blocks: TimeBlock[]) => {
    try {
      await api.saveDay(day, blocks);
      await load([day]);
    } catch (e) {
      toast(String(e), true);
    }
  };

  const copy = async () => {
    if (!report) return;
    try {
      await navigator.clipboard.writeText(summaryText(report));
      toast("Day summary copied");
    } catch {
      toast("Couldn't copy to the clipboard", true);
    }
  };

  const addBlock = () => {
    const last = report?.blocks[report.blocks.length - 1];
    const start = last ? last.end : withTime(day, "09:00");
    const end = new Date(new Date(start).getTime() + 60 * 60_000).toISOString();
    const first = report?.groups[0];
    setEditing({
      block: { key: first?.key ?? `M:${Date.now()}`, label: first?.label ?? "", start, end, minutes: 60 },
      index: null,
    });
  };

  return (
    <div className="page day-page">
      <div className="day-header">
        <div className="headline compact">
          <h1>{day === today ? "Today" : longDate(day)}</h1>
          <p className="muted">
            {report ? (
              <>
                <span className="strong total">{duration(report.totalMinutes)}</span> tracked
                {report.groups.length > 0 && ` across ${report.groups.length} ${report.groups.length === 1 ? "thing" : "things"}`}
                {day === today && " so far"}
              </>
            ) : (
              "Loading…"
            )}
          </p>
        </div>
        <div className="day-nav">
          <button className="ghost icon-only" onClick={() => setDay(addDays(day, -1))} title="Previous day">
            <Icon name="chevronRight" className="flip" />
          </button>
          <button onClick={() => setDay(today)} disabled={day === today}>
            Today
          </button>
          <button
            className="ghost icon-only"
            onClick={() => setDay(addDays(day, 1))}
            disabled={day >= today}
            title="Next day"
          >
            <Icon name="chevronRight" />
          </button>
          <input type="date" value={day} max={today} onChange={(e) => e.target.value && setDay(e.target.value)} />
        </div>
      </div>

      <div className="week-strip">
        {week.map((d) => {
          const r = reports[d];
          const future = d > today;
          return (
            <button
              key={d}
              className={`week-day${d === day ? " active" : ""}`}
              disabled={future}
              onClick={() => setDay(d)}
            >
              <span className="wd-name">{parseDay(d).toLocaleDateString(undefined, { weekday: "short" })}</span>
              <span className="wd-date">{parseDay(d).getDate()}</span>
              <span className="wd-bar">
                <span style={{ height: `${Math.round(((r?.totalMinutes ?? 0) / maxWeek) * 100)}%` }} />
              </span>
              <span className="wd-total">{future ? "" : r ? duration(r.totalMinutes) : "…"}</span>
            </button>
          );
        })}
        <div className="week-total">
          <span className="muted small">Week</span>
          <span className="strong">{duration(week.reduce((sum, d) => sum + (reports[d]?.totalMinutes ?? 0), 0))}</span>
        </div>
      </div>

      <div className="day-toolbar">
        {report?.frozen && (
          <span className="chip-state tone-violet" title="You edited this day, so it no longer updates automatically">
            <Icon name="pencil" size={12} /> Edited by you
          </span>
        )}
        <span className="grow" />
        {report?.frozen && (
          <button
            className="ghost"
            onClick={async () => {
              await api.resetDay(day);
              await load([day]);
              toast("Back to automatic");
            }}
          >
            <Icon name="undo" size={14} /> Reset to automatic
          </button>
        )}
        <button onClick={addBlock}>
          <Icon name="clock" size={14} /> Add time
        </button>
        <button onClick={copy} disabled={!report}>
          <Icon name="copy" size={14} /> Copy summary
        </button>
      </div>

      {report &&
      report.blocks.length === 0 &&
      report.groups.length === 0 &&
      report.ticketMoves.length === 0 &&
      report.meetings.length === 0 ? (
        <div className="empty big">
          <Icon name="calendar" size={24} />
          <p className="strong">Nothing tracked on this day</p>
          <p className="muted small">
            Time comes from your commits, pushes, reviews and comments, plus local editing in your clones (Settings →
            Time tracking). You can also add time by hand.
          </p>
        </div>
      ) : (
        report && (
          <div className="day-grid">
            <Timeline report={report} isToday={day === today} onEdit={(block, index) => setEditing({ block, index })} />
            <div className="day-groups">
              {report.groups.map((g) => {
                const blocks = report.blocks.filter((b) => b.key === g.key);
                const open = expanded[g.key] ?? false;
                const ticket = ticketOf(g.key);
                return (
                  <div key={g.key} className="day-group" style={{ "--h": hueFor(g.key) } as React.CSSProperties}>
                    <div className="dg-head" onClick={() => setExpanded({ ...expanded, [g.key]: !open })}>
                      <span className="swatch" />
                      <div className="grow">
                        <div className="dg-title">
                          {ticket && <span className="ticket">{ticket}</span>}
                          <span className="ellipsis">{ticket ? g.label.replace(`${ticket} · `, "") : g.label}</span>
                          {g.status && <span className="dg-status">{g.status}</span>}
                        </div>
                        <div className="muted small">
                          {blocks.map((b) => `${hhmm(b.start)}–${hhmm(b.end)}`).join(", ") || "No time recorded"}
                          {g.events.length > 0 && ` · ${eventSummary(g.events.map((e) => e.kind))}`}
                        </div>
                      </div>
                      <span className="dg-total">{duration(g.minutes)}</span>
                      <Icon name="chevronDown" size={14} className={`chevron${open ? "" : " closed"}`} />
                    </div>
                    {open && (
                      <div className="dg-events">
                        {g.url && (
                          <button className="link small" onClick={() => openUrl(g.url!)}>
                            Open on GitHub
                          </button>
                        )}
                        {collapseEdits(g.events).map((e, i) => (
                          <div
                            key={i}
                            className={`dg-event${e.url ? " link-row" : ""}`}
                            onClick={() => e.url && openUrl(e.url)}
                          >
                            <span className="tl-time">{e.until ? `${hhmm(e.at)}–${hhmm(e.until)}` : hhmm(e.at)}</span>
                            <Icon name={EVENT_ICON[e.kind] ?? "clock"} size={13} />
                            <span className="grow">{e.text}</span>
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                );
              })}
              <DayMeetings meetings={report.meetings} onOpen={onOpenMeeting} />
              <TicketMoves moves={report.ticketMoves} onOpen={onOpenTicket} />
            </div>
          </div>
        )
      )}

      {editing && report && (
        <BlockEditor
          day={day}
          block={editing.block}
          isNew={editing.index === null}
          groups={report.groups.map((g) => ({ key: g.key, label: g.label }))}
          onCancel={() => setEditing(null)}
          onDelete={() => {
            const blocks = report.blocks.filter((_, i) => i !== editing.index);
            setEditing(null);
            saveBlocks(blocks);
          }}
          onSave={(b) => {
            const blocks = [...report.blocks];
            if (editing.index === null) blocks.push(b);
            else blocks[editing.index] = b;
            setEditing(null);
            saveBlocks(blocks);
          }}
        />
      )}
    </div>
  );
}

/** The day's recorded meetings; click one to read its transcript. */
function DayMeetings({ meetings, onOpen }: { meetings: Meeting[]; onOpen: (id: string) => void }) {
  if (meetings.length === 0) return null;
  return (
    <section className="day-section">
      <div className="group-label">Meetings</div>
      {meetings.map((m) => (
        <div
          key={m.id}
          className="day-item link-row"
          style={{ "--h": hueFor(m.ticket ? `T:${m.ticket}` : `MTG:${m.id}`) } as React.CSSProperties}
          onClick={() => onOpen(m.id)}
          title="Open the transcript"
        >
          <Icon name="mic" size={14} />
          <div className="grow">
            <div className="dg-title">
              {m.ticket && !m.title.toUpperCase().includes(m.ticket) && <span className="ticket">{m.ticket}</span>}
              <span className="ellipsis">{m.title}</span>
            </div>
            <div className="muted small ellipsis">
              {m.endedAt ? `${hhmm(m.startedAt)}–${hhmm(m.endedAt)}` : `${hhmm(m.startedAt)} · recording`}
              {m.status === "transcribing" && " · transcribing…"}
              {m.status === "failed" && " · transcription failed"}
              {m.status === "done" && (m.preview ? ` · ${m.preview}` : " · no speech found")}
            </div>
          </div>
          <Icon name="chevronRight" size={13} />
        </div>
      ))}
    </section>
  );
}

/** How the tickets that involve you moved that day — yours and other people's moves. */
function TicketMoves({ moves, onOpen }: { moves: TicketMove[]; onOpen: (key: string) => void }) {
  if (moves.length === 0) return null;
  return (
    <section className="day-section">
      <div className="group-label">Ticket moves</div>
      {movesByTicket(moves).map((ms) => {
        const t = ms[0];
        return (
          <div key={t.key} className="move-group" style={{ "--h": hueFor(`T:${t.key}`) } as React.CSSProperties}>
            <div className="day-item link-row" onClick={() => onOpen(t.key)} title="Open in Tickets">
              <span className="ticket">{t.key}</span>
              <span className="grow ellipsis">{t.summary}</span>
              <span className={`chip-state tone-${CATEGORY_TONE[t.category] ?? "gray"}`}>{t.status}</span>
            </div>
            {ms.map((m, i) => (
              <div key={i} className={`dg-event${m.actorIsMe ? " mine" : " theirs"}`}>
                <span className="tl-time">{hhmm(m.at)}</span>
                <Icon name={MOVE_ICON[m.kind] ?? "clock"} size={13} />
                <span className="grow">{describeMove(m)}</span>
              </div>
            ))}
          </div>
        );
      })}
    </section>
  );
}

/** Runs of local-edit samples on the same branch read as one line ("Edited files · 13:35–14:20"). */
function collapseEdits(events: DayEvent[]): (DayEvent & { until?: string })[] {
  const out: (DayEvent & { until?: string })[] = [];
  for (const e of events) {
    const prev = out[out.length - 1];
    if (e.kind === "edit" && prev?.kind === "edit" && prev.branch === e.branch) prev.until = e.at;
    else out.push({ ...e });
  }
  return out;
}

/** Blocks laid out on an hour grid; overlapping blocks share the width. */
function Timeline({
  report,
  isToday,
  onEdit,
}: {
  report: DayReport;
  isToday: boolean;
  onEdit: (block: TimeBlock, index: number) => void;
}) {
  const hours = report.blocks.flatMap((b) => [new Date(b.start).getHours(), new Date(b.end).getHours() + 1]);
  const from = Math.max(0, Math.min(8, ...hours));
  const to = Math.min(24, Math.max(18, ...hours));
  const minutesFrom = (iso: string) => {
    const d = new Date(iso);
    return (d.getHours() - from) * 60 + d.getMinutes();
  };

  // Lanes only where blocks actually overlap: each cluster of overlapping blocks shares the width.
  const lanes: number[] = [];
  const laneCounts: number[] = [];
  const order = report.blocks.map((b, i) => [b, i] as const).sort((a, b) => a[0].start.localeCompare(b[0].start));
  let cluster: number[] = [];
  let clusterEnd = "";
  let laneEnds: string[] = [];
  const closeCluster = () => {
    for (const i of cluster) laneCounts[i] = Math.max(1, laneEnds.length);
    cluster = [];
    laneEnds = [];
  };
  for (const [b, i] of order) {
    if (cluster.length && b.start >= clusterEnd) closeCluster();
    let lane = laneEnds.findIndex((end) => end <= b.start);
    if (lane === -1) {
      lane = laneEnds.length;
      laneEnds.push(b.end);
    } else laneEnds[lane] = b.end;
    lanes[i] = lane;
    cluster.push(i);
    // A new cluster starts at this block's end; otherwise the cluster runs to its latest end.
    clusterEnd = cluster.length === 1 || b.end > clusterEnd ? b.end : clusterEnd;
  }
  closeCluster();
  const now = new Date();

  return (
    <div className="timeline-grid" style={{ height: (to - from) * PX_PER_HOUR }}>
      {Array.from({ length: to - from + 1 }, (_, i) => (
        <div key={i} className="hour-line" style={{ top: i * PX_PER_HOUR }}>
          <span>{String(from + i).padStart(2, "0")}:00</span>
        </div>
      ))}
      {isToday && now.getHours() >= from && now.getHours() < to && (
        <div className="now-line" style={{ top: ((now.getHours() - from) * 60 + now.getMinutes()) * (PX_PER_HOUR / 60) }} />
      )}
      <div className="blocks">
        {report.blocks.map((b, i) => {
          const top = minutesFrom(b.start) * (PX_PER_HOUR / 60);
          const height = Math.max(18, b.minutes * (PX_PER_HOUR / 60));
          const ticket = ticketOf(b.key);
          return (
            <button
              key={i}
              className="time-block"
              title={`${b.label}\n${hhmm(b.start)}–${hhmm(b.end)} · ${duration(b.minutes)}${b.note ? `\n${b.note}` : ""}\nClick to edit`}
              style={
                {
                  top,
                  height,
                  left: `calc(${(lanes[i] / laneCounts[i]) * 100}% + 2px)`,
                  width: `calc(${100 / laneCounts[i]}% - 4px)`,
                  "--h": hueFor(b.key),
                } as React.CSSProperties
              }
              onClick={() => onEdit(b, i)}
            >
              <span className="tb-title ellipsis">{ticket ?? b.label}</span>
              {height > 34 && (
                <span className="tb-meta ellipsis">
                  {hhmm(b.start)}–{hhmm(b.end)} · {duration(b.minutes)}
                </span>
              )}
            </button>
          );
        })}
      </div>
    </div>
  );
}

function BlockEditor({
  day,
  block,
  isNew,
  groups,
  onSave,
  onDelete,
  onCancel,
}: {
  day: string;
  block: TimeBlock;
  isNew: boolean;
  groups: { key: string; label: string }[];
  onSave: (b: TimeBlock) => void;
  onDelete: () => void;
  onCancel: () => void;
}) {
  const known = groups.some((g) => g.key === block.key);
  const [key, setKey] = useState(known ? block.key : "__other");
  const [label, setLabel] = useState(known ? "" : block.label);
  const [from, setFrom] = useState(hhmm(block.start));
  const [to, setTo] = useState(hhmm(block.end));
  const [note, setNote] = useState(block.note ?? "");
  const start = withTime(day, from);
  const end = withTime(day, to);
  const minutes = Math.round((new Date(end).getTime() - new Date(start).getTime()) / 60_000);
  const valid = minutes > 0 && (key !== "__other" || label.trim().length > 0);

  const save = () => {
    if (!valid) return;
    const group = groups.find((g) => g.key === key);
    // Something typed by hand: use a ticket key if it looks like one, so it groups with that ticket.
    const typedTicket = label.trim().match(/^([A-Z][A-Z0-9]+-\d+)\b/)?.[1];
    onSave({
      id: block.id ?? null,
      key: group ? group.key : typedTicket ? `T:${typedTicket}` : `M:${label.trim().toLowerCase()}`,
      label: group ? group.label : label.trim(),
      start,
      end,
      minutes,
      note: note.trim() || null,
    });
  };

  return (
    <div className="overlay" onMouseDown={onCancel}>
      <div
        className="dialog"
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Escape") onCancel();
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) save();
        }}
      >
        <div className="dialog-head">
          <span className="strong">{isNew ? "Add time" : "Edit time"}</span>
          <span className="muted small">Editing a day stops it updating automatically</span>
        </div>
        <label className="field">
          <span>What</span>
          <select className="grow" value={key} onChange={(e) => setKey(e.target.value)}>
            {groups.map((g) => (
              <option key={g.key} value={g.key}>
                {g.label}
              </option>
            ))}
            <option value="__other">Something else…</option>
          </select>
        </label>
        {key === "__other" && (
          <label className="field">
            <span>Name</span>
            <input
              className="grow"
              autoFocus
              placeholder="e.g. IO-2650 planning, or Team meeting"
              value={label}
              onChange={(e) => setLabel(e.target.value)}
            />
          </label>
        )}
        <label className="field">
          <span>From – to</span>
          <input type="time" value={from} onChange={(e) => setFrom(e.target.value)} />
          <span className="muted">–</span>
          <input type="time" value={to} onChange={(e) => setTo(e.target.value)} />
          <span className="muted small">{minutes > 0 ? duration(minutes) : "ends before it starts"}</span>
        </label>
        <label className="field top">
          <span>Note</span>
          <textarea
            className="grow composer"
            rows={2}
            placeholder="What you did (goes into the summary and, later, the Jira worklog)"
            value={note}
            onChange={(e) => setNote(e.target.value)}
          />
        </label>
        <div className="dialog-foot">
          {!isNew && (
            <button className="danger-outline" onClick={onDelete}>
              Delete
            </button>
          )}
          <span className="grow" />
          <button onClick={onCancel}>Cancel</button>
          <button className="primary" disabled={!valid} onClick={save}>
            Save
          </button>
        </div>
      </div>
    </div>
  );
}
