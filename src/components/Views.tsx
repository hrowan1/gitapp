import { useEffect, useMemo, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  api,
  type Activity,
  type EditorSettings,
  type Item,
  type RepoInfo,
  type Session,
  type Settings,
  type StateId,
} from "../lib/api";
import { toast } from "../lib/toast";
import { SECTIONS, STATES, compareItems, groupItems, shortRef } from "../lib/states";
import { setTheme as applyThemeChoice, getTheme, type Theme } from "../lib/theme";
import { clock, friendlyDate } from "../lib/time";
import { PrList } from "./PrList";

export function Today({ items, now, active }: { items: Item[]; now: number; active: boolean }) {
  const groups = useMemo(() => groupItems(items), [items]);
  const sections = SECTIONS.map((s) => ({
    ...s,
    items: groups[s.id],
    collapsedByDefault: s.id === "done" || s.id === "stale",
  }));
  const needs = groups.needs.length;
  const fresh = items.filter((i) => i.isNew);
  return (
    <>
      <div className="headline">
        {needs === 0 ? (
          <h1>You're all caught up.</h1>
        ) : (
          <h1>
            {needs} {needs === 1 ? "thing needs" : "things need"} you.
          </h1>
        )}
        <p className="muted">
          {fresh.length > 0 && (
            <>
              <span className="new-text">{fresh.length} new since you last looked</span>{" "}
              <button className="link small" onClick={() => api.markSeen(fresh.map((i) => i.id))}>
                mark seen
              </button>
              {" · "}
            </>
          )}
          {groups.later.length > 0 && `${groups.later.length} parked · `}
          {groups.waiting.length + groups.mine.length} waiting on others or in progress
        </p>
      </div>
      <PrList sections={sections} now={now} active={active} />
    </>
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

  const repos = useMemo(() => [...new Set(items.map((i) => i.repo))].sort(), [items]);
  const authors = useMemo(() => [...new Set(items.map((i) => i.author))].sort((a, b) => a.localeCompare(b)), [items]);
  const filtered = items
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
    .sort(compareItems);
  const orgs = [...new Set(repos.map((r) => r.split("/")[0]))];

  return (
    <>
      <div className="filters">
        <input className="search" placeholder="Filter…" value={text} onChange={(e) => setText(e.target.value)} />
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
        <select value={author} onChange={(e) => setAuthor(e.target.value)}>
          <option value="">Any author</option>
          {authors.map((a) => (
            <option key={a}>{a}</option>
          ))}
        </select>
        <select value={state} onChange={(e) => setStateFilter(e.target.value)}>
          <option value="">Any state</option>
          {(Object.keys(STATES) as StateId[]).map((s) => (
            <option key={s} value={s}>
              {STATES[s].emoji} {STATES[s].label}
            </option>
          ))}
        </select>
        <select value={kind} onChange={(e) => setKind(e.target.value)}>
          <option value="">PRs, issues & branches</option>
          <option value="pr">Pull requests</option>
          <option value="issue">Issues</option>
          <option value="run">Branches</option>
        </select>
        <select value={ci} onChange={(e) => setCi(e.target.value)}>
          <option value="">Any CI</option>
          <option value="failure">Failing</option>
          <option value="pending">Running</option>
          <option value="success">Passing</option>
        </select>
        <select value={owner} onChange={(e) => setOwner(e.target.value as typeof owner)}>
          <option value="">Everyone's</option>
          <option value="mine">Mine</option>
          <option value="others">Others'</option>
        </select>
      </div>
      <PrList
        sections={[{ id: "all", title: `${filtered.length} items`, items: filtered, empty: "No matches." }]}
        now={now}
        active={active}
      />
    </>
  );
}

const KIND_ICON: Record<Activity["kind"], string> = {
  reviewed: "🔍",
  commented: "💬",
  opened: "🆕",
  merged: "🟣",
  closed: "⚪️",
  marked: "📝",
  update: "🔔",
};

export function History({ version, onStandup }: { version: number; onStandup: () => void }) {
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
    <>
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
        <button className="primary standup-btn" onClick={onStandup} title="Write my standup (s)">
          📝 Write my standup
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
              <span className="row-state">{KIND_ICON[a.kind]}</span>
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
    </>
  );
}

