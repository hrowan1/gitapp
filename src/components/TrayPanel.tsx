import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { api, type Inbox, type Item, type MeetingAction, type RecordingStatus, type TicketAttention } from "../lib/api";
import { openItem } from "../lib/actions";
import { groupItems, shortRef } from "../lib/states";
import { age } from "../lib/time";
import { Icon } from "./Icon";
import { StateIcon } from "./StateIcon";

const SHOW = 4;

function mmss(total: number): string {
  const s = Math.max(0, Math.floor(total));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** Open something in the main window and put the panel away. */
const openInMain = (kind: "view" | "ticket" | "meeting" | "palette", target?: string) =>
  invoke("open_in_main", { kind, target: target ?? null });
const hide = () => invoke("hide_panel");

/** The menu bar panel: what needs you, at a glance, without opening the app. */
export function TrayPanel() {
  const [inbox, setInbox] = useState<Inbox | null>(null);
  const [tickets, setTickets] = useState<TicketAttention[]>([]);
  const [actions, setActions] = useState<MeetingAction[]>([]);
  const [recording, setRecording] = useState<RecordingStatus>({ recording: false });
  const [seconds, setSeconds] = useState(0);
  const [now, setNow] = useState(Date.now());
  const body = useRef<HTMLDivElement>(null);

  const load = useCallback(() => {
    setNow(Date.now());
    api.getInbox().then(setInbox).catch(() => {});
    api.getTicketAttention().then(setTickets).catch(() => {});
    api.getMeetingActions().then(setActions).catch(() => {});
    api.recordingStatus().then(setRecording).catch(() => {});
  }, []);

  useEffect(() => {
    document.documentElement.classList.add("panel-window");
    load();
    const subs = [
      listen("panel-shown", load),
      listen("inbox-updated", load),
      listen("jira-updated", load),
      listen("meetings-updated", load),
      listen("recording-event", load),
      listen<{ seconds: number }>("recording-level", (e) => setSeconds(e.payload.seconds)),
    ];
    const close = (e: KeyboardEvent) => e.key === "Escape" && void hide();
    window.addEventListener("keydown", close);
    return () => {
      subs.forEach((p) => p.then((f) => f()));
      window.removeEventListener("keydown", close);
    };
  }, [load]);

  // Fit the window to what's in it.
  useLayoutEffect(() => {
    const h = body.current?.scrollHeight ?? 0;
    if (h > 0) void getCurrentWindow().setSize(new LogicalSize(380, Math.min(620, Math.max(180, h))));
  });

  const items: Item[] = inbox ? groupItems(inbox.items).needs : [];
  const total = items.length + tickets.length + actions.length;
  const syncedAt = inbox?.lastSync;

  return (
    <div className="panel" ref={body}>
      <header className="panel-head">
        <div className="panel-title">
          <span className="strong">
            {total === 0 ? "You're all caught up" : total === 1 ? "1 thing needs you" : `${total} things need you`}
          </span>
          <span className="muted small">
            {inbox?.syncing ? "Syncing…" : syncedAt ? `Synced ${age(syncedAt, now)} ago` : "Not synced yet"}
          </span>
        </div>
        <button className="ghost icon-only" title="Sync now" onClick={() => void api.syncNow()}>
          <Icon name="refresh" size={14} />
        </button>
        <button className="ghost icon-only" title="Open Dev Inbox" onClick={() => void openInMain("view", "today")}>
          <Icon name="external" size={14} />
        </button>
      </header>

      {recording.recording && (
        <div className="panel-rec">
          <span className="rec-dot" />
          <span className="strong">Recording</span>
          <span className="rec-time">{mmss(seconds)}</span>
          <span className="grow" />
          <button className="danger small-btn" onClick={() => void api.stopRecording()}>
            <Icon name="stop" size={11} /> Stop
          </button>
        </div>
      )}

      <div className="panel-body">
        {total === 0 && (
          <div className="panel-empty">
            <Icon name="checkCircle" size={22} />
            <span className="muted small">Nothing needs you right now.</span>
          </div>
        )}

        {items.length > 0 && (
          <Section title="Needs you" count={items.length} onMore={() => void openInMain("view", "today")}>
            {items.slice(0, SHOW).map((i) => (
              <button
                key={i.id}
                className="panel-row"
                onClick={() => {
                  openItem(i);
                  void hide();
                }}
                title="Open on GitHub"
              >
                <StateIcon state={i.state} size={13} />
                <span className="panel-row-main">
                  <span className="panel-row-title">{i.title}</span>
                  <span className="panel-row-sub">
                    {shortRef(i)} · {i.reason}
                  </span>
                </span>
              </button>
            ))}
          </Section>
        )}

        {tickets.length > 0 && (
          <Section title="Jira" count={tickets.length} onMore={() => void openInMain("view", "tickets")}>
            {tickets.slice(0, SHOW).map((t) => (
              <button key={t.key} className="panel-row" onClick={() => void openInMain("ticket", t.key)}>
                <span className={`state-icon tone-${t.kind === "mention" || t.kind === "assignee" ? "orange" : "blue"}`}>
                  <Icon name={t.kind === "mention" || t.kind === "comment" ? "message" : "ticket"} size={12} />
                </span>
                <span className="panel-row-main">
                  <span className="panel-row-title">
                    <span className="ticket-key">{t.key}</span> {t.summary}
                  </span>
                  <span className="panel-row-sub">{t.reason}</span>
                </span>
              </button>
            ))}
          </Section>
        )}

        {actions.length > 0 && (
          <Section title="From your meetings" count={actions.length} onMore={() => void openInMain("view", "today")}>
            {actions.slice(0, SHOW).map((a) => (
              <div key={a.id} className="panel-row" role="button" onClick={() => void openInMain("meeting", a.meetingId)}>
                <button
                  className="tick"
                  title="Done"
                  onClick={(e) => {
                    e.stopPropagation();
                    void api.setActionDone(a.id, true).then(load);
                  }}
                >
                  <Icon name="check" size={10} />
                </button>
                <span className="panel-row-main">
                  <span className="panel-row-title">{a.text}</span>
                  <span className="panel-row-sub">{a.meetingTitle}</span>
                </span>
              </div>
            ))}
          </Section>
        )}
      </div>

      <footer className="panel-foot">
        {!recording.recording && (
          <button
            className="panel-btn"
            onClick={() => {
              void openInMain("view", "meetings");
              void api.startRecording();
            }}
          >
            <Icon name="mic" size={13} /> Record
          </button>
        )}
        <button className="panel-btn" onClick={() => void openInMain("palette")}>
          <Icon name="sparkle" size={13} /> Ask or act <kbd>⌥⌘K</kbd>
        </button>
        <span className="grow" />
        <button className="ghost icon-only" title="Settings" onClick={() => void openInMain("view", "settings")}>
          <Icon name="settings" size={14} />
        </button>
      </footer>
    </div>
  );
}

function Section({
  title,
  count,
  onMore,
  children,
}: {
  title: string;
  count: number;
  onMore: () => void;
  children: React.ReactNode;
}) {
  return (
    <section className="panel-section">
      <div className="panel-section-head">
        <span>{title}</span>
        <span className="count">{count}</span>
        <span className="grow" />
        {count > SHOW && (
          <button className="link small" onClick={onMore}>
            View all
          </button>
        )}
      </div>
      {children}
    </section>
  );
}
