import { useEffect, useRef, useState, type ReactElement } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, type Activity, type Item, type StateId } from "../lib/api";
import { copyCheckout, openInEditor, openItem, rerunChecks, setStates, unfollow } from "../lib/actions";
import { openDialog } from "../lib/dialog";
import { on } from "../lib/events";
import { MANUAL_STATES, STATES, shortRef } from "../lib/states";
import { matchTitles } from "../lib/store";
import { SNOOZE_PRESETS, age, clock, friendlyDate, untilLabel } from "../lib/time";
import { Icon, type IconName } from "./Icon";
import { StateChip } from "./StateIcon";

export type Selection = { item: Item } | { group: { repo: string; items: Item[] } } | null;

/** Right-hand pane: everything about the selection, and every action as a visible button. */
export function DetailPane({ selection, now, onClose }: { selection: Selection; now: number; onClose?: () => void }) {
  if (!selection) return <EmptyDetail />;
  return (
    <aside className="detail">
      {onClose && (
        <button className="detail-close ghost" onClick={onClose} title="Close (Esc)">
          <Icon name="x" />
        </button>
      )}
      {"item" in selection ? (
        <ItemDetail key={selection.item.id} item={selection.item} now={now} />
      ) : (
        <GroupDetail key={selection.group.repo} repo={selection.group.repo} items={selection.group.items} />
      )}
    </aside>
  );
}

function EmptyDetail() {
  const tips: [string, string][] = [
    ["j / k", "move through the list"],
    ["↵", "open on GitHub"],
    ["e", "done"],
    ["l", "come back later"],
    ["R", "reply"],
    ["c", "check out & open in editor"],
    ["⌘K", "ask or act on anything"],
    ["?", "all shortcuts"],
  ];
  return (
    <aside className="detail empty-detail">
      <div className="empty-art">
        <Icon name="layers" size={28} />
      </div>
      <p className="strong">Select something to see it here</p>
      <p className="muted small">Details and every action live in this pane.</p>
      <dl className="tips">
        {tips.map(([k, v]) => (
          <div key={k}>
            <dt>
              <kbd>{k}</kbd>
            </dt>
            <dd>{v}</dd>
          </div>
        ))}
      </dl>
    </aside>
  );
}

function Action({
  icon,
  label,
  hint,
  onClick,
  primary,
  danger,
}: {
  icon: IconName;
  label: string;
  hint?: string;
  onClick: () => void;
  primary?: boolean;
  danger?: boolean;
}) {
  return (
    <button className={`action${primary ? " primary" : ""}${danger ? " danger-text" : ""}`} onClick={onClick}>
      <Icon name={icon} size={15} />
      <span className="grow">{label}</span>
      {hint && <kbd>{hint}</kbd>}
    </button>
  );
}

function stateNote(item: Item): string | null {
  const p = item.personal;
  if (item.reminderDue) return p?.note ? `Reminder due — ${p.note}` : "Reminder due";
  if (!p || p.state === "due") return null;
  if (p.state === "later") {
    const when = p.until
      ? `until ${untilLabel(p.until)}`
      : p.waitsOn
        ? `until ${p.waitsOn} merges or closes`
        : "until something changes";
    return [`Parked ${when}`, p.note].filter(Boolean).join(" — ");
  }
  return `You marked this “${STATES[p.state as StateId]?.label ?? p.state}”`;
}

