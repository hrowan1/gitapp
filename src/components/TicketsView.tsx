import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  api,
  type JiraStatus,
  type JiraTransition,
  type JiraUser,
  type Ticket,
  type TicketDetail,
  type TicketEvent,
} from "../lib/api";
import { setActionDone } from "./MeetingActions";
import { dayKey } from "../lib/day";
import { clock, friendlyDate } from "../lib/time";
import { toast } from "../lib/toast";
import { age } from "../lib/time";
import { Icon, type IconName } from "./Icon";

type Filter = "active" | "all" | "done";

const CATEGORY_TONE: Record<string, string> = { new: "gray", indeterminate: "blue", done: "green" };

/** Remembered preference for the "only assigned to you" filter. */
const MINE_ONLY_KEY = "tickets-mine-only";

/** A colour tone for a Jira status, so "In QA", "In Review", "In Progress" each read at a glance. */
function statusTone(t: { status: string; category: string }): string {
  if (t.category === "done") return "green";
  const s = t.status.toLowerCase();
  if (/block|impediment|hold/.test(s)) return "red";
  if (/\bqa\b|test|verif/.test(s)) return "orange";
  if (/review|\bpr\b|pull request|approv/.test(s)) return "violet";
  if (/progress|doing|develop|\bwip\b|build|implement/.test(s)) return "blue";
  if (/to ?do|backlog|open|triage|selected|\bnew\b|ready|groom|refine/.test(s)) return "gray";
  return CATEGORY_TONE[t.category] ?? "gray";
}

/** Roughly where a status sits in a dev workflow, so groups read To Do → In Progress → Review → QA → Done. */
function statusRank(t: { status: string; category: string }): number {
  if (t.category === "done") return 9;
  const s = t.status.toLowerCase();
  if (t.category === "new" || /to ?do|backlog|open|triage|selected|\bnew\b|groom|refine/.test(s)) return 0;
  if (/progress|doing|develop|\bwip\b|build|implement/.test(s)) return 1;
  if (/review|\bpr\b|pull request|approv/.test(s)) return 2;
  if (/\bqa\b|test|verif/.test(s)) return 3;
  return 4;
}


const EVENT_ICON: Record<string, IconName> = {
  status: "chevronRight",
  assignee: "today",
  resolution: "checkCircle",
  comment: "message",
  mention: "message",
  created: "ticket",
};

function describe(e: TicketEvent): string {
  const who = e.actorIsMe ? "You" : (e.actor ?? "Someone");
  switch (e.kind) {
    case "status":
      return `${who} moved it ${e.detail}`;
    case "assignee":
      if (e.toMe) return `${e.actorIsMe ? "You took it" : `${who} assigned it to you`}`;
      if (e.fromMe) return `${e.actorIsMe ? "You handed it on" : `${who} reassigned it`} · ${e.detail}`;
      return `${who}: ${e.detail}`;
    case "comment":
    case "mention":
      return `${who} ${e.detail.charAt(0).toLowerCase()}${e.detail.slice(1)}`;
    case "created":
      return e.actorIsMe ? "You created it" : "Created";
    default:
      return `${who}: ${e.detail}`;
  }
}

/** Jira tickets that are or were assigned to you, and how they moved. Read-only. */
const INVOLVEMENT: Record<string, string> = {
  was_assigned: "previously yours",
  reported: "you reported it",
  updated: "you worked on it",
};

const STATUS_COLOR: Record<string, string> = { new: "var(--muted)", indeterminate: "var(--blue)", done: "var(--green)" };

/** Close a popover when clicking anywhere else. */
function useDismiss(open: boolean, close: () => void) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => !ref.current?.contains(e.target as Node) && close();
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && close();
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open, close]);
  return ref;
}

