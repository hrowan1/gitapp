import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, type Inbox, type Session } from "./lib/api";
import { groupItems } from "./lib/states";
import { age } from "./lib/time";
import { CommandPalette, type View } from "./components/CommandPalette";
import { AllPrs, History, SettingsView, SignIn, Today } from "./components/Views";
import { onToast, type Toast } from "./lib/toast";
import "./styles.css";

const VIEWS: { id: View; label: string }[] = [
  { id: "today", label: "Today" },
  { id: "all", label: "All" },
  { id: "history", label: "History" },
  { id: "settings", label: "Settings" },
];

export default function App() {
  const [session, setSession] = useState<Session | null | undefined>(undefined);
  const [inbox, setInbox] = useState<Inbox | null>(null);
  const [view, setView] = useState<View>("today");
  const [palette, setPalette] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [now, setNow] = useState(Date.now());
  const [version, setVersion] = useState(0);
  const [toasts, setToasts] = useState<Toast[]>([]);

  useEffect(
    () =>
      onToast((t) => {
        setToasts((ts) => [...ts.slice(-2), t]);
        setTimeout(() => setToasts((ts) => ts.filter((x) => x.id !== t.id)), t.error ? 6000 : 3000);
      }),
    [],
  );

  const reload = useCallback(async () => {
    const next = await api.getInbox();
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
    ];
    const tick = setInterval(() => setNow(Date.now()), 30_000);
    return () => {
      unlisten.forEach((p) => p.then((f) => f()));
      clearInterval(tick);
    };
  }, [session, reload]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPalette((p) => !p);
        return;
      }
      const tag = (document.activeElement?.tagName ?? "").toLowerCase();
      if (palette || tag === "input" || tag === "select" || e.metaKey || e.ctrlKey || e.altKey) return;
      const idx = ["1", "2", "3", "4"].indexOf(e.key);
      if (idx >= 0) setView(VIEWS[idx].id);
      else if (e.key === "r") sync();
      else if (e.key === "/") {
        e.preventDefault();
        setPalette(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [palette, sync]);

  if (session === undefined) return null;
  if (session === null) return <SignIn onSignedIn={setSession} />;

  const items = inbox?.items ?? [];
  const needs = groupItems(items).needs.length;
  const firstLoad = !inbox?.lastSync;

  return (
    <div className="app">
      <header className="topbar">
        <nav className="tabs">
          {VIEWS.map((v) => (
            <button key={v.id} className={`tab${view === v.id ? " active" : ""}`} onClick={() => setView(v.id)}>
              {v.label}
              {v.id === "today" && needs > 0 && <span className="badge">{needs}</span>}
            </button>
          ))}
        </nav>
        <button className="ask" onClick={() => setPalette(true)}>
          <span>What needs my attention?</span>
          <kbd>⌘K</kbd>
        </button>
        <button className="sync" onClick={sync} disabled={syncing} title="Sync now (r)">
          <span className={syncing ? "spin" : ""}>↻</span>
          <span className="muted small">
            {syncing ? "Syncing…" : inbox?.lastSync ? `Synced ${age(inbox.lastSync, now)} ago` : "Not synced"}
          </span>
        </button>
      </header>

      {inbox?.lastError && (
        <div className="banner error">
          Last sync failed: {inbox.lastError}{" "}
          <button className="link" onClick={sync}>
            Retry
          </button>
        </div>
      )}

      <main className="content">
        {firstLoad && view !== "settings" ? (
          <div className="empty big">
            <div className="spin big">↻</div>
            Fetching your pull requests from GitHub… the first sync can take up to a minute.
          </div>
        ) : (
          <>
            {view === "today" && <Today items={items} now={now} active={!palette} />}
            {view === "all" && <AllPrs items={items} now={now} active={!palette} />}
            {view === "history" && <History version={version} />}
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
          </div>
        ))}
      </div>

      {palette && (
        <CommandPalette
          items={items}
          onClose={() => setPalette(false)}
          onNavigate={(v) => {
            setView(v);
            setPalette(false);
          }}
          onSync={sync}
        />
      )}
    </div>
  );
}
