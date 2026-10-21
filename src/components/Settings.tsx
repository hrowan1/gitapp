import { useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  api,
  type CalendarStatus,
  type EditorSettings,
  type HarvestStatus,
  type JiraStatus,
  type KnownVoice,
  type RepoInfo,
  type Session,
  type Settings,
  type SummaryStatus,
} from "../lib/api";
import { toast } from "../lib/toast";
import { setTheme as applyThemeChoice, getTheme, type Theme } from "../lib/theme";
import { clock, friendlyDate } from "../lib/time";
import { useUpdates } from "../lib/updates";
import { Following } from "./Following";
import { Icon, type IconName } from "./Icon";

/** Recognising people you've named before, and the voices the app knows. */
function KnownVoices({ settings, onSave }: { settings: Settings; onSave: (s: Settings) => void }) {
  const [voices, setVoices] = useState<KnownVoice[] | null>(null);
  const refresh = () =>
    api
      .knownVoices()
      .then(setVoices)
      .catch(() => setVoices([]));
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
          When you name a speaker, a fingerprint of their voice is kept on this Mac so later meetings can name them for
          you.
          {voices && voices.length === 0 && " Nobody yet — click a speaker in a transcript to name them."}
        </p>
      )}
      {voices && voices.length > 0 && (
        <div className="voice-list">
          {voices.map((v) => (
            <span
              key={v.id}
              className="voice-chip"
              title={`Learnt from ${v.samples} meeting${v.samples === 1 ? "" : "s"}`}
            >
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
export function EngineChoice({
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
          {status &&
            !status.claudeKey &&
            " Until an Anthropic API key is saved (Settings → AI), Apple Intelligence is used instead."}
        </p>
      )}
    </>
  );
}

/** The Anthropic key (entered once, used by every AI feature) and the engine for the AI features. */
export function AiCard({
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
        Ask about your history in ⌘K, day and week summaries, ticket suggestions for meetings, CI failure explanations
        and smarter reminders. Apple Intelligence runs on this Mac and is free; Claude uses your own Anthropic API key
        (Haiku costs well under a cent per question).
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

export type SettingsSection = "general" | "connections" | "time" | "meetings" | "ai" | "repos" | "following";

const SETTINGS_SECTIONS: { id: SettingsSection; label: string; icon: IconName }[] = [
  { id: "general", label: "General", icon: "settings" },
  { id: "connections", label: "Connections", icon: "layers" },
  { id: "time", label: "Time & timesheet", icon: "clock" },
  { id: "meetings", label: "Meetings", icon: "mic" },
  { id: "ai", label: "AI", icon: "sparkle" },
  { id: "repos", label: "Repositories", icon: "code" },
  { id: "following", label: "Following", icon: "eye" },
];

export function SettingsView({
  session,
  onSignedOut,
  section,
  onSection,
  onSetup,
  version,
}: {
  session: Session;
  onSignedOut: () => void;
  section: SettingsSection;
  onSection: (s: SettingsSection) => void;
  /** Open the first-run setup again. */
  onSetup: () => void;
  version: number;
}) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [repos, setRepos] = useState<RepoInfo[]>([]);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [theme, setTheme] = useState<Theme>(getTheme);
  const [aiStatus, setAiStatus] = useState<SummaryStatus | null>(null);
  const refreshAi = () =>
    void api
      .summaryStatus()
      .then(setAiStatus)
      .catch(() => {});
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
    <div className="page settings-page">
      <div className="settings-layout">
        <nav className="settings-nav">
          <h1>Settings</h1>
          {SETTINGS_SECTIONS.map((s) => (
            <button
              key={s.id}
              className={`settings-nav-item${section === s.id ? " active" : ""}`}
              onClick={() => onSection(s.id)}
            >
              <Icon name={s.icon} size={15} />
              {s.label}
            </button>
          ))}
        </nav>

        <div className="settings settings-content">
          {section === "general" && (
            <>
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
                    <button
                      className="link small"
                      onClick={() =>
                        api.testNotification().then(
                          (m) => toast(m),
                          (e) => toast(String(e), true),
                        )
                      }
                    >
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
                    Global shortcut: <kbd>{shortcut}</kbd> opens the command bar from anywhere. Closing the window keeps
                    Dev Inbox running in the menu bar.
                  </p>
                </section>
              )}

              <UpdatesCard />

              <section className="card">
                <h2>Setup</h2>
                <p className="muted">
                  Go through the first-run setup again: connections, permissions and a test notification.
                </p>
                <div className="actions">
                  <button onClick={onSetup}>Run setup again</button>
                </div>
              </section>
            </>
          )}

          {section === "connections" && (
            <>
              <section className="card">
                <h2>GitHub</h2>
                <div className="account">
                  {session.avatarUrl && <img className="avatar lg" src={session.avatarUrl} alt="" />}
                  <div>
                    <div className="strong">{session.login}</div>
                    <div className="muted">
                      Token stored in your system keychain. PR metadata stays on this machine.
                    </div>
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

              <JiraCard />

              <HarvestCard />

              <CalendarCard />
            </>
          )}

          {section === "time" && (
            <>
              {settings && (
                <section className="card">
                  <h2>Time tracking</h2>
                  <p className="muted">
                    The Day view estimates time from your commits, pushes, reviews and comments. It's more accurate if
                    Dev Inbox also notices when files change in your local clones (checked every 5 minutes). Only the
                    repo, branch and time are recorded, and nothing leaves this Mac.
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
            </>
          )}

          {section === "meetings" && (
            <>
              {settings && (
                <section className="card">
                  <h2>Meetings</h2>
                  <p className="muted">
                    Recordings stay on this Mac, and transcription uses macOS's on-device speech recognition.
                    Transcripts are always kept.
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
                    The first recording asks for Microphone and Screen &amp; System Audio Recording permission (System
                    Settings → Privacy &amp; Security).
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
            </>
          )}

          {section === "ai" && settings && (
            <AiCard settings={settings} status={aiStatus} onSave={save} onStatus={refreshAi} />
          )}

          {section === "repos" && (
            <>
              <section className="card">
                <h2>Repositories</h2>
                <p className="muted">Hidden repositories are still synced but never shown or notified.</p>
                {repos.length === 0 && (
                  <div className="empty">No repositories yet — they appear after the first sync.</div>
                )}
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
              {settings && <IgnoredUsers settings={settings} onSave={save} />}

              <CodeAndEditor repos={repos.filter((r) => !r.hidden && r.openPrs > 0).map((r) => r.repo)} />
            </>
          )}

          {section === "following" && <Following version={version} embedded />}
        </div>
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
        roots
          .split("\n")
          .map((r) => r.trim())
          .filter(Boolean),
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
          {Object.keys(es.clones).length} clones found. “Check out” only switches branch when you have no uncommitted
          changes.
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
export function JiraCard() {
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
              {status.tickets} tickets ·{" "}
              {status.lastSync
                ? `synced ${friendlyDate(status.lastSync)} ${clock(status.lastSync)}`
                : "first sync running…"}
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
        Shows tickets that involve you (assigned, reported or worked on), and how they moved, in Today, Tickets and the
        Day view. From Tickets you can move a ticket's status and assign it; nothing else is written to Jira. The token
        is stored in your system keychain.
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
          <input
            className="grow"
            placeholder="yourcompany.atlassian.net"
            value={site}
            onChange={(e) => setSite(e.target.value)}
          />
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

export function HarvestCard() {
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
        Jira ticket, in the project and task you pick (remembered per Jira project). Only entries this app made are ever
        changed or removed. The token is stored in your system keychain.
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

export function UpdatesCard() {
  const status = useUpdates();
  const [checking, setChecking] = useState(false);
  if (!status) return null;
  const run = async (install: boolean) => {
    setChecking(true);
    try {
      const s = await api.checkForUpdates(install);
      if (!s.available && !s.error) toast(`Dev Inbox ${s.current} is the latest version`);
    } catch (e) {
      toast(String(e), true);
    } finally {
      setChecking(false);
    }
  };
  const busy = checking || status.busy;
  let line: React.ReactNode;
  if (status.installed) line = <>Version {status.installed} is installed — it opens next time Dev Inbox starts.</>;
  else if (busy) line = status.available ? <>Downloading {status.available}…</> : <>Checking…</>;
  else if (status.available) line = <>Version {status.available} is available.</>;
  else if (status.lastCheck)
    line = (
      <>
        Up to date · checked {friendlyDate(status.lastCheck)} {clock(status.lastCheck)}
      </>
    );
  else line = <>Checks GitHub for new versions a minute after launch, then every 6 hours.</>;

  return (
    <section className="card">
      <h2>Updates</h2>
      <div className="account">
        <span className="state-icon tone-blue">
          <Icon name="refresh" size={14} className={busy ? "spin" : ""} />
        </span>
        <div className="grow">
          <div className="strong">Dev Inbox {status.current}</div>
          <div className="muted small">
            {line}
            {status.error && <span className="error"> · {status.error}</span>}
          </div>
        </div>
      </div>
      {status.notes && (status.available || status.installed) && (
        <p className="muted small update-notes">{status.notes}</p>
      )}
      <label className="check">
        <input type="checkbox" checked={status.automatic} onChange={(e) => void api.setAutoUpdate(e.target.checked)} />
        Download and install updates automatically
      </label>
      <div className="actions">
        {status.installed ? (
          <button className="primary" onClick={() => api.restartToUpdate().catch((e) => toast(String(e), true))}>
            Restart now
          </button>
        ) : status.available && !status.automatic && status.canInstall ? (
          <button className="primary" disabled={busy} onClick={() => void run(true)}>
            Download and install {status.available}
          </button>
        ) : null}
        <button disabled={busy} onClick={() => void run(false)}>
          {busy ? "Checking…" : "Check now"}
        </button>
        <button className="link small" onClick={() => openUrl("https://github.com/hrowan1/gitapp/releases")}>
          Release notes
        </button>
      </div>
      {!status.canInstall && (
        <p className="muted small">This is a development build: it can see new versions but doesn't install them.</p>
      )}
    </section>
  );
}

const PRIVACY_CALENDARS = "x-apple.systempreferences:com.apple.preference.security?Privacy_Calendars";

/** Meetings from the Mac's calendars: access, which calendars, reminders, and counting them as time. */
export function CalendarCard({ compact }: { compact?: boolean }) {
  const [status, setStatus] = useState<CalendarStatus | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    api.calendarStatus().then(setStatus).catch(() => {});
  }, []);
  if (!status) return null;

  const run = async (fn: () => Promise<CalendarStatus>, done?: string) => {
    setBusy(true);
    try {
      setStatus(await fn());
      if (done) toast(done);
    } catch (e) {
      toast(String(e), true);
      api.calendarStatus().then(setStatus).catch(() => {});
    } finally {
      setBusy(false);
    }
  };
  const save = (selected: string[], remind: number, countTime: boolean) =>
    run(() => api.calendarSave(selected, remind, countTime));
  const all = status.selected.length === 0;
  const isOn = (id: string) => all || status.selected.includes(id);
  const toggle = (id: string) => {
    const current = all ? status.calendars.map((c) => c.id) : status.selected;
    const next = current.includes(id) ? current.filter((c) => c !== id) : [...current, id];
    // Everything ticked is stored as "all", so new calendars are included too.
    void save(next.length === status.calendars.length ? [] : next, status.remind, status.countTime);
  };

  if (!status.enabled || status.access !== "full") {
    return (
      <section className="card">
        <h2>Calendar</h2>
        <p className="muted">
          Your meetings from the Mac's Calendar app — Google, Outlook and iCloud calendars it syncs. They show in the Day
          view and timesheet even when you don't record them, a reminder comes before each one with a Record button, and
          Meetings shows what's next with a brief: its ticket, open PRs and what's left from last time. Read-only; nothing
          leaves this Mac.
        </p>
        {status.access === "denied" || status.access === "writeOnly" ? (
          <>
            <p className="warn small">
              Calendar access is turned off for Dev Inbox. Allow full access in System Settings → Privacy &amp; Security
              → Calendars, then connect again.
            </p>
            <div className="actions">
              <button onClick={() => openUrl(PRIVACY_CALENDARS)}>Open Privacy settings</button>
              <button className="primary" disabled={busy} onClick={() => void run(api.calendarConnect, "Calendar connected")}>
                {busy ? "Connecting…" : "Try again"}
              </button>
            </div>
          </>
        ) : (
          <div className="actions">
            <button className="primary" disabled={busy} onClick={() => void run(api.calendarConnect, "Calendar connected")}>
              <Icon name="calendar" size={14} /> {busy ? "Waiting for permission…" : "Connect calendar"}
            </button>
            {status.access === "notDetermined" && <span className="muted small">macOS will ask to allow access.</span>}
          </div>
        )}
      </section>
    );
  }

  return (
    <section className="card">
      <h2>Calendar</h2>
      <div className="account">
        <span className="state-icon tone-blue">
          <Icon name="calendar" size={14} />
        </span>
        <div className="grow">
          <div className="strong">
            {all ? "All calendars" : `${status.selected.length} of ${status.calendars.length} calendars`}
          </div>
          <div className="muted small">
            {status.lastSync ? `Synced ${friendlyDate(status.lastSync)} ${clock(status.lastSync)}` : "Syncing…"}
            {status.error && <span className="error"> · {status.error}</span>}
          </div>
        </div>
      </div>
      {!compact && (
        <div className="calendar-list">
          {status.calendars.map((c) => (
            <label key={c.id} className="check">
              <input type="checkbox" checked={isOn(c.id)} disabled={busy} onChange={() => toggle(c.id)} />
              <span className="cal-dot" style={{ background: c.color }} />
              {c.title}
              <span className="muted small">{c.source}</span>
            </label>
          ))}
        </div>
      )}
      <label className="field">
        <span>Remind me</span>
        <select
          value={status.remind}
          disabled={busy}
          onChange={(e) => void save(status.selected, Number(e.target.value), status.countTime)}
        >
          <option value={0}>Off</option>
          <option value={1}>1 minute before</option>
          <option value={2}>2 minutes before</option>
          <option value={5}>5 minutes before</option>
          <option value={10}>10 minutes before</option>
          <option value={15}>15 minutes before</option>
        </select>
      </label>
      <label className="check">
        <input
          type="checkbox"
          checked={status.countTime}
          disabled={busy}
          onChange={(e) => void save(status.selected, status.remind, e.target.checked)}
        />
        Count meetings as time in the Day view and timesheet (ones you recorded count once)
      </label>
      <p className="muted small">
        Only meetings count: events with other people or a call link that you haven't declined — not all-day events,
        "Lunch" or focus blocks. A meeting with a ticket key in its title counts towards that ticket.
      </p>
      {!compact && (
        <div className="actions">
          <button disabled={busy} onClick={() => void run(api.calendarSyncNow, "Calendar synced")}>
            Sync now
          </button>
          <button
            className="danger-outline"
            onClick={async () => {
              await api.calendarDisconnect();
              setStatus(await api.calendarStatus());
            }}
          >
            Turn off
          </button>
        </div>
      )}
    </section>
  );
}
