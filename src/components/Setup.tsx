import { useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, type Session, type Settings, type SummaryStatus } from "../lib/api";
import { toast } from "../lib/toast";
import { Icon, type IconName } from "./Icon";
import { AiCard, CalendarCard, HarvestCard, JiraCard } from "./Settings";

type Step = "welcome" | "jira" | "harvest" | "calendar" | "notifications" | "ai" | "done";

const STEPS: { id: Step; label: string; icon: IconName; optional?: boolean }[] = [
  { id: "welcome", label: "GitHub", icon: "pr" },
  { id: "jira", label: "Jira", icon: "ticket", optional: true },
  { id: "harvest", label: "Harvest", icon: "clock", optional: true },
  { id: "calendar", label: "Calendar", icon: "calendar", optional: true },
  { id: "notifications", label: "Notifications", icon: "today" },
  { id: "ai", label: "AI", icon: "sparkle", optional: true },
  { id: "done", label: "Ready", icon: "checkCircle" },
];

const PRIVACY_NOTIFICATIONS = "x-apple.systempreferences:com.apple.Notifications-Settings.extension";

/** First run, after signing in to GitHub: connect the rest, allow notifications, and go. */
export function Setup({ session, onDone }: { session: Session; onDone: () => void }) {
  const [step, setStep] = useState<Step>("welcome");
  const [connected, setConnected] = useState<Partial<Record<Step, boolean>>>({ welcome: true });
  const [settings, setSettings] = useState<Settings | null>(null);
  const [aiStatus, setAiStatus] = useState<SummaryStatus | null>(null);
  const [tested, setTested] = useState<string | null>(null);
  const index = STEPS.findIndex((s) => s.id === step);

  // What's connected, for the ticks on the left (rechecked as you move through).
  useEffect(() => {
    api.jiraStatus().then((s) => setConnected((c) => ({ ...c, jira: s.connected }))).catch(() => {});
    api.harvestStatus().then((s) => setConnected((c) => ({ ...c, harvest: s.connected }))).catch(() => {});
    api
      .calendarStatus()
      .then((s) => setConnected((c) => ({ ...c, calendar: s.enabled && s.access === "full" })))
      .catch(() => {});
    api.summaryStatus().then(setAiStatus).catch(() => {});
    api.getSettings().then(setSettings).catch(() => {});
  }, [step]);

  const go = (to: Step) => setStep(to);
  const next = () => go(STEPS[Math.min(index + 1, STEPS.length - 1)].id);
  const back = () => go(STEPS[Math.max(index - 1, 0)].id);
  const finish = async () => {
    try {
      await api.finishSetup();
    } catch (e) {
      toast(String(e), true);
    }
    onDone();
  };
  const current = STEPS[index];

  return (
    <div className="setup">
      <div className="setup-card">
        <nav className="setup-steps">
          <div className="brand">
            <span className="brand-mark">
              <Icon name="inbox" size={15} />
            </span>
            <span className="brand-name">Dev Inbox</span>
          </div>
          {STEPS.map((s, i) => (
            <button
              key={s.id}
              className={`setup-step${s.id === step ? " active" : ""}${connected[s.id] ? " done" : ""}`}
              onClick={() => go(s.id)}
            >
              <span className="setup-step-icon">
                <Icon name={connected[s.id] ? "check" : s.icon} size={12} />
              </span>
              <span className="grow">{s.label}</span>
              {s.optional && !connected[s.id] && i < STEPS.length - 1 && <span className="muted small">optional</span>}
            </button>
          ))}
          <span className="grow" />
          <button className="link small setup-skip" onClick={() => void finish()}>
            Skip setup
          </button>
        </nav>

        <div className="setup-body">
          <div className="setup-content">
            {step === "welcome" && (
              <>
                <h1>Welcome, {session.login}</h1>
                <p className="lead">
                  Dev Inbox is connected to GitHub and is fetching your pull requests and issues now. A few optional
                  connections make it much more useful — each takes a minute, and you can change any of them later in
                  Settings.
                </p>
                <ul className="setup-list">
                  <li>
                    <Icon name="ticket" size={14} /> <b>Jira</b> — tickets that need you, how they moved, status and
                    assignee changes.
                  </li>
                  <li>
                    <Icon name="clock" size={14} /> <b>Harvest</b> — your week as a timesheet, sent in one click.
                  </li>
                  <li>
                    <Icon name="calendar" size={14} /> <b>Calendar</b> — meetings in your day, reminders with a Record
                    button, and a brief before each one.
                  </li>
                  <li>
                    <Icon name="sparkle" size={14} /> <b>AI</b> — meeting notes, day summaries and answers about your
                    history.
                  </li>
                </ul>
                <p className="muted small">Everything stays on this Mac. Tokens go in the macOS keychain.</p>
              </>
            )}

            {step === "jira" && (
              <>
                <h1>Jira</h1>
                <p className="lead">See tickets that involve you, and when someone moves, assigns or mentions you on one.</p>
                <JiraCard />
              </>
            )}

            {step === "harvest" && (
              <>
                <h1>Harvest</h1>
                <p className="lead">
                  Turn the Day view into a weekly timesheet and send it to Harvest, linked to each Jira ticket.
                </p>
                <HarvestCard />
              </>
            )}

            {step === "calendar" && (
              <>
                <h1>Calendar</h1>
                <p className="lead">
                  Uses the calendars already on this Mac (Google, Outlook, iCloud) — nothing to sign in to. macOS asks once
                  to allow access.
                </p>
                <CalendarCard compact />
              </>
            )}

            {step === "notifications" && (
              <>
                <h1>Notifications</h1>
                <p className="lead">
                  Dev Inbox tells you when a review is requested, someone replies, CI fails on your PR, a ticket comes
                  back to you, or a meeting is about to start. Clicking one opens it here.
                </p>
                <section className="card">
                  <div className="actions">
                    <button
                      className="primary"
                      onClick={() =>
                        api.testNotification().then(
                          (m) => setTested(m),
                          (e) => setTested(String(e)),
                        )
                      }
                    >
                      Send a test notification
                    </button>
                    <button onClick={() => openUrl(PRIVACY_NOTIFICATIONS)}>Notification settings</button>
                  </div>
                  {tested && <p className="small">{tested}</p>}
                  <p className="muted small">
                    Nothing arrived? Allow notifications for Dev Inbox in System Settings → Notifications, and check
                    Focus isn't hiding them.
                  </p>
                </section>
                <section className="card">
                  <p className="small setup-tip">
                    <Icon name="inbox" size={14} /> Dev Inbox lives in the menu bar too: click its icon for what needs
                    you, without opening the window. <kbd>⌥⌘K</kbd> opens it from any app.
                  </p>
                </section>
              </>
            )}

            {step === "ai" && (
              <>
                <h1>AI</h1>
                <p className="lead">
                  Apple Intelligence works on this Mac for free. Add an Anthropic API key to use Claude instead — better
                  notes, pennies a month.
                </p>
                {settings && (
                  <AiCard
                    settings={settings}
                    status={aiStatus}
                    onSave={(s) => {
                      setSettings(s);
                      void api.updateSettings(s);
                    }}
                    onStatus={() => void api.summaryStatus().then(setAiStatus)}
                  />
                )}
              </>
            )}

            {step === "done" && (
              <>
                <h1>You're set up</h1>
                <p className="lead">Here's what's connected. Anything else can be added in Settings → Connections.</p>
                <ul className="setup-summary">
                  {STEPS.filter((s) => s.id !== "done" && s.id !== "notifications").map((s) => (
                    <li key={s.id} className={connected[s.id] ? "on" : ""}>
                      <Icon name={connected[s.id] ? "checkCircle" : "minusCircle"} size={15} />
                      {s.id === "welcome" ? `GitHub as ${session.login}` : s.label}
                      {s.id === "ai" && aiStatus && (
                        <span className="muted small">
                          {aiStatus.claudeKey ? "Claude" : aiStatus.appleReady ? "Apple Intelligence" : "not available yet"}
                        </span>
                      )}
                      {!connected[s.id] && s.id !== "ai" && (
                        <button className="link small" onClick={() => go(s.id)}>
                          Connect
                        </button>
                      )}
                    </li>
                  ))}
                </ul>
                <p className="muted small">
                  Keys: <kbd>1</kbd>–<kbd>6</kbd> switch views, <kbd>⌘K</kbd> asks or acts on anything, <kbd>?</kbd>{" "}
                  shows every shortcut.
                </p>
              </>
            )}
          </div>

          <footer className="setup-foot">
            {index > 0 && <button onClick={back}>Back</button>}
            <span className="grow" />
            {step === "done" ? (
              <button className="primary" onClick={() => void finish()}>
                Open Dev Inbox
              </button>
            ) : (
              <button className="primary" onClick={next}>
                {current.optional && !connected[step] ? "Skip for now" : "Continue"}
              </button>
            )}
          </footer>
        </div>
      </div>
    </div>
  );
}