export function SettingsView({ session, onSignedOut }: { session: Session; onSignedOut: () => void }) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [repos, setRepos] = useState<RepoInfo[]>([]);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [theme, setTheme] = useState<Theme>(getTheme);

  useEffect(() => {
    api.getSettings().then(setSettings);
    api.getRepos().then(setRepos);
  }, []);

  const save = (s: Settings) => {
    setSettings(s);
    api.updateSettings(s);
  };
  const toggleRepo = async (r: RepoInfo) => {
    await api.setRepoHidden(r.repo, !r.hidden);
    setRepos(await api.getRepos());
  };
  const shortcut = navigator.platform.toLowerCase().includes("mac") ? "⌥⌘K" : "Ctrl+Alt+K";

  return (
    <div className="settings">
      <section className="card">
        <h2>Account</h2>
        <div className="account">
          {session.avatarUrl && <img className="avatar lg" src={session.avatarUrl} alt="" />}
          <div>
            <div className="strong">{session.login}</div>
            <div className="muted">Token stored in your system keychain. PR metadata stays on this machine.</div>
          </div>
        </div>
        <div className="actions">
          <button
            onClick={async () => {
              await api.signOut(false);
              onSignedOut();
            }}
          >
            Sign out
          </button>
          {confirmDelete ? (
            <button
              className="danger"
              onClick={async () => {
                await api.signOut(true);
                onSignedOut();
              }}
            >
              Really delete all local data?
            </button>
          ) : (
            <button className="danger-outline" onClick={() => setConfirmDelete(true)}>
              Sign out and delete local data…
            </button>
          )}
        </div>
      </section>

      <section className="card">
        <h2>Appearance</h2>
        <label className="field">
          <span>Theme</span>
          <select
            value={theme}
            onChange={(e) => {
              const t = e.target.value as Theme;
              setTheme(t);
              applyThemeChoice(t);
            }}
          >
            <option value="system">Match system</option>
            <option value="light">Light</option>
            <option value="dark">Dark</option>
          </select>
        </label>
      </section>

      {settings && (
        <section className="card">
          <h2>Sync & notifications</h2>
          <label className="field">
            <span>Check GitHub every</span>
            <select
              value={settings.pollSeconds}
              onChange={(e) => save({ ...settings, pollSeconds: Number(e.target.value) })}
            >
              <option value={60}>minute</option>
              <option value={120}>2 minutes</option>
              <option value={300}>5 minutes</option>
              <option value={900}>15 minutes</option>
            </select>
          </label>
          <label className="check">
            <input
              type="checkbox"
              checked={settings.notifications}
              onChange={(e) => save({ ...settings, notifications: e.target.checked })}
            />
            Desktop notifications when something new needs me
            <button className="link small" onClick={() => api.testNotification()}>
              send a test
            </button>
          </label>
          <label className="check">
            <input
              type="checkbox"
              checked={settings.digest}
              disabled={!settings.notifications}
              onChange={(e) => save({ ...settings, digest: e.target.checked })}
            />
            Morning summary on weekdays at
            <select
              value={settings.digestHour}
              disabled={!settings.notifications || !settings.digest}
              onChange={(e) => save({ ...settings, digestHour: Number(e.target.value) })}
            >
              {[6, 7, 8, 9, 10, 11, 12].map((h) => (
                <option key={h} value={h}>
                  {String(h).padStart(2, "0")}:00
                </option>
              ))}
            </select>
          </label>
          <p className="muted">
            Global shortcut: <kbd>{shortcut}</kbd> opens the command bar from anywhere. Closing the window keeps Dev
            Inbox running in the menu bar.
          </p>
        </section>
      )}

      {settings && <IgnoredUsers settings={settings} onSave={save} />}

      <CodeAndEditor repos={repos.filter((r) => !r.hidden && r.openPrs > 0).map((r) => r.repo)} />

      <section className="card">
        <h2>Repositories</h2>
        <p className="muted">Hidden repositories are still synced but never shown or notified.</p>
        {repos.length === 0 && <div className="empty">No repositories yet — they appear after the first sync.</div>}
        <div className="repo-list">
          {repos.map((r) => (
            <label key={r.repo} className="check repo-row">
              <input type="checkbox" checked={!r.hidden} onChange={() => toggleRepo(r)} />
              <span className="grow">{r.repo}</span>
              <span className="muted">{r.openPrs} open</span>
            </label>
          ))}
        </div>
      </section>
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

function IgnoredUsers({ settings, onSave }: { settings: Settings; onSave: (s: Settings) => void }) {
  const [text, setText] = useState(settings.ignoredUsers.join(", "));
  const parse = (t: string) =>
    t
      .split(/[\s,]+/)
      .map((s) => s.replace(/^@/, "").trim())
      .filter(Boolean);
  const dirty = parse(text).join(",") !== settings.ignoredUsers.join(",");
  return (
    <section className="card">
      <h2>Ignore people & bots</h2>
      <p className="muted">
        Their comments never count as replies, and PRs they open go to FYI unless they ask you for a review. Accounts
        ending in “bot” are ignored automatically.
      </p>
      <div className="row-inline">
        <input
          className="grow"
          placeholder="e.g. renovate, sonarcloud, deploy-user"
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") onSave({ ...settings, ignoredUsers: parse(text) });
          }}
        />
        <button disabled={!dirty} onClick={() => onSave({ ...settings, ignoredUsers: parse(text) })}>
          Save
        </button>
      </div>
    </section>
  );
}

