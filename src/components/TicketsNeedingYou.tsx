import { useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, type TicketAttention } from "../lib/api";
import { age } from "../lib/time";
import { toast } from "../lib/toast";
import { Icon, type IconName } from "./Icon";

const KIND_ICON: Record<string, IconName> = {
  assignee: "ticket",
  mention: "message",
  comment: "message",
  status: "chevronRight",
  resolution: "checkCircle",
};

/** Jira tickets where someone else did something since you last looked. Opening one (or ✓) clears it. */
export function TicketsNeedingYou({
  tickets,
  now,
  onOpen,
}: {
  tickets: TicketAttention[];
  now: number;
  onOpen: (key: string) => void;
}) {
  const [closed, setClosed] = useState(false);
  if (tickets.length === 0) return null;
  const seen = (key: string) => api.markTicketSeen(key).catch((e) => toast(String(e), true));
  return (
    <div className="list ticket-attention">
      <section className="section">
        <button className="section-head" onClick={() => setClosed(!closed)}>
          <Icon name="chevronDown" size={13} className={`chevron${closed ? " closed" : ""}`} />
          <span className="section-title">Jira tickets</span>
          <span className="count">{tickets.length}</span>
        </button>
        {!closed &&
          tickets.map((t) => (
            <div key={t.key} className="row" role="button" onClick={() => onOpen(t.key)} title="Open in Tickets">
              <span className={`state-icon tone-${t.kind === "mention" || t.kind === "assignee" ? "orange" : "blue"}`}>
                <Icon name={KIND_ICON[t.kind] ?? "ticket"} size={14} />
              </span>
              <div className="row-main">
                <div className="row-title">
                  <span className="ticket-key">{t.key}</span>
                  <span className="title">{t.summary}</span>
                </div>
                <div className="row-sub">
                  <span className="reason">{t.reason}</span>
                  {t.count > 1 && <span className="pill">+{t.count - 1} more</span>}
                  <span className="sep">·</span>
                  <span>{t.status}</span>
                </div>
              </div>
              <div className="row-right">
                <span className="age">{age(t.at, now)}</span>
                <button
                  className="ghost icon-only row-action"
                  title="Open in Jira"
                  onClick={(e) => {
                    e.stopPropagation();
                    void seen(t.key);
                    openUrl(t.url);
                  }}
                >
                  <Icon name="external" size={13} />
                </button>
                <button
                  className="ghost icon-only row-action"
                  title="Seen it — clear from Today"
                  onClick={(e) => {
                    e.stopPropagation();
                    void seen(t.key);
                  }}
                >
                  <Icon name="check" size={13} />
                </button>
              </div>
            </div>
          ))}
      </section>
    </div>
  );
}
