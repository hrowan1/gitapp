import { useEffect, useMemo, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  api,
  type Activity,
  type EditorSettings,
  type Item,
  type RepoInfo,
  type Session,
  type HarvestStatus,
  type JiraStatus,
  type Settings,
  type StateId,
  type TicketAttention,
  type SummaryStatus,
  type MeetingAction,
  type KnownVoice,
} from "../lib/api";
import { toast } from "../lib/toast";
import { SECTIONS, STATES, compareItems, groupItems, shortRef } from "../lib/states";
import { setTheme as applyThemeChoice, getTheme, type Theme } from "../lib/theme";
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

/** Recognising people you've named before, and the voices the app knows. */
function KnownVoices({ settings, onSave }: { settings: Settings; onSave: (s: Settings) => void }) {
  const [voices, setVoices] = useState<KnownVoice[] | null>(null);
  const refresh = () => api.knownVoices().then(setVoices).catch(() => setVoices([]));
  useEffect(() => {
    refresh();
  }, []);
  return (
    <div className="known-voices">
      <label className="check">
        <input
          type="checkbox"
          checked={settings.recogniseVoices}
          onChange={(e) => onSave({ ...settings, recogniseVoices: e.target.checked })}
        />
        Recognise people you've named before
      </label>
      {settings.recogniseVoices && (
        <p className="muted small">
          When you name a speaker, a fingerprint of their voice is kept on this Mac so later meetings can name them for you.
          {voices && voices.length === 0 && " Nobody yet — click a speaker in a transcript to name them."}
        </p>
      )}
      {voices && voices.length > 0 && (
        <div className="voice-list">
          {voices.map((v) => (
            <span key={v.id} className="voice-chip" title={`Learnt from ${v.samples} meeting${v.samples === 1 ? "" : "s"}`}>
              {v.name}
              <button
                className="ghost icon-only"
                title={`Forget ${v.name}'s voice`}
                onClick={() => api.forgetVoice(v.id).then(refresh, (e) => toast(String(e), true))}
              >
                <Icon name="x" size={11} />
              </button>
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

/** Apple Intelligence / Claude (with model) / Off, for one use of AI. */
function EngineChoice({
  label,
  provider,
  model,
  status,
  privacy,
  onChange,
}: {
  label: string;
  provider: string;
  model: string;
  status: SummaryStatus | null;
  /** What gets sent to Anthropic when it's Claude. */
  privacy: string;
  onChange: (provider: string, model: string) => void;
}) {
  return (
    <>
      <label className="field">
        <span>{label}</span>
        <select value={provider} onChange={(e) => onChange(e.target.value, model)}>
          <option value="apple">On this Mac (Apple Intelligence)</option>
          <option value="claude">With Claude (Anthropic)</option>
          <option value="off">Off</option>
        </select>
        {provider === "claude" && (
          <select value={model} onChange={(e) => onChange(provider, e.target.value)} title="Claude model">
            {(status?.claudeModels ?? [{ id: model, name: model }]).map((m) => (
              <option key={m.id} value={m.id}>
                {m.name.replace("Claude ", "")}
                {m.id.includes("haiku") ? " (recommended)" : ""}
              </option>
            ))}
          </select>
        )}
      </label>
      {provider === "apple" && status && !status.appleReady && <p className="warn small">{status.appleReason}.</p>}
      {provider === "claude" && (
        <p className="muted small">
          {privacy}
          {status && !status.claudeKey && " Until an Anthropic API key is saved (Settings → AI), Apple Intelligence is used instead."}
        </p>
      )}
    </>
  );
}

/** The Anthropic key (entered once, used by every AI feature) and the engine for the AI features. */
function AiCard({
  settings,
  status,
  onSave,
  onStatus,
}: {
  settings: Settings;
  status: SummaryStatus | null;
  onSave: (s: Settings) => void;
  onStatus: () => void;
}) {
  const [key, setKey] = useState("");
  return (
    <section className="card">
      <h2>AI</h2>
      <p className="muted">
        Ask about your history in ⌘K, day and week summaries, ticket suggestions for meetings, CI failure explanations and
        smarter reminders. Apple Intelligence runs on this Mac and is free; Claude uses your own Anthropic API key (Haiku
        costs well under a cent per question).
      </p>
      {status?.claudeKey ? (
        <p className="small">
          <Icon name="checkCircle" size={13} /> Anthropic API key saved in the keychain ·{" "}
          <button className="link" onClick={() => api.clearAnthropicKey().then(onStatus)}>
            remove
          </button>
        </p>
      ) : (
        <div className="field">
          <span>Anthropic API key</span>
          <input
            type="password"
            className="grow"
            placeholder="sk-ant-… (optional — for Claude)"
            autoComplete="off"
            value={key}
            onChange={(e) => setKey(e.target.value)}
          />
          <button
            className="primary"
            disabled={!key.trim()}
            onClick={async () => {
              try {
                await api.setAnthropicKey(key);
                setKey("");
                onStatus();
                toast("API key saved");
              } catch (e) {
                toast(String(e), true);
              }
            }}
          >
            Save
          </button>
        </div>
      )}
      <EngineChoice
        label="AI features"
        provider={settings.aiProvider}
        model={settings.aiModel}
        status={status}
        privacy="The facts each feature needs (activity, ticket moves, meeting notes and lines, CI logs) are sent to Anthropic, billed to your account."
        onChange={(aiProvider, aiModel) => onSave({ ...settings, aiProvider, aiModel })}
      />
    </section>
  );
}

export function SettingsView({ session, onSignedOut }: { session: Session; onSignedOut: () => void }) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [repos, setRepos] = useState<RepoInfo[]>([]);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [theme, setTheme] = useState<Theme>(getTheme);
  const [aiStatus, setAiStatus] = useState<SummaryStatus | null>(null);
  const refreshAi = () => void api.summaryStatus().then(setAiStatus).catch(() => {});
  useEffect(refreshAi, []);

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
    <div className="page settings">
      <div className="headline compact">
        <h1>Settings</h1>
      </div>
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
            <button className="link small" onClick={() => api.testNotification().then((m) => toast(m), (e) => toast(String(e), true))}>
              send a test
            </button>
          </label>
          <label className="check">
            <input
              type="checkbox"
              checked={settings.menuBarCount}
              onChange={(e) => save({ ...settings, menuBarCount: e.target.checked })}
            />
            Show the number in the menu bar (otherwise a dot on the icon when something needs you)
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

      {settings && (
        <section className="card">
          <h2>Time tracking</h2>
          <p className="muted">
            The Day view estimates time from your commits, pushes, reviews and comments. It's more accurate if Dev
            Inbox also notices when files change in your local clones (checked every 5 minutes). Only the repo, branch
            and time are recorded, and nothing leaves this Mac.
          </p>
          <label className="check">
            <input
              type="checkbox"
              checked={settings.trackLocal}
              onChange={(e) => save({ ...settings, trackLocal: e.target.checked })}
            />
            Notice local editing activity
          </label>
        </section>
      )}

      {settings && (
        <section className="card">
          <h2>Meetings</h2>
          <p className="muted">
            Recordings stay on this Mac, and transcription uses macOS's on-device speech recognition. Transcripts are
            always kept.
          </p>
          <label className="field">
            <span>Keep meeting audio</span>
            <select
              value={settings.audioRetentionDays}
              onChange={(e) => save({ ...settings, audioRetentionDays: Number(e.target.value) })}
            >
              <option value={-1}>Delete once transcribed</option>
              <option value={7}>7 days</option>
              <option value={30}>30 days</option>
              <option value={90}>90 days</option>
              <option value={0}>Forever</option>
            </select>
          </label>
          <p className="muted small">
            The first recording asks for Microphone and Screen &amp; System Audio Recording permission (System Settings →
            Privacy &amp; Security).
          </p>
          <label className="check">
            <input
              type="checkbox"
              checked={settings.liveCaptions}
              onChange={(e) => save({ ...settings, liveCaptions: e.target.checked })}
            />
            Live captions while recording
          </label>
          <label className="check">
            <input
              type="checkbox"
              checked={settings.separateSpeakers}
              onChange={(e) => save({ ...settings, separateSpeakers: e.target.checked })}
            />
            Tell speakers apart (Speaker 1, 2… — on this Mac; downloads a ~20 MB voice model once)
          </label>
          {settings.separateSpeakers && <KnownVoices settings={settings} onSave={save} />}
          <EngineChoice
            label="Summarise meetings"
            provider={settings.summaryProvider}
            model={settings.claudeModel}
            status={aiStatus}
            privacy="After each transcript, it's sent to Anthropic to write the notes, billed to your account (Haiku: about $0.002 for a 30-minute meeting)."
            onChange={(summaryProvider, claudeModel) => save({ ...settings, summaryProvider, claudeModel })}
          />
        </section>
      )}

      {settings && <AiCard settings={settings} status={aiStatus} onSave={save} onStatus={refreshAi} />}

      <JiraCard />

      <HarvestCard />

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

/** Connect Jira (read-only): site, Atlassian email and an API token the user creates. */
function JiraCard() {
  const [status, setStatus] = useState<JiraStatus | null>(null);
  const [site, setSite] = useState("");
  const [email, setEmail] = useState("");
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    api.jiraStatus().then((s) => {
      setStatus(s);
      setSite(s.site?.replace("https://", "") ?? "");
      setEmail(s.email ?? "");
    });
  }, []);
  if (!status) return null;

  if (status.connected) {
    return (
      <section className="card">
        <h2>Jira</h2>
        <div className="account">
          <span className="state-icon tone-blue">
            <Icon name="ticket" size={14} />
          </span>
          <div>
            <div className="strong">
              {status.name} · {status.site?.replace("https://", "")}
            </div>
            <div className="muted small">
              {status.tickets} tickets · {status.lastSync ? `synced ${friendlyDate(status.lastSync)} ${clock(status.lastSync)}` : "first sync running…"}
              {status.error && <span className="error"> · {status.error}</span>}
            </div>
          </div>
        </div>
        <div className="actions">
          <button
            disabled={busy}
            onClick={async () => {
              setBusy(true);
              try {
                await api.jiraSyncNow();
                setStatus(await api.jiraStatus());
                toast("Jira synced");
              } catch (e) {
                toast(String(e), true);
              } finally {
                setBusy(false);
              }
            }}
          >
            {busy ? "Syncing…" : "Sync now"}
          </button>
          <button
            className="danger-outline"
            onClick={async () => {
              await api.jiraDisconnect();
              setStatus(await api.jiraStatus());
              setToken("");
            }}
          >
            Disconnect and forget Jira data
          </button>
        </div>
      </section>
    );
  }

  return (
    <section className="card">
      <h2>Jira</h2>
      <p className="muted">
        Shows tickets that involve you (assigned, reported or worked on), and how they moved, in Today, Tickets and
        the Day view. From Tickets you can move a ticket's status and assign it; nothing else is written to Jira. The
        token is stored in your system keychain.
      </p>
      <form
        onSubmit={async (e) => {
          e.preventDefault();
          setBusy(true);
          try {
            setStatus(await api.jiraConnect(site, email, token));
            setToken("");
            toast("Jira connected — fetching the last 30 days");
          } catch (err) {
            toast(String(err), true);
          } finally {
            setBusy(false);
          }
        }}
      >
        <label className="field">
          <span>Jira site</span>
          <input className="grow" placeholder="yourcompany.atlassian.net" value={site} onChange={(e) => setSite(e.target.value)} />
        </label>
        <label className="field">
          <span>Atlassian email</span>
          <input className="grow" type="email" value={email} onChange={(e) => setEmail(e.target.value)} />
        </label>
        <label className="field">
          <span>API token</span>
          <input className="grow" type="password" value={token} onChange={(e) => setToken(e.target.value)} />
        </label>
        <div className="actions">
          <button className="primary" disabled={busy || !site || !email || !token}>
            {busy ? "Connecting…" : "Connect"}
          </button>
          <button
            type="button"
            className="link small"
            onClick={() => openUrl("https://id.atlassian.com/manage-profile/security/api-tokens")}
          >
            Create an API token
          </button>
        </div>
      </form>
    </section>
  );
}

function HarvestCard() {
  const [status, setStatus] = useState<HarvestStatus | null>(null);
  const [account, setAccount] = useState("");
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [nudge, setNudge] = useState("17:30");
  useEffect(() => {
    api.harvestStatus().then((s) => {
      setStatus(s);
      setAccount(s.accountId ?? "");
      if (s.nudge) setNudge(s.nudge);
    });
  }, []);
  if (!status) return null;

  const saveNudge = async (at: string | null) => {
    try {
      await api.setTimesheetNudge(at);
      setStatus(await api.harvestStatus());
    } catch (e) {
      toast(String(e), true);
    }
  };

  if (status.connected) {
    return (
      <section className="card">
        <h2>Harvest</h2>
        <div className="account">
          <span className="state-icon tone-orange">
            <Icon name="clock" size={14} />
          </span>
          <div>
            <div className="strong">
              {status.name} · account {status.accountId}
            </div>
            <div className="muted small">
              {status.linkLearnt
                ? "Entries are linked to their Jira ticket, the same way Harvest's Jira button does."
                : "Entries include their Jira link. Log one entry from Jira's Harvest button, then reconnect, to link them exactly as it does."}
            </div>
          </div>
        </div>
        <label className="check">
          <input
            type="checkbox"
            checked={status.nudge !== null}
            onChange={(e) => void saveNudge(e.target.checked ? nudge : null)}
          />
          Remind me on weekdays if today's timesheet hasn't been sent, at
          <input
            type="time"
            value={nudge}
            disabled={status.nudge === null}
            onChange={(e) => setNudge(e.target.value)}
            onBlur={() => status.nudge !== null && nudge && void saveNudge(nudge)}
          />
        </label>
        <div className="actions">
          <button
            className="danger-outline"
            onClick={async () => {
              await api.harvestDisconnect();
              setStatus(await api.harvestStatus());
              setToken("");
            }}
          >
            Disconnect Harvest
          </button>
        </div>
      </section>
    );
  }

  return (
    <section className="card">
      <h2>Harvest</h2>
      <p className="muted">
        Turns the Day view into a weekly timesheet and sends it to Harvest: one entry per ticket per day, linked to the
        Jira ticket, in the project and task you pick (remembered per Jira project). Only entries this app made are
        ever changed or removed. The token is stored in your system keychain.
      </p>
      <form
        onSubmit={async (e) => {
          e.preventDefault();
          setBusy(true);
          try {
            setStatus(await api.harvestConnect(account, token));
            setToken("");
            toast("Harvest connected");
          } catch (err) {
            toast(String(err), true);
          } finally {
            setBusy(false);
          }
        }}
      >
        <label className="field">
          <span>Account ID</span>
          <input className="grow" inputMode="numeric" value={account} onChange={(e) => setAccount(e.target.value)} />
        </label>
        <label className="field">
          <span>Personal access token</span>
          <input className="grow" type="password" value={token} onChange={(e) => setToken(e.target.value)} />
        </label>
        <div className="actions">
          <button className="primary" disabled={busy || !account || !token}>
            {busy ? "Connecting…" : "Connect"}
          </button>
          <button type="button" className="link small" onClick={() => openUrl("https://id.getharvest.com/developers")}>
            Create a token (shows your account ID)
          </button>
        </div>
      </form>
    </section>
  );
}