function ItemDetail({ item, now }: { item: Item; now: number }) {
  const [timeline, setTimeline] = useState<Activity[] | null>(null);
  const [snoozing, setSnoozing] = useState(false);

  useEffect(() => {
    api.getItemActivity(item.id).then(setTimeline).catch(() => setTimeline([]));
  }, [item.id, item.updatedAt, item.personal?.setAt]);
  useEffect(() => on("focus-snooze", () => setSnoozing(true)), []);

  const isPr = item.kind === "pr";
  const isRun = item.kind === "run";
  const note = stateNote(item);
  const kindIcon: IconName = isRun ? "branch" : item.kind === "issue" ? "issue" : "pr";

  // The one or two things this item most likely needs, first.
  const primary: ReactElement[] = [];
  if (!isRun && item.state === "replied")
    primary.push(<Action key="reply" icon="reply" label={`Reply to ${item.context?.who ?? "them"}`} hint="R" primary onClick={() => openDialog("reply", item)} />);
  if (isPr && !item.isMine && (item.state === "needs_review" || item.state === "needs_rereview"))
    primary.push(<Action key="approve" icon="checkCircle" label="Approve…" hint="a" primary onClick={() => openDialog("approve", item)} />);
  if (isPr && item.state === "ready_to_merge")
    primary.push(<Action key="merge" icon="merge" label="Merge…" hint="m" primary onClick={() => openDialog("merge", item)} />);
  if (isPr && item.isMine && item.ci === "failure")
    primary.push(<Action key="rerun" icon="rerun" label="Re-run failed checks" primary onClick={() => rerunChecks(item)} />);
  if (isRun) primary.push(<Action key="run" icon="external" label="Open the failing run" primary onClick={() => openItem(item)} />);

  return (
    <div className="detail-body">
      <div className="detail-meta">
        <Icon name={kindIcon} size={14} />
        <span>{shortRef(item)}</span>
        <span className="sep">·</span>
        {item.authorAvatar ? <img className="avatar xs" src={item.authorAvatar} alt="" /> : null}
        <span>{item.isMine ? "you" : item.author}</span>
        <span className="sep">·</span>
        <span title={new Date(item.updatedAt).toLocaleString()}>{age(item.updatedAt, now)} ago</span>
      </div>
      <h2 className="detail-title">{item.title}</h2>

      <div className="detail-state">
        <StateChip state={item.state} />
        <span className="muted">{item.reason}</span>
      </div>
      {note && (
        <div className="callout">
          <Icon name="pin" size={14} />
          <span>{note}</span>
        </div>
      )}

      {(isPr || item.branch) && (
        <div className="facts">
          {isPr && (
            <div className={`fact ci-${item.ci}`}>
              <Icon name={item.ci === "failure" ? "xCircle" : item.ci === "pending" ? "loader" : item.ci === "success" ? "checkCircle" : "minusCircle"} size={14} />
              {item.ci === "failure" ? "Checks failing" : item.ci === "pending" ? "Checks running" : item.ci === "success" ? "Checks passing" : "No checks"}
            </div>
          )}
          {item.branch && (
            <div className="fact mono" title="Branch">
              <Icon name="branch" size={14} />
              {item.branch}
            </div>
          )}
          {item.isDraft && <div className="fact">Draft</div>}
        </div>
      )}

      {item.context?.body && (
        <blockquote className="quote">
          <div className="quote-who">{item.context.who}</div>
          {item.context.body}
        </blockquote>
      )}

      {primary.length > 0 && <div className="action-group">{primary}</div>}

      <div className="action-group">
        <div className="group-label">On GitHub</div>
        <Action icon="external" label="Open on GitHub" hint="↵" onClick={() => openItem(item)} />
        {!isRun && <Action icon="message" label={item.state === "replied" ? "Reply…" : "Comment…"} hint="R" onClick={() => openDialog("reply", item)} />}
        {isPr && !item.isMine && item.state !== "needs_review" && item.state !== "needs_rereview" && (
          <Action icon="checkCircle" label="Approve…" hint="a" onClick={() => openDialog("approve", item)} />
        )}
        {isPr && !item.isMine && <Action icon="minusCircle" label="Request changes…" onClick={() => openDialog("request_changes", item)} />}
        {isPr && item.state !== "ready_to_merge" && <Action icon="merge" label="Merge…" hint="m" onClick={() => openDialog("merge", item)} />}
        {isPr && item.ci === "failure" && !item.isMine && <Action icon="rerun" label="Re-run failed checks" onClick={() => rerunChecks(item)} />}
      </div>

      {!isRun && (
        <div className="action-group">
          <div className="group-label">On your machine</div>
          {isPr && <Action icon="branch" label="Check out & open in editor" hint="c" onClick={() => openInEditor(item, true)} />}
          <Action icon="code" label="Open repo in editor" onClick={() => openInEditor(item, false)} />
          {isPr && <Action icon="copy" label="Copy checkout command" onClick={() => copyCheckout(item)} />}
        </div>
      )}

      <div className="action-group">
        <div className="group-label">Organise</div>
        {item.state !== "done" && <Action icon="check" label="Done" hint="e" onClick={() => setStates([item], "done")} />}
        <Action icon="pin" label="Come back later…" hint="l" onClick={() => setSnoozing((s) => !s)} />
        {snoozing && <SnoozePanel items={[item]} onDone={() => setSnoozing(false)} />}
        <MarkAs items={[item]} />
        {item.personal && (
          <Action icon="undo" label={`Back to automatic (${STATES[item.autoState].label.toLowerCase()})`} hint="x" onClick={() => setStates([item], null)} />
        )}
        <Action icon="bellOff" label="Stop following" onClick={() => unfollow([item])} />
      </div>

      <div className="timeline">
        <div className="group-label">History</div>
        {timeline === null && <div className="muted small">Loading…</div>}
        {timeline?.length === 0 && <div className="muted small">Nothing recorded yet.</div>}
        {timeline?.map((a, i) => (
          <div key={i} className="tl-row">
            <span className="tl-dot" />
            <span className="tl-text">{a.detail}</span>
            <span className="tl-time" title={new Date(a.at).toLocaleString()}>
              {friendlyDate(a.at)} {clock(a.at)}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}

function GroupDetail({ repo, items }: { repo: string; items: Item[] }) {
  const [snoozing, setSnoozing] = useState(false);
  const by = new Map<StateId, number>();
  for (const i of items) by.set(i.state, (by.get(i.state) ?? 0) + 1);
  return (
    <div className="detail-body">
      <div className="detail-meta">
        <Icon name="layers" size={14} />
        <span>{items.length} items</span>
      </div>
      <h2 className="detail-title">{repo}</h2>
      <div className="detail-state wrap">
        {[...by.entries()].map(([s, n]) => (
          <StateChip key={s} state={s} label={`${n} ${STATES[s].label.toLowerCase()}`} />
        ))}
      </div>
      <div className="action-group">
        <div className="group-label">All {items.length}</div>
        <Action icon="check" label="Mark all done" hint="e" onClick={() => setStates(items, "done")} />
        <Action icon="pin" label="Come back to all later…" hint="l" onClick={() => setSnoozing((s) => !s)} />
        {snoozing && <SnoozePanel items={items} onDone={() => setSnoozing(false)} />}
        <MarkAs items={items} />
        <Action icon="bellOff" label={`Stop following all ${items.length}`} onClick={() => unfollow(items)} />
        <Action icon="external" label="Open repo on GitHub" onClick={() => openUrl(`https://github.com/${repo}/pulls`)} />
      </div>
      <div className="timeline">
        <div className="group-label">In this group</div>
        {items.map((i) => (
          <div key={i.id} className="tl-row link-row" onClick={() => openItem(i)} title="Open on GitHub">
            <span className="tl-dot" />
            <span className="tl-text">{i.title}</span>
            <span className="tl-time">{shortRef(i)}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

function MarkAs({ items }: { items: Item[] }) {
  return (
    <label className="action select-action">
      <Icon name="pencil" size={15} />
      <span className="grow">Mark as…</span>
      <select
        value=""
        onChange={(e) => {
          if (e.target.value) setStates(items, e.target.value as StateId);
        }}
      >
        <option value="">Choose</option>
        {MANUAL_STATES.map((s) => (
          <option key={s} value={s}>
            {STATES[s].label}
          </option>
        ))}
      </select>
    </label>
  );
}

/** Note + "wait for this PR" suggestions + presets + a specific time. */
export function SnoozePanel({ items, onDone }: { items: Item[]; onDone: () => void }) {
  const single = items.length === 1 ? items[0] : null;
  const [note, setNote] = useState(single?.personal?.note ?? "");
  const [custom, setCustom] = useState("");
  const input = useRef<HTMLInputElement>(null);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    input.current?.focus({ preventScroll: true });
    box.current?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }, []);
  const waitFor = single && note.trim().length >= 3 ? matchTitles(note, single.id) : [];
  const park = async (until: string | null, noteOverride?: string) => {
    onDone();
    await setStates(items, "later", { until, note: noteOverride ?? note });
  };
  return (
    <div className="snooze" ref={box} onKeyDown={(e) => e.key === "Escape" && onDone()}>
      <input
        ref={input}
        placeholder="Why? e.g. after the API migration, #392…"
        value={note}
        onChange={(e) => setNote(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && park(SNOOZE_PRESETS[1].until())}
      />
      {waitFor.map((w) => (
        <button key={w.id} className="snooze-opt" onClick={() => park(null, `${note.trim()} (${w.repo}#${w.number})`)}>
          <Icon name="merge" size={14} />
          <span className="grow ellipsis">
            Until {shortRef(w)} merges <span className="muted">· {w.title}</span>
          </span>
        </button>
      ))}
      <div className="snooze-presets">
        {SNOOZE_PRESETS.map((p) => (
          <button key={p.id} className="chip-btn" onClick={() => park(p.until())}>
            {p.label}
          </button>
        ))}
      </div>
      <div className="snooze-custom">
        <input type="datetime-local" value={custom} onChange={(e) => setCustom(e.target.value)} />
        <button disabled={!custom} onClick={() => park(new Date(custom).toISOString())}>
          Set
        </button>
      </div>
    </div>
  );
}