/** The ticket's status; click to move it (in Jira) to one of the statuses its workflow allows. */
function StatusPicker({ ticket }: { ticket: Ticket }) {
  const [open, setOpen] = useState(false);
  const [moves, setMoves] = useState<JiraTransition[] | null>(null);
  const [busy, setBusy] = useState(false);
  const close = useCallback(() => setOpen(false), []);
  const ref = useDismiss(open, close);
  const toggle = () => {
    if (open) return setOpen(false);
    setOpen(true);
    setMoves(null);
    api.jiraTransitions(ticket.key).then(setMoves, (e) => {
      setOpen(false);
      toast(String(e), true);
    });
  };
  const move = async (t: JiraTransition) => {
    setOpen(false);
    setBusy(true);
    try {
      await api.jiraMove(ticket.key, t.id);
      toast(`${ticket.key} moved to ${t.to}`);
    } catch (e) {
      toast(String(e), true);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="picker" ref={ref}>
      <button
        className={`chip-state chip-button tone-${statusTone(ticket)}`}
        onClick={toggle}
        disabled={busy}
        title="Move to another status (in Jira)"
      >
        {busy ? "Moving…" : ticket.status}
        <Icon name="chevronDown" size={11} />
      </button>
      {open && (
        <div className="menu picker-menu">
          <div className="menu-label">Move to</div>
          {moves === null && <div className="menu-item muted">Loading…</div>}
          {moves?.length === 0 && <div className="menu-item muted">No moves available from {ticket.status}</div>}
          {moves
            ?.filter((t) => t.to !== ticket.status)
            .map((t) => (
              <button key={t.id} className="menu-item" onClick={() => void move(t)}>
                <span className="status-dot" style={{ background: STATUS_COLOR[t.category] ?? "var(--muted)" }} />
                <span className="grow">{t.to}</span>
                {t.name.toLowerCase() !== t.to.toLowerCase() && <span className="muted small">{t.name}</span>}
              </button>
            ))}
        </div>
      )}
    </div>
  );
}

/** Who has the ticket; click to assign it (in Jira) to you, someone else, or nobody. */
function AssigneePicker({ ticket }: { ticket: Ticket }) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [people, setPeople] = useState<JiraUser[] | null>(null);
  const [busy, setBusy] = useState(false);
  const close = useCallback(() => setOpen(false), []);
  const ref = useDismiss(open, close);
  useEffect(() => {
    if (!open) return;
    setPeople(null);
    const t = setTimeout(() => {
      api.jiraAssignable(ticket.key, query).then(setPeople, (e) => {
        setOpen(false);
        toast(String(e), true);
      });
    }, 200);
    return () => clearTimeout(t);
  }, [open, query, ticket.key]);
  const assign = async (accountId: string | null, name: string) => {
    setOpen(false);
    setBusy(true);
    try {
      await api.jiraAssign(ticket.key, accountId);
      toast(accountId === null ? `${ticket.key} unassigned` : `${ticket.key} assigned to ${name}`);
    } catch (e) {
      toast(String(e), true);
    } finally {
      setBusy(false);
    }
  };
  const label = ticket.assignedToMe ? "Assigned to you" : ticket.assignee ? `With ${ticket.assignee}` : "Unassigned";
  return (
    <div className="picker" ref={ref}>
      <button className="link-button" onClick={() => setOpen(!open)} disabled={busy} title="Assign to someone (in Jira)">
        <Icon name="today" size={12} />
        {busy ? "Assigning…" : label}
        <Icon name="chevronDown" size={11} />
      </button>
      {open && (
        <div className="menu picker-menu">
          {!ticket.assignedToMe && (
            <button className="menu-item" onClick={() => void assign("me", "you")}>
              <Icon name="check" size={13} /> <span className="grow">Assign to me</span>
            </button>
          )}
          <div className="menu-input">
            <input autoFocus placeholder="Find someone…" value={query} onChange={(e) => setQuery(e.target.value)} />
          </div>
          {people === null && <div className="menu-item muted">Loading…</div>}
          {people?.length === 0 && <div className="menu-item muted">Nobody matches</div>}
          {people?.slice(0, 8).map((p) => (
            <button key={p.accountId} className="menu-item" onClick={() => void assign(p.accountId, p.name)}>
              {p.avatar ? <img className="avatar xs" src={p.avatar} alt="" /> : <Icon name="today" size={13} />}
              <span className="grow">{p.name}</span>
            </button>
          ))}
          {ticket.assignee && (
            <button className="menu-item" onClick={() => void assign(null, "")}>
              <Icon name="x" size={13} /> <span className="grow">Unassign</span>
            </button>
          )}
        </div>
      )}
    </div>
  );
}

