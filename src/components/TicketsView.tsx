import { useEffect, useMemo, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, type JiraStatus, type Ticket, type TicketDetail, type TicketEvent } from "../lib/api";
import { dayKey } from "../lib/day";
import { clock, friendlyDate } from "../lib/time";
import { toast } from "../lib/toast";
import { age } from "../lib/time";
import { Icon, type IconName } from "./Icon";

type Filter = "active" | "all" | "done";

const CATEGORY_TONE: Record<string, string> = { new: "gray", indeterminate: "blue", done: "green" };

function StatusChip({ t }: { t: { status: string; category: string } }) {
  return <span className={`chip-state tone-${CATEGORY_TONE[t.category] ?? "gray"}`}>{t.status}</span>;
}

const EVENT_ICON: Record<string, IconName> = {
  status: "chevronRight",
  assignee: "today",
  resolution: "checkCircle",
  comment: "message",
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
  onSettings,
}: {
  version: number;
  now: number;
  /** Open this ticket (e.g. clicked in the Day view). */
  target?: { key: string } | null;
  onOpenDay: (day: string) => void;
  onSettings: () => void;
}) {
  const [status, setStatus] = useState<JiraStatus | null>(null);
  const [tickets, setTickets] = useState<Ticket[] | null>(null);
  const [filter, setFilter] = useState<Filter>("active");
  const [text, setText] = useState("");
  const [mineOnly, setMineOnly] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [detail, setDetail] = useState<TicketDetail | null>(null);
  const [syncing, setSyncing] = useState(false);
  useEffect(() => {
    if (target) {
      setSelected(target.key);
      setFilter("all");
      setText("");
    }
  }, [target]);

  useEffect(() => {
    api.jiraStatus().then(setStatus).catch(() => setStatus(null));
    api.getTickets().then(setTickets).catch(() => setTickets([]));
  }, [version]);

  useEffect(() => {
    if (!selected) return setDetail(null);
    api.getTicket(selected).then(setDetail).catch((e) => toast(String(e), true));
  }, [selected, version]);

  const shown = useMemo(() => {
    const q = text.trim().toLowerCase();
    return (tickets ?? []).filter(
      (t) =>
        (filter === "all" || (filter === "done" ? t.category === "done" : t.category !== "done")) &&
        (!mineOnly || t.assignedToMe) &&
        (!q || `${t.key} ${t.summary} ${t.status} ${t.project}`.toLowerCase().includes(q)),
    );
  }, [tickets, filter, mineOnly, text]);
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
          <input type="checkbox" checked={mineOnly} onChange={(e) => setMineOnly(e.target.checked)} /> Only assigned to
          you
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
          {shown.map((t) => (
            <div
              key={t.key}
              className={`row${selected === t.key ? " selected" : ""}`}
              role="button"
              onClick={() => setSelected(t.key)}
              onDoubleClick={() => openUrl(t.url)}
            >
              <span className={`state-icon tone-${CATEGORY_TONE[t.category] ?? "gray"}`}>
                <Icon name={t.category === "done" ? "checkCircle" : "ticket"} size={14} />
              </span>
              <div className="row-main">
                <div className="row-title">
                  <span className="ticket-key">{t.key}</span>
                  <span className="title">{t.summary}</span>
                </div>
                <div className="row-sub">
                  <span>{t.status}</span>
                  <span className="sep">·</span>
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
          ))}
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
              <StatusChip t={ticket} />
              <span className="muted">
                {ticket.assignedToMe ? "Assigned to you" : ticket.assignee ? `Now with ${ticket.assignee}` : "Unassigned"}
              </span>
            </div>
            <div className="action-group">
              <button className="action primary" onClick={() => openUrl(ticket.url)}>
                <Icon name="external" size={15} />
                <span className="grow">Open in Jira</span>
              </button>
            </div>

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
