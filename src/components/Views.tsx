import { useEffect, useMemo, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  api,
  type Activity,
  type Item,
  type Session,
  type StateId,
  type TicketAttention,
  type MeetingAction,
} from "../lib/api";
import { SECTIONS, STATES, compareItems, groupItems, shortRef } from "../lib/states";
import { clock, friendlyDate } from "../lib/time";
import { Icon, type IconName } from "./Icon";
import { PrList } from "./PrList";
import { TicketsNeedingYou } from "./TicketsNeedingYou";
import { MeetingActions } from "./MeetingActions";
import { useSelection, Workspace } from "./Workspace";

export function Today({
  items,
  tickets,
  actions,
  onOpenMeeting,
  focus,
  now,
  active,
  onOpenTicket,
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
  /** Open this item (from a notification or the menu bar panel). */
  focus?: { id: string; n: number } | null;
}) {
  const groups = useMemo(() => groupItems(items), [items]);
  const sections = useMemo(
    () =>
      SECTIONS.map((s) => ({
        ...s,
        items: groups[s.id],
        collapsedByDefault: s.id === "done" || s.id === "stale",
      })),
    [groups],
  );
  const { selectedId, selection, onSelect, clear } = useSelection(items, focus);
  const needs = groups.needs.length + tickets.length + actions.length;
  const fresh = items.filter((i) => i.isNew);
  const header = (
    <div className="headline">
      <h1>{needs === 0 ? "You're all caught up" : `${needs} ${needs === 1 ? "thing needs" : "things need"} you`}</h1>
      <p className="muted">
        {fresh.length > 0 && (
          <>
            <span className="new-text">{fresh.length} new</span>{" "}
            <button className="link" onClick={() => api.markSeen(fresh.map((i) => i.id))}>
              mark seen
            </button>
            {" · "}
          </>
        )}
        {groups.later.length > 0 && `${groups.later.length} parked · `}
        {groups.waiting.length + groups.mine.length} in progress or waiting
      </p>
    </div>
  );
  return (
    <Workspace header={header} selection={selection} now={now} onClose={clear}>
      <TicketsNeedingYou tickets={tickets} now={now} onOpen={onOpenTicket} />
      <MeetingActions actions={actions} onOpenMeeting={onOpenMeeting} />
      <PrList sections={sections} now={now} active={active} selectedId={selectedId} onSelect={onSelect} />
    </Workspace>
  );
}

export function AllPrs({ items, now, active }: { items: Item[]; now: number; active: boolean }) {
  const [repo, setRepo] = useState("");
  const [author, setAuthor] = useState("");
  const [state, setStateFilter] = useState("");
  const [owner, setOwner] = useState<"" | "mine" | "others">("");
  const [ci, setCi] = useState("");
  const [kind, setKind] = useState("");
  const [text, setText] = useState("");
  const { selectedId, selection, onSelect, clear } = useSelection(items);

  const repos = useMemo(() => [...new Set(items.map((i) => i.repo))].sort(), [items]);
  const authors = useMemo(() => [...new Set(items.map((i) => i.author))].sort((a, b) => a.localeCompare(b)), [items]);
  const filtered = useMemo(
    () =>
      items
        .filter(
          (i) =>
            (!repo || i.repo === repo || i.repo.startsWith(repo + "/")) &&
            (!author || i.author === author) &&
            (!state || i.state === state) &&
            (!ci || i.ci === ci) &&
            (!kind || i.kind === kind) &&
            (!owner || (owner === "mine") === i.isMine) &&
            (!text || `${i.title} ${i.repo} #${i.number}`.toLowerCase().includes(text.toLowerCase())),
        )
        .sort(compareItems),
    [items, repo, author, state, ci, kind, owner, text],
  );
  const orgs = [...new Set(repos.map((r) => r.split("/")[0]))];
  const sections = useMemo(
    () => [{ id: "all", title: `${filtered.length} items`, items: filtered, empty: "No matches." }],
    [filtered],
  );

  const header = (
    <>
      <div className="headline compact">
        <h1>Inbox</h1>
        <p className="muted">Everything you're involved in, across every repository.</p>
      </div>
      <div className="filters">
        <div className="search-box">
          <Icon name="search" size={14} />
          <input placeholder="Filter by title, repo or number" value={text} onChange={(e) => setText(e.target.value)} />
        </div>
        <select value={kind} onChange={(e) => setKind(e.target.value)}>
          <option value="">All types</option>
          <option value="pr">Pull requests</option>
          <option value="issue">Issues</option>
          <option value="run">Branches</option>
        </select>
        <select value={repo} onChange={(e) => setRepo(e.target.value)}>
          <option value="">All repos</option>
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
        <select value={state} onChange={(e) => setStateFilter(e.target.value)}>
          <option value="">Any state</option>
          {(Object.keys(STATES) as StateId[]).map((s) => (
            <option key={s} value={s}>
              {STATES[s].label}
            </option>
          ))}
        </select>
        <select value={author} onChange={(e) => setAuthor(e.target.value)}>
          <option value="">Anyone</option>
          {authors.map((a) => (
            <option key={a}>{a}</option>
          ))}
        </select>
        <select value={ci} onChange={(e) => setCi(e.target.value)}>
          <option value="">Any CI</option>
          <option value="failure">Failing</option>
          <option value="pending">Running</option>
          <option value="success">Passing</option>
        </select>
        <select value={owner} onChange={(e) => setOwner(e.target.value as typeof owner)}>
          <option value="">Mine & others'</option>
          <option value="mine">Mine</option>
          <option value="others">Others'</option>
        </select>
      </div>
    </>
  );

  return (
    <Workspace header={header} selection={selection} now={now} onClose={clear}>
      <PrList sections={sections} now={now} active={active} selectedId={selectedId} onSelect={onSelect} />
    </Workspace>
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