/** Meetings linked to the ticket in this app (Jira isn't touched), with their notes and your action items. */
function TicketMeetings({ ticketKey, detail, onOpen }: { ticketKey: string; detail: TicketDetail; onOpen: (id: string) => void }) {
  const link = (id: string, ticket: string | null) => api.linkMeeting(id, ticket).catch((e) => toast(String(e), true));
  if (detail.meetings.length === 0 && detail.linkable.length === 0) return null;
  return (
    <div className="timeline">
      <div className="group-label">Meetings</div>
      {detail.meetings.map((m) => {
        const mine = detail.actions.filter((a) => a.meetingId === m.id && a.owner === "you");
        return (
          <div key={m.id} className="ticket-meeting">
            <div className="tl-row link-row" onClick={() => onOpen(m.id)} title="Open the meeting">
              <Icon name="mic" size={13} />
              <span className="tl-text">{m.title}</span>
              <span className="tl-time">{friendlyDate(m.startedAt)}</span>
              <button
                className="ghost icon-only row-action"
                title="Unlink from this ticket"
                onClick={(e) => {
                  e.stopPropagation();
                  void link(m.id, null);
                }}
              >
                <Icon name="x" size={12} />
              </button>
            </div>
            {m.summary?.overview && <p className="ticket-meeting-notes">{m.summary.overview}</p>}
            {mine.map((a) => (
              <label key={a.id} className={`ticket-meeting-action${a.done ? " done" : ""}`}>
                <input type="checkbox" checked={a.done} onChange={(e) => void setActionDone(a, e.target.checked)} />
                <span>{a.text}</span>
              </label>
            ))}
          </div>
        );
      })}
      {detail.linkable.length > 0 && (
        <select
          className="link-meeting"
          value=""
          onChange={(e) => e.target.value && void link(e.target.value, ticketKey)}
          title="Link a recent meeting to this ticket (only in Dev Inbox; nothing is added to Jira)"
        >
          <option value="">Link a recent meeting…</option>
          {detail.linkable.map((m) => (
            <option key={m.id} value={m.id}>
              {m.title} · {friendlyDate(m.startedAt)}
            </option>
          ))}
        </select>
      )}
    </div>
  );
}

const DESCRIPTION_OPEN = "ticket-description-open";

/** Text with its URLs clickable (they open in the browser). */
function Linkified({ text }: { text: string }) {
  const parts = text.split(/(https?:\/\/[^\s)]+)/g);
  return (
    <>
      {parts.map((p, i) =>
        i % 2 ? (
          <a key={i} href={p} onClick={(e) => (e.preventDefault(), openUrl(p))}>
            {p}
          </a>
        ) : (
          p
        ),
      )}
    </>
  );
}

/** The ticket's description: a short preview, or all of it. Remembers which you prefer. */
function Description({ text }: { text: string }) {
  const [open, setOpen] = useState(() => {
    try {
      return localStorage.getItem(DESCRIPTION_OPEN) === "1";
    } catch {
      return false;
    }
  });
  const toggle = () => {
    setOpen(!open);
    try {
      localStorage.setItem(DESCRIPTION_OPEN, open ? "0" : "1");
    } catch {
      // Not remembered; fine.
    }
  };
  return (
    <div className={`description${open ? " open" : ""}`}>
      <button className="description-head" onClick={toggle} aria-expanded={open}>
        <Icon name="chevronDown" size={13} className={`chevron${open ? "" : " closed"}`} />
        <span className="group-label">Description</span>
      </button>
      <div className="description-body" onClick={open ? undefined : toggle} title={open ? undefined : "Show all"}>
        <Linkified text={text} />
      </div>
    </div>
  );
}

/** One day's movements, newest day first. */
function byDay(events: TicketEvent[]): { day: string; events: TicketEvent[] }[] {
  const out: { day: string; events: TicketEvent[] }[] = [];
  for (const e of events) {
    const day = dayKey(new Date(e.at));
    if (out[out.length - 1]?.day !== day) out.push({ day, events: [] });
    out[out.length - 1].events.push(e);
  }
  return out;
}