/** Which editor to use and where the user's clones live. */
function CodeAndEditor({ repos }: { repos: string[] }) {
  const [es, setEs] = useState<EditorSettings | null>(null);
  const [roots, setRoots] = useState("");
  const [cloneDir, setCloneDir] = useState("");
  const [busy, setBusy] = useState(false);
  const [editing, setEditing] = useState<Record<string, string>>({});

  const load = () =>
    api
      .editorSettings()
      .then((e) => {
        setEs(e);
        setRoots(e.roots.join("\n"));
        setCloneDir(e.cloneDir);
      })
      .catch((e) => toast(String(e), true));
  useEffect(() => {
    load();
  }, []);

  if (!es) return null;
  const save = async (editor: string | null) => {
    setBusy(true);
    try {
      const n = await api.updateEditorSettings(
        editor,
        roots.split("\n").map((r) => r.trim()).filter(Boolean),
        cloneDir,
      );
      toast(`Found ${n} local clone${n === 1 ? "" : "s"}`);
      await load();
    } catch (e) {
      toast(String(e), true);
    } finally {
      setBusy(false);
    }
  };
  const setPath = async (repo: string, path: string | null) => {
    try {
      await api.setRepoPath(repo, path);
      setEditing(({ [repo]: _, ...rest }) => rest);
      await load();
    } catch (e) {
      toast(String(e), true);
    }
  };

  return (
    <section className="card">
      <h2>Code & editor</h2>
      <label className="field">
        <span>Open repos in</span>
        <select value={es.editor ?? ""} onChange={(e) => save(e.target.value || null)}>
          {es.editors.length === 0 && <option value="">No supported editor found</option>}
          {es.editors.map((e) => (
            <option key={e}>{e}</option>
          ))}
        </select>
      </label>
      <label className="field top">
        <span>Look for clones in</span>
        <textarea className="grow" rows={2} value={roots} onChange={(e) => setRoots(e.target.value)} />
      </label>
      <label className="field">
        <span>Clone new repos into</span>
        <input className="grow" value={cloneDir} onChange={(e) => setCloneDir(e.target.value)} />
      </label>
      <div className="actions">
        <button disabled={busy} onClick={() => save(es.editor ?? null)}>
          {busy ? "Scanning…" : "Save & rescan"}
        </button>
        <span className="muted small">
          {Object.keys(es.clones).length} clones found. “Check out” only switches branch when you have no
          uncommitted changes.
        </span>
      </div>
      {repos.length > 0 && (
        <div className="repo-list clones">
          {repos.map((repo) => {
            const path = es.clones[repo.toLowerCase()];
            const draft = editing[repo];
            return (
              <div key={repo} className="clone-row">
                <span className="grow ellipsis">{repo}</span>
                {draft !== undefined ? (
                  <>
                    <input
                      autoFocus
                      className="grow"
                      placeholder="/path/to/clone"
                      value={draft}
                      onChange={(e) => setEditing({ ...editing, [repo]: e.target.value })}
                      onKeyDown={(e) => e.key === "Enter" && setPath(repo, draft || null)}
                    />
                    <button className="link small" onClick={() => setPath(repo, draft || null)}>
                      save
                    </button>
                  </>
                ) : (
                  <>
                    <span className={`grow ellipsis small ${path ? "muted" : "faint"}`}>{path ?? "not cloned"}</span>
                    <button className="link small" onClick={() => setEditing({ ...editing, [repo]: path ?? "" })}>
                      set folder
                    </button>
                  </>
                )}
              </div>
            );
          })}
        </div>
      )}
    </section>
  );
}
