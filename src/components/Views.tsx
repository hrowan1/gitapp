import { useEffect, useMemo, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  api,
  type Activity,
  type Item,
  type Session,
  type TicketAttention,
  type MeetingAction,
} from "../lib/api";
import { TODO_SECTIONS, groupTodo, shortRef } from "../lib/states";
import { clock, friendlyDate } from "../lib/time";
import { Icon, type IconName } from "./Icon";
import { PrList } from "./PrList";
import { TicketsNeedingYou } from "./TicketsNeedingYou";
import { MeetingActions } from "./MeetingActions";
import { useSelection } from "./Workspace";

/** One column: a header, then the list. Details open inside the list, under the row. */
function ListPage({ header, children }: { header: React.ReactNode; children: React.ReactNode }) {
  return (
    <div className="page list-page">
      <div className="list-page-head">{header}</div>
      {children}
    </div>
  );
}

const ACTIONABLE_TODO = TODO_SECTIONS.filter((s) => s.actionable);

export function Today({
  items,
  tickets,
  actions,
  onOpenMeeting,
  focus,
  now,
  active,
  onOpenTicket,
  onOpenInbox,
}: {
  items: Item[];
  /** Jira tickets that need you. */
  tickets: TicketAttention[];
  /** Action items of yours from meeting notes. */
  actions: MeetingAction[];
  now: number;
  active: boolean;
  onOpenTicket: (key: string) => void;
  onOpenMeeting: (id: string) => void;
  onOpenInbox: () => void;
  /** Open this item (from a notification or the menu bar panel). */
  focus?: { id: string; n: number } | null;
}) {
  const groups = useMemo(() => groupTodo(items), [items]);
  // Today is what needs you; what you're waiting on and what you've parked are tucked away below.
  const sections = useMemo(
    () => [
      ...ACTIONABLE_TODO.map((s) => ({ id: s.id, title: s.title, items: groups[s.id] })),
      { id: "waiting", title: "Waiting on others", items: groups.waiting, collapsedByDefault: true },
      { id: "parked", title: "Parked", items: groups.parked, collapsedByDefault: true },
    ],
    [groups],
  );
  const { selectedId, onSelect } = useSelection(items, focus);
  const prs = ACTIONABLE_TODO.reduce((n, s) => n + groups[s.id].length, 0);
  const needs = prs + tickets.length + actions.length;
  const fresh = items.filter((i) => i.isNew);
  const counts = [
    groups.replies.length && `${groups.replies.length} ${groups.replies.length === 1 ? "reply" : "replies"}`,
    groups.review.length && `${groups.review.length} to review`,
    groups.fix.length && `${groups.fix.length} to fix`,
    groups.merge.length && `${groups.merge.length} to merge`,
    tickets.length && `${tickets.length} Jira`,
    actions.length && `${actions.length} from meetings`,
  ].filter(Boolean);

  const header = (
    <div className="headline">
      <h1>{needs === 0 ? "You're all caught up" : `${needs} ${needs === 1 ? "thing needs" : "things need"} you`}</h1>
      <p className="muted">
        {counts.join(" · ")}
        {fresh.length > 0 && (
          <>
            {counts.length > 0 && " · "}
            <span className="new-text">{fresh.length} new</span>{" "}
            <button className="link" onClick={() => api.markSeen(fresh.map((i) => i.id))}>
              mark seen
            </button>
          </>
        )}
      </p>
    </div>
  );
  return (
    <ListPage header={header}>
      {needs === 0 && (
        <div className="empty big">
          <Icon name="checkCircle" size={22} />
          <p>Nothing needs you right now.</p>
          <button className="link" onClick={onOpenInbox}>
            See everything in the Inbox
          </button>
        </div>
      )}
      <TicketsNeedingYou tickets={tickets} now={now} onOpen={onOpenTicket} />
      <MeetingActions actions={actions} onOpenMeeting={onOpenMeeting} />
      <PrList sections={sections} now={now} active={active} selectedId={selectedId} onSelect={onSelect} />
    </ListPage>
  );
}

