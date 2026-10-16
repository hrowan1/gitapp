import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, type Inbox, type Session } from "./lib/api";
import { groupItems } from "./lib/states";
import { CommandPalette, type View } from "./components/CommandPalette";
import { NAV, Sidebar } from "./components/Sidebar";
import { Shortcuts } from "./components/Shortcuts";
import { Icon } from "./components/Icon";
import { AllPrs, History, SettingsView, SignIn, Today } from "./components/Views";
import { Following } from "./components/Following";
import { setAllItems } from "./lib/store";
import { onToast, type Toast } from "./lib/toast";
import { onDialog, type DialogRequest } from "./lib/dialog";
import { ActionDialog } from "./components/ActionDialog";
import { DayView } from "./components/DayView";
import { TicketsView } from "./components/TicketsView";
import { MeetingsView } from "./components/MeetingsView";
import type { MeetingAction, RecordingStatus, TicketAttention } from "./lib/api";
import "./styles.css";

export default function App() {
  const [session, setSession] = useState<Session | null | undefined>(undefined);
  const [inbox, setInbox] = useState<Inbox | null>(null);
  const [view, setView] = useState<View>("today");
  const [palette, setPalette] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [now, setNow] = useState(Date.now());
  const [version, setVersion] = useState(0);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [dialog, setDialog] = useState<DialogRequest | null>(null);
  const [shortcuts, setShortcuts] = useState(false);
  /** A date to show in the Day view (e.g. from a ticket's history). */
  const [dayTarget, setDayTarget] = useState<{ day: string } | null>(null);
  const [ticketTarget, setTicketTarget] = useState<{ key: string } | null>(null);
  const [meetingTarget, setMeetingTarget] = useState<{ id: string } | null>(null);
  /** Jira tickets that need you, for Today and its count. */
  const [attention, setAttention] = useState<TicketAttention[]>([]);
  /** Action items of yours from meeting notes, for Today and its count. */
  const [actions, setActions] = useState<MeetingAction[]>([]);
  const openMeeting = useCallback((id: string) => {
    setMeetingTarget({ id });
    setView("meetings");
  }, []);
  const openTicket = useCallback((key: string) => {
    setTicketTarget({ key });
    setView("tickets");
  }, []);
  // Navigating yourself (sidebar, keys, ⌘K) starts each view fresh rather than at the last jump target.
  const go = useCallback((v: View) => {
    setDayTarget(null);
    setTicketTarget(null);
    setMeetingTarget(null);
    setView(v);
  }, []);
  const [recording, setRecording] = useState<RecordingStatus>({ recording: false });
  const refreshRecording = useCallback(() => {
    api.recordingStatus().then(setRecording).catch(() => {});
  }, []);
  const overlay = palette || !!dialog || shortcuts;

  useEffect(() => onDialog(setDialog), []);

  useEffect(
    () =>
      onToast((t) => {
        setToasts((ts) => [...ts.slice(-2), t]);
        setTimeout(() => setToasts((ts) => ts.filter((x) => x.id !== t.id)), t.error || t.action ? 8000 : 3000);
      }),
    [],
  );

  const reload = useCallback(async () => {
    const next = await api.getInbox();
    setAllItems(next.items);
    setInbox(next);
    setSyncing(next.syncing);
    setVersion((v) => v + 1);
  }, []);

  const sync = useCallback(() => {
    setSyncing(true);
    api.syncNow().catch(() => {}).finally(reload);
  }, [reload]);

  useEffect(() => {
    api.getSession().then(setSession);
  }, []);

  useEffect(() => {
    if (!session) return;
    reload();
    const unlisten = [
      listen("inbox-updated", reload),
      listen("sync-started", () => setSyncing(true)),
      listen("open-palette", () => setPalette(true)),
      listen("jira-updated", () => setVersion((v) => v + 1)),
      listen("meetings-updated", () => {
        refreshRecording();
        setVersion((v) => v + 1);
      }),
      listen("recording-event", refreshRecording),
    ];
    const tick = setInterval(() => setNow(Date.now()), 30_000);
    return () => {
      unlisten.forEach((p) => p.then((f) => f()));
      clearInterval(tick);
    };
  }, [session, reload, refreshRecording]);

  useEffect(() => {
    if (session) refreshRecording();
  }, [session, refreshRecording]);

  useEffect(() => {
    if (!session) return;
    api.getTicketAttention().then(setAttention).catch(() => {});
    api.getMeetingActions().then(setActions).catch(() => {});
  }, [session, version]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPalette((p) => !p);
        return;
      }
      const tag = (document.activeElement?.tagName ?? "").toLowerCase();
      if (overlay || tag === "input" || tag === "select" || tag === "textarea" || e.metaKey || e.ctrlKey || e.altKey) return;
      const nav = NAV.find((n) => n.key === e.key);
      // preventDefault so the key isn't also typed into whatever the shortcut opens.
      if (nav) go(nav.id);
      else if (e.key === "r") sync();
      else if (e.key === "s") go("day");
      else if (e.key === "/") setPalette(true);
      else if (e.key === "?") setShortcuts(true);
      else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [palette, overlay, sync, go]);

  if (session === undefined) return null;
  if (session === null) return <SignIn onSignedIn={setSession} />;

  const items = inbox?.items ?? [];
  const needs = groupItems(items).needs.length;
  const firstLoad = !inbox?.lastSync;

  return (
    <div className="app shell">
      <Sidebar
        view={view}
        onView={go}
        counts={{ today: needs + attention.length + actions.length, all: items.length }}
        session={session}
        syncing={syncing}
        lastSync={inbox?.lastSync}
        now={now}
        onSync={sync}
        onAsk={() => setPalette(true)}
        onShortcuts={() => setShortcuts(true)}
        recording={recording.recording}
      />

      <main className="main">
        {inbox?.lastError && (
          <div className="banner error">
            <Icon name="alert" size={14} />
            <span className="grow">Last sync failed: {inbox.lastError}</span>
            <button className="link" onClick={sync}>
              Retry
            </button>
          </div>
        )}
        {firstLoad && !["settings", "following", "day", "tickets", "meetings"].includes(view) ? (
          <div className="empty big">
            <Icon name="refresh" size={22} className="spin" />
            <p>Fetching your pull requests and issues from GitHub…</p>
            <p className="muted small">The first sync can take up to a minute.</p>
          </div>
        ) : (
          <>
            {view === "today" && (
              <Today
                items={items}
                tickets={attention}
                actions={actions}
                now={now}
                active={!overlay}
                onOpenTicket={openTicket}
                onOpenMeeting={openMeeting}
              />
            )}
            {view === "all" && <AllPrs items={items} now={now} active={!overlay} />}
            {view === "day" && (
              <DayView
                version={version}
                target={dayTarget}
                onOpenTicket={openTicket}
                onOpenMeeting={openMeeting}
              />
            )}
            {view === "meetings" && (
              <MeetingsView
                version={version}
                recording={recording}
                target={meetingTarget}
                onOpenDay={(day) => {
                  setDayTarget({ day });
                  setView("day");
                }}
              />
            )}
            {view === "tickets" && (
              <TicketsView
                version={version}
                now={now}
                target={ticketTarget}
                onOpenDay={(day) => {
                  setDayTarget({ day });
                  setView("day");
                }}
                onSettings={() => setView("settings")}
              />
            )}
            {view === "history" && <History version={version} onOpenDay={() => setView("day")} />}
            {view === "following" && <Following version={version} />}
          </>
        )}
        {view === "settings" && (
          <SettingsView
            session={session}
            onSignedOut={() => {
              setInbox(null);
              setSession(null);
            }}
          />
        )}
      </main>

      <div className="toasts">
        {toasts.map((t) => (
          <div key={t.id} className={`toast${t.error ? " error" : ""}`}>
            {t.text}
            {t.action && (
              <button
                className="toast-action"
                onClick={() => {
                  setToasts((ts) => ts.filter((x) => x.id !== t.id));
                  t.action!.run();
                }}
              >
                {t.action.label}
              </button>
            )}
          </div>
        ))}
      </div>

      {palette && (
        <CommandPalette
          items={items}
          onClose={() => setPalette(false)}
          onNavigate={(v) => {
            go(v);
            setPalette(false);
          }}
          onSync={sync}
          onOpenTicket={(key) => {
            openTicket(key);
            setPalette(false);
          }}
        />
      )}
      {dialog && <ActionDialog request={dialog} onClose={() => setDialog(null)} />}
      {shortcuts && <Shortcuts onClose={() => setShortcuts(false)} />}
    </div>
  );
}
