import type { Session } from "../lib/api";
import { age } from "../lib/time";
import type { View } from "./CommandPalette";
import { Icon, type IconName } from "./Icon";

export const NAV: { id: View; label: string; icon: IconName; key: string }[] = [
  { id: "today", label: "Today", icon: "today", key: "1" },
  { id: "all", label: "Inbox", icon: "inbox", key: "2" },
  { id: "history", label: "History", icon: "history", key: "3" },
  { id: "following", label: "Following", icon: "eye", key: "4" },
  { id: "settings", label: "Settings", icon: "settings", key: "5" },
];

export function Sidebar({
  view,
  onView,
  counts,
  session,
  syncing,
  lastSync,
  now,
  onSync,
  onAsk,
  onShortcuts,
}: {
  view: View;
  onView: (v: View) => void;
  counts: Partial<Record<View, number>>;
  session: Session;
  syncing: boolean;
  lastSync?: string | null;
  now: number;
  onSync: () => void;
  onAsk: () => void;
  onShortcuts: () => void;
}) {
  return (
    <nav className="sidebar">
      <div className="brand">
        <span className="brand-mark">
          <Icon name="inbox" size={15} />
        </span>
        <span className="brand-name">Dev Inbox</span>
      </div>

      <button className="ask-btn" onClick={onAsk} title="Ask or act (⌘K)">
        <Icon name="search" size={14} />
        <span className="grow">Ask or act…</span>
        <kbd>⌘K</kbd>
      </button>

      <div className="nav">
        {NAV.map((n) => (
          <button
            key={n.id}
            className={`nav-item${view === n.id ? " active" : ""}`}
            onClick={() => onView(n.id)}
            title={`${n.label} (${n.key})`}
          >
            <Icon name={n.icon} size={16} />
            <span className="nav-label">{n.label}</span>
            {!!counts[n.id] && <span className={`nav-count${n.id === "today" ? " hot" : ""}`}>{counts[n.id]}</span>}
          </button>
        ))}
      </div>

      <div className="sidebar-foot">
        <button className="nav-item" onClick={onShortcuts} title="Keyboard shortcuts (?)">
          <Icon name="keyboard" size={16} />
          <span className="nav-label">Shortcuts</span>
          <kbd>?</kbd>
        </button>
        <button className="sync-status" onClick={onSync} disabled={syncing} title="Sync now (r)">
          {session.avatarUrl ? <img className="avatar xs" src={session.avatarUrl} alt="" /> : null}
          <span className="nav-label grow">
            <span className="strong">{session.login}</span>
            <span className="muted small">
              {syncing ? "Syncing…" : lastSync ? `Synced ${age(lastSync, now)} ago` : "Not synced yet"}
            </span>
          </span>
          <Icon name="refresh" size={14} className={syncing ? "spin" : ""} />
        </button>
      </div>
    </nav>
  );
}