export function TicketsView({
  version,
  now,
  target,
  onOpenDay,
  onOpenMeeting,
  onSettings,
}: {
  version: number;
  now: number;
  /** Open this ticket (e.g. clicked in the Day view). */
  target?: { key: string } | null;
  onOpenDay: (day: string) => void;
  onOpenMeeting: (id: string) => void;
  onSettings: () => void;
}) {
  const [status, setStatus] = useState<JiraStatus | null>(null);
  const [tickets, setTickets] = useState<Ticket[] | null>(null);
  const [filter, setFilter] = useState<Filter>("active");
  const [text, setText] = useState("");
  const [mineOnly, setMineOnly] = useState(() => {
    try {
      return localStorage.getItem(MINE_ONLY_KEY) !== "0";
    } catch {
      return true;
    }
  });
  const [project, setProject] = useState<string>("all");
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());
  const [selected, setSelected] = useState<string | null>(null);
  const [detail, setDetail] = useState<TicketDetail | null>(null);
  const [syncing, setSyncing] = useState(false);
  useEffect(() => {
    if (target) {
      setSelected(target.key);
      setFilter("all");
      setText("");
      setProject("all");
      setMineOnly(false);
    }
  }, [target]);

  useEffect(() => {
    api.jiraStatus().then(setStatus).catch(() => setStatus(null));
    api.getTickets().then(setTickets).catch(() => setTickets([]));
  }, [version]);

  // Looking at a ticket settles whatever made it need you (it leaves Today).
  useEffect(() => {
    if (selected) api.markTicketSeen(selected).catch(() => {});
  }, [selected]);

  useEffect(() => {
    if (!selected) return setDetail(null);
    api.getTicket(selected).then(setDetail).catch((e) => toast(String(e), true));
  }, [selected, version]);

  // Everything matching the text / open-done / assigned-to-you filters, before narrowing to a project tab.
  const base = useMemo(() => {
    const q = text.trim().toLowerCase();
    return (tickets ?? []).filter(
      (t) =>
        (filter === "all" || (filter === "done" ? t.category === "done" : t.category !== "done")) &&
        (!mineOnly || t.assignedToMe) &&
        (!q || `${t.key} ${t.summary} ${t.status} ${t.project}`.toLowerCase().includes(q)),
    );
  }, [tickets, filter, mineOnly, text]);

  // The projects present, with a count each, for the tabs.
  const projects = useMemo(() => {
    const counts = new Map<string, number>();
    for (const t of base) counts.set(t.project, (counts.get(t.project) ?? 0) + 1);
    return [...counts.entries()].sort((a, b) => a[0].localeCompare(b[0]));
  }, [base]);

  // A selected project tab that no longer has anything falls back to All.
  useEffect(() => {
    if (project !== "all" && !projects.some(([p]) => p === project)) setProject("all");
  }, [projects, project]);

  const shown = useMemo(
    () => (project === "all" ? base : base.filter((t) => t.project === project)),
    [base, project],
  );

  // Grouped by status, newest within each group, ordered To Do → In Progress → Review → QA → Done.
  const groups = useMemo(() => {
    const by = new Map<string, Ticket[]>();
    for (const t of shown) {
      const arr = by.get(t.status);
      if (arr) arr.push(t);
      else by.set(t.status, [t]);
    }
    const list = [...by.entries()].map(([statusName, items]) => {
      items.sort((a, b) => (b.updated ?? b.lastMoveAt ?? "").localeCompare(a.updated ?? a.lastMoveAt ?? ""));
      return { status: statusName, items, rank: statusRank(items[0]), tone: statusTone(items[0]) };
    });
    list.sort((a, b) => a.rank - b.rank || a.status.localeCompare(b.status));
    return list;
  }, [shown]);

  const toggleGroup = useCallback((statusName: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(statusName)) next.delete(statusName);
      else next.add(statusName);
      return next;
    });
  }, []);

  const renderRow = (t: Ticket) => (
    <div
      key={t.key}
      className={`row${selected === t.key ? " selected" : ""}`}
      role="button"
      onClick={() => setSelected(t.key)}
      onDoubleClick={() => openUrl(t.url)}
    >
      <span className={`state-icon tone-${statusTone(t)}`}>
        <Icon name={t.category === "done" ? "checkCircle" : "ticket"} size={14} />
      </span>
      <div className="row-main">
        <div className="row-title">
          <span className="ticket-key">{t.key}</span>
          <span className="title">{t.summary}</span>
        </div>
        <div className="row-sub">
          <span className="reason">{t.lastMove ?? t.issueType}</span>
          {INVOLVEMENT[t.involvement] && <span className="pill">{INVOLVEMENT[t.involvement]}</span>}
        </div>
      </div>
      <div className="row-right">
        {/* The list is sorted by last update, so show that rather than the last status move. */}
        <span className="age" title={t.updated ? `Updated ${friendlyDate(t.updated)} ${clock(t.updated)}` : undefined}>
          {t.updated ? age(t.updated, now) : t.lastMoveAt ? age(t.lastMoveAt, now) : ""}
        </span>
      </div>
    </div>
  );

  const ticket = tickets?.find((t) => t.key === selected) ?? null;

  if (status && !status.connected) {
    return (
      <div className="page">
        <div className="headline compact">
          <h1>Tickets</h1>
        </div>
        <div className="empty big">
          <Icon name="ticket" size={26} />
          <p className="strong">Connect Jira to see your tickets</p>
          <p className="muted small">
            Read-only: the tickets that involve you — assigned to you now or before, reported by you, or moved or
            commented on by you — and how they moved, so your Day view shows the whole picture.
          </p>
          <button className="primary" onClick={onSettings}>
            Connect Jira in Settings
          </button>
        </div>
      </div>
    );
  }

  const header = (
    <>
      <div className="headline compact">
        <h1>Tickets</h1>
        <p className="muted">
          {mineOnly ? "Assigned to you" : "Assigned to you, reported by you or worked on by you"}{status?.site ? ` · ${status.site.replace("https://", "")}` : ""}
          {status?.lastSync && ` · synced ${age(status.lastSync, now)} ago`}{" "}
          <button
            className="link"
            disabled={syncing}
            onClick={async () => {
              setSyncing(true);
              try {
                await api.jiraSyncNow();
              } catch (e) {
                toast(String(e), true);
              } finally {
                setSyncing(false);
              }
            }}
          >
            {syncing ? "syncing…" : "sync now"}
          </button>
        </p>
        {status?.error && <p className="error small">Last Jira sync failed: {status.error}</p>}
      </div>
      {projects.length > 1 && (
        <div className="tabs" role="tablist">
          <button className={`tab${project === "all" ? " on" : ""}`} onClick={() => setProject("all")}>
            All <span className="count">{base.length}</span>
          </button>
          {projects.map(([p, n]) => (
            <button key={p} className={`tab${project === p ? " on" : ""}`} onClick={() => setProject(p)}>
              {p} <span className="count">{n}</span>
            </button>
          ))}
        </div>
      )}
      <div className="filters">
        <div className="search-box">
          <Icon name="search" size={14} />
          <input placeholder="Filter by key, title or status" value={text} onChange={(e) => setText(e.target.value)} />
        </div>
        <div className="segmented">
          {(["active", "done", "all"] as Filter[]).map((f) => (
            <button key={f} className={filter === f ? "on" : ""} onClick={() => setFilter(f)}>
              {f === "active" ? "Open" : f === "done" ? "Done" : "All"}
            </button>
          ))}
        </div>
        <label className="check">
          <input
            type="checkbox"
            checked={mineOnly}
            onChange={(e) => {
              setMineOnly(e.target.checked);
              try {
                localStorage.setItem(MINE_ONLY_KEY, e.target.checked ? "1" : "0");
              } catch {
                // Preference not remembered; fine.
              }
            }}
          />{" "}
          Only assigned to you
        </label>
      </div>
    </>
  );

  return (
    <div className={`workspace${ticket ? " has-selection" : ""}`}>
      <section className="pane-list">
        <div className="pane-header">{header}</div>
        <div className="pane-scroll">
          {tickets === null && <div className="empty">Loading…</div>}
          {tickets && shown.length === 0 && (
            <div className="empty">
              {tickets.length === 0 ? "No tickets yet — the first Jira sync may still be running." : "Nothing matches."}
            </div>
          )}
          {groups.map((g) => {
            const isCollapsed = collapsed.has(g.status);
            return (
              <div className="status-group" key={g.status}>
                <button
                  className="status-head"
                  onClick={() => toggleGroup(g.status)}
                  aria-expanded={!isCollapsed}
                  title={isCollapsed ? "Show these" : "Hide these"}
                >
                  <Icon name="chevronDown" size={13} className={`chevron${isCollapsed ? " closed" : ""}`} />
                  <span className={`group-dot tone-${g.tone}`} />
                  <span className="status-name">{g.status}</span>
                  <span className="count">{g.items.length}</span>
                </button>
                {!isCollapsed && g.items.map(renderRow)}
              </div>
            );
          })}
        </div>
      </section>

      {ticket ? (
        <aside className="detail">
          <button className="detail-close ghost" onClick={() => setSelected(null)} title="Close">
            <Icon name="x" />
          </button>
          <div className="detail-body">
            <div className="detail-meta">
              <Icon name="ticket" size={14} />
              <span>{ticket.key}</span>
              <span className="sep">·</span>
              <span>{ticket.issueType}</span>
              {ticket.priority && (
                <>
                  <span className="sep">·</span>
                  <span>{ticket.priority}</span>
                </>
              )}
            </div>
            <h2 className="detail-title">{ticket.summary}</h2>
            <div className="detail-state">
              <StatusPicker ticket={ticket} />
              <AssigneePicker ticket={ticket} />
            </div>
            <div className="action-group">
              <button className="action primary" onClick={() => openUrl(ticket.url)}>
                <Icon name="external" size={15} />
                <span className="grow">Open in Jira</span>
              </button>
            </div>

            {detail?.description && <Description text={detail.description} />}

            {detail && <TicketMeetings ticketKey={ticket.key} detail={detail} onOpen={onOpenMeeting} />}

            {detail && detail.prs.length > 0 && (
              <div className="timeline">
                <div className="group-label">Pull requests</div>
                {detail.prs.map((p) => (
                  <div key={p.id} className="tl-row link-row" onClick={() => openUrl(p.url)} title="Open on GitHub">
                    <Icon name={p.open ? "pr" : "merge"} size={13} />
                    <span className="tl-text">{p.title}</span>
                    <span className="tl-time">
                      {p.repo.split("/")[1]}#{p.number}
                    </span>
                  </div>
                ))}
              </div>
            )}

            <div className="timeline">
              <div className="group-label">How it moved</div>
              {!detail && <div className="muted small">Loading…</div>}
              {detail?.events.length === 0 && <div className="muted small">No movement recorded in the last 30 days.</div>}
              {detail &&
                byDay(detail.events).map(({ day, events }) => (
                  <div key={day} className="tl-day">
                    <button className="tl-day-head link-row" onClick={() => onOpenDay(day)} title="See this day in the Day view">
                      <span>{friendlyDate(events[0].at)}</span>
                      <span className="muted small">open day</span>
                      <Icon name="chevronRight" size={12} />
                    </button>
                    {events.map((e, i) => (
                      <div key={i} className={`tl-row${e.actorIsMe ? " mine" : ""}`}>
                        <Icon name={EVENT_ICON[e.kind] ?? "clock"} size={13} />
                        <span className="tl-text">{describe(e)}</span>
                        <span className="tl-time">{clock(e.at)}</span>
                      </div>
                    ))}
                  </div>
                ))}
            </div>
          </div>
        </aside>
      ) : (
        <aside className="detail empty-detail">
          <div className="empty-art">
            <Icon name="ticket" size={26} />
          </div>
          <p className="strong">Select a ticket</p>
          <p className="muted small">See how it moved, which PRs belong to it, and jump to the days you worked on it.</p>
        </aside>
      )}
    </div>
  );
}