/** Everything you're involved in, in the same what-to-do sections, with search and a repo filter. */
export function AllPrs({ items, now, active }: { items: Item[]; now: number; active: boolean }) {
  const [repo, setRepo] = useState("");
  const [text, setText] = useState("");
  const { selectedId, onSelect } = useSelection(items);

  const repos = useMemo(() => [...new Set(items.map((i) => i.repo))].sort(), [items]);
  const orgs = [...new Set(repos.map((r) => r.split("/")[0]))];
  const filtered = useMemo(() => {
    const q = text.toLowerCase().trim();
    return items.filter(
      (i) =>
        (!repo || i.repo === repo || i.repo.startsWith(repo + "/")) &&
        (!q || `${i.title} ${i.repo} #${i.number} ${i.author}`.toLowerCase().includes(q)),
    );
  }, [items, repo, text]);
  const groups = useMemo(() => groupTodo(filtered), [filtered]);
  const searching = !!text.trim() || !!repo;
  const sections = useMemo(
    () =>
      TODO_SECTIONS.map((s) => ({
        id: s.id,
        title: s.title,
        items: groups[s.id],
        // Old and finished things stay folded unless you're looking for something.
        collapsedByDefault: !searching && (s.id === "quiet" || s.id === "done"),
      })),
    [groups, searching],
  );

  const header = (
    <>
      <div className="headline compact">
        <h1>Inbox</h1>
        <p className="muted">
          {searching ? `${filtered.length} of ${items.length}` : items.length} things you're involved in, across every
          repository.
        </p>
      </div>
      <div className="filters">
        <div className="search-box grow">
          <Icon name="search" size={14} />
          <input
            placeholder="Search titles, repos, numbers or people"
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => e.key === "Escape" && setText("")}
          />
        </div>
        <select value={repo} onChange={(e) => setRepo(e.target.value)}>
          <option value="">All repositories</option>
          {orgs.length > 1 && (
            <optgroup label="Organisations">
              {orgs.map((o) => (
                <option key={o} value={o}>
                  {o}/*
                </option>
              ))}
            </optgroup>
          )}
          <optgroup label="Repositories">
            {repos.map((r) => (
              <option key={r} value={r}>
                {r}
              </option>
            ))}
          </optgroup>
        </select>
      </div>
    </>
  );

  return (
    <ListPage header={header}>
      {searching && filtered.length === 0 && <div className="empty">Nothing matches.</div>}
      <PrList sections={sections} now={now} active={active} selectedId={selectedId} onSelect={onSelect} />
    </ListPage>
  );
}

const KIND_ICON: Record<Activity["kind"], IconName> = {
  committed: "code",
  pushed: "branch",
  reviewed: "eye",
  commented: "message",
  opened: "pr",
  merged: "merge",
  closed: "xCircle",
  marked: "pin",
  update: "refresh",
};

export function History({ version, onOpenDay }: { version: number; onOpenDay: () => void }) {
  const [days, setDays] = useState(7);
  const [mineOnly, setMineOnly] = useState(false);
  const [rows, setRows] = useState<Activity[]>([]);
  useEffect(() => {
    api.getActivity(days).then(setRows).catch(console.error);
  }, [days, version]);

  const shown = mineOnly ? rows.filter((r) => r.kind !== "update") : rows;
  const byDay = new Map<string, Activity[]>();
  for (const r of shown) {
    const k = friendlyDate(r.at);
    byDay.set(k, [...(byDay.get(k) ?? []), r]);
  }

  return (
    <div className="page">
      <div className="headline compact">
        <h1>History</h1>
        <p className="muted">What you did, and what changed on things you follow.</p>
      </div>
      <div className="filters">
        <select value={days} onChange={(e) => setDays(Number(e.target.value))}>
          <option value={1}>Last 24 hours</option>
          <option value={7}>Last 7 days</option>
          <option value={30}>Last 30 days</option>
          <option value={365}>Last year</option>
        </select>
        <label className="check">
          <input type="checkbox" checked={mineOnly} onChange={(e) => setMineOnly(e.target.checked)} /> Only things I
          did
        </label>
        <button className="standup-btn" onClick={onOpenDay} title="Day view (s)">
          <Icon name="calendar" size={14} /> Time spent per day
        </button>
      </div>
      {shown.length === 0 && (
        <div className="empty big">
          Nothing yet. Your reviews, comments, PRs and the states you set will build up here.
        </div>
      )}
      {[...byDay.entries()].map(([day, list]) => (
        <section key={day} className="section">
          <div className="section-head static">
            <span className="section-title">{day}</span>
            <span className="count">{list.length}</span>
          </div>
          {list.map((a, idx) => (
            <div
              key={idx}
              className="row history"
              role="button"
              onClick={() => a.url && openUrl(a.url)}
            >
              <span className="time">{clock(a.at)}</span>
              <span className={`state-icon act-${a.kind}`}>
                <Icon name={KIND_ICON[a.kind]} size={15} />
              </span>
              <div className="row-main">
                <div className="row-title">
                  <span className="who">{a.detail}</span>
                </div>
                {a.title && (
                  <div className="row-sub">
                    <span className="repo">{a.repo ? shortRef({ kind: a.number ? "pr" : "run", repo: a.repo, number: a.number ?? 0 }) : ""}</span>
                    <span className="sep">·</span>
                    <span className="reason">{a.title}</span>
                  </div>
                )}
              </div>
            </div>
          ))}
        </section>
      ))}
    </div>
  );
}

export function SignIn({ onSignedIn }: { onSignedIn: (s: Session) => void }) {
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = async (fn: () => Promise<Session>) => {
    setBusy(true);
    setError(null);
    try {
      onSignedIn(await fn());
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="signin">
      <div className="signin-card">
        <div className="logo">📥</div>
        <h1>Dev Inbox</h1>
        <p className="muted">Everything that needs your attention, across every repository.</p>
        <button className="primary wide" disabled={busy} onClick={() => run(api.importGhToken)}>
          Use my GitHub CLI login
        </button>
        <p className="muted small">
          Reuses <code>gh auth login</code> if you have the GitHub CLI installed.
        </p>
        <div className="divider">
          <span>or paste a token</span>
        </div>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            run(() => api.signIn(token));
          }}
        >
          <input
            className="wide"
            type="password"
            placeholder="ghp_…"
            value={token}
            onChange={(e) => setToken(e.target.value)}
          />
          <button className="wide" disabled={busy || !token.trim()}>
            Sign in with token
          </button>
        </form>
        <p className="muted small">
          Needs a classic token with <code>repo</code> and <code>read:org</code> scopes.{" "}
          <a
            href="#"
            onClick={(e) => {
              e.preventDefault();
              openUrl("https://github.com/settings/tokens/new?scopes=repo,read:org&description=Dev%20Inbox");
            }}
          >
            Create one
          </a>
          .
        </p>
        {busy && <p className="muted">Connecting to GitHub…</p>}
        {error && <p className="error">{error}</p>}
      </div>
    </div>
  );
}

