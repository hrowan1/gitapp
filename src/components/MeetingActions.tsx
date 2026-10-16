import { useEffect, useState } from "react";
import { api, type MeetingAction } from "../lib/api";
import { friendlyDate, SNOOZE_PRESETS } from "../lib/time";
import { toast } from "../lib/toast";
import { Icon } from "./Icon";

/** Snoozes with a time ("until something changes" means nothing for an action item). */
const TIMED_PRESETS = SNOOZE_PRESETS.filter((p) => p.until() !== null);

/** Tick an action item off (with undo), or put it back. */
export async function setActionDone(a: MeetingAction, done: boolean) {
  try {
    await api.setActionDone(a.id, done);
    if (done) toast("Done", false, { label: "Undo", run: () => void api.setActionDone(a.id, false) });
  } catch (e) {
    toast(String(e), true);
  }
}

/** Today: what you said you'd do in meetings. Tick it off, snooze it, or open the meeting. */
export function MeetingActions({ actions, onOpenMeeting }: { actions: MeetingAction[]; onOpenMeeting: (id: string) => void }) {
  const [closed, setClosed] = useState(false);
  const [menu, setMenu] = useState<number | null>(null);
  useEffect(() => {
    if (menu === null) return;
    const close = () => setMenu(null);
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, [menu]);
  if (actions.length === 0) return null;

  const snooze = async (a: MeetingAction, until: string, label: string) => {
    setMenu(null);
    try {
      await api.snoozeAction(a.id, until);
      toast(`Back ${label.charAt(0).toLowerCase()}${label.slice(1)}`, false, {
        label: "Undo",
        run: () => void api.snoozeAction(a.id, null),
      });
    } catch (e) {
      toast(String(e), true);
    }
  };

  return (
    <div className="list meeting-actions">
      <section className="section">
        <button className="section-head" onClick={() => setClosed(!closed)}>
          <Icon name="chevronDown" size={13} className={`chevron${closed ? " closed" : ""}`} />
          <span className="section-title">From your meetings</span>
          <span className="count">{actions.length}</span>
        </button>
        {!closed &&
          actions.map((a) => (
            <div key={a.id} className="row" role="button" onClick={() => onOpenMeeting(a.meetingId)} title="Open the meeting">
              <button
                className="tick"
                title="Done"
                onClick={(e) => {
                  e.stopPropagation();
                  void setActionDone(a, true);
                }}
              >
                <Icon name="check" size={11} />
              </button>
              <div className="row-main">
                <div className="row-title">
                  <span className="title">{a.text}</span>
                </div>
                <div className="row-sub">
                  <Icon name="mic" size={11} />
                  <span>{a.meetingTitle}</span>
                  {a.ticket && !a.meetingTitle.toUpperCase().includes(a.ticket) && <span className="ticket-key">{a.ticket}</span>}
                  <span className="sep">·</span>
                  <span>{friendlyDate(a.meetingAt)}</span>
                </div>
              </div>
              <div className="row-right">
                <button
                  className="ghost icon-only row-action"
                  title="Come back to it later"
                  onClick={(e) => {
                    e.stopPropagation();
                    setMenu(menu === a.id ? null : a.id);
                  }}
                >
                  <Icon name="clock" size={13} />
                </button>
              </div>
              {menu === a.id && (
                <div className="menu" onClick={(e) => e.stopPropagation()}>
                  <div className="menu-label">Come back to it</div>
                  {TIMED_PRESETS.map((p) => (
                    <button key={p.id} className="menu-item" onClick={() => snooze(a, p.until() ?? "", p.label)}>
                      <Icon name="pin" size={14} /> {p.label}
                    </button>
                  ))}
                </div>
              )}
            </div>
          ))}
      </section>
    </div>
  );
}
