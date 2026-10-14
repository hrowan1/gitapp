import { useEffect, useRef, useState } from "react";
import type { Item, StateId } from "../lib/api";
import { copyCheckout, openInEditor, openItem, rerunChecks, setStates, unfollow } from "../lib/actions";
import { openDialog } from "../lib/dialog";
import { MANUAL_STATES, STATES, shortRef } from "../lib/states";
import { matchTitles } from "../lib/store";
import { SNOOZE_PRESETS, age, untilLabel } from "../lib/time";

const CI_LABEL: Record<Item["ci"], string | null> = {
  failure: "Checks failing",
  pending: "Checks running",
  success: null,
  none: null,
};

export function PrRow({
  item,
  now,
  selected,
  onSelect,
  menuOpen,
  onMenu,
  nested,
}: {
  item: Item;
  now: number;
  selected: boolean;
  onSelect: () => void;
  menuOpen: boolean;
  onMenu: (open: boolean) => void;
  nested?: boolean;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const meta = STATES[item.state];
  const personal = item.personal && item.personal.state !== "due" ? item.personal : null;

  useEffect(() => {
    if (selected) ref.current?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  let reason = item.reason;
  if (item.reminderDue) reason = item.personal?.note ? `⏰ ${item.personal.note}` : "⏰ Reminder due";
  else if (personal?.state === "later") {
    const when = personal.until
      ? `until ${untilLabel(personal.until)}`
      : personal.waitsOn
        ? `until ${personal.waitsOn.split("/")[1] ?? personal.waitsOn} merges or closes`
        : "until something changes";
    reason = [personal.note, when].filter(Boolean).join(" · ");
  } else if (personal) reason = `Marked ${STATES[personal.state as StateId]?.label.toLowerCase() ?? personal.state} · ${item.reason}`;

  const ci = CI_LABEL[item.ci];

  return (
    <div
      ref={ref}
      className={`row${selected ? " selected" : ""}${nested ? " nested" : ""}`}
      onMouseEnter={onSelect}
      onClick={() => openItem(item)}
      role="button"
    >
      <span className="row-state" title={meta.label}>
        {meta.emoji}
      </span>
      {item.authorAvatar ? (
        <img className="avatar" src={item.authorAvatar} alt="" />
      ) : (
        <span className="avatar placeholder">{item.author.slice(0, 1)}</span>
      )}
      <div className="row-main">
        <div className="row-title">
          {item.isNew && <span className="new-dot" title="New since you last looked" />}
          {item.kind !== "pr" && <span className={`kind kind-${item.kind}`}>{item.kind === "issue" ? "Issue" : "Branch"}</span>}
          <span className="who">{item.isMine ? "You" : item.author}</span>
          <span className="sep">·</span>
          <span className="title">{item.title}</span>
        </div>
        <div className="row-sub">
          <span className="repo">{shortRef(item)}</span>
          <span className="sep">·</span>
          <span className="reason">{reason}</span>
          {ci && item.kind === "pr" && !reason.toLowerCase().includes("ci") && !reason.includes("checks") && (
            <span className={`pill ci-${item.ci}`}>{ci}</span>
          )}
          {item.isDraft && !item.isMine && <span className="pill">Draft</span>}
        </div>
        {item.context?.body && item.state === "replied" && (
          <div className="row-quote" title={item.context.body}>
            “{item.context.body}”
          </div>
        )}
      </div>
      <div className="row-right">
        <span className="age" title={new Date(item.since).toLocaleString()}>
          {age(item.since, now)}
        </span>
        <div className="row-actions" onClick={(e) => e.stopPropagation()}>
          {item.kind !== "run" && item.state === "replied" && (
            <button className="icon-btn" title="Reply (R)" onClick={() => openDialog("reply", item)}>
              ↩︎
            </button>
          )}
          {item.state !== "done" && (
            <button className="icon-btn" title="Done (e)" onClick={() => setStates([item], "done")}>
              ✓
            </button>
          )}
          <button className="icon-btn" title="More (.)" onClick={() => onMenu(!menuOpen)}>
            ⋯
          </button>
        </div>
      </div>
      {menuOpen && <ActionMenu items={[item]} onClose={() => onMenu(false)} />}
    </div>
  );
}

/** Snooze / mark / mute menu for one PR or a whole group. */
export function ActionMenu({ items, onClose }: { items: Item[]; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const single = items.length === 1 ? items[0] : null;
  const [note, setNote] = useState(single?.personal?.note ?? "");

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey, true);
    };
  }, [onClose]);

  const act = async (state: StateId | null, until?: string | null, noteOverride?: string) => {
    onClose();
    await setStates(items, state, { until, note: state === "later" ? (noteOverride ?? note) : null });
  };
  // "after the API migration" → offer to wait for the PR that matches.
  const waitFor = single && note.trim().length >= 3 ? matchTitles(note, single.id) : [];
  const run = (fn: () => void) => {
    onClose();
    fn();
  };

  return (
    <div className="menu" ref={ref} onClick={(e) => e.stopPropagation()}>
      <div className="menu-label">Come back later</div>
      <input
        className="menu-input"
        placeholder="Why? e.g. after the API migration"
        value={note}
        autoFocus
        onChange={(e) => setNote(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") act("later", SNOOZE_PRESETS[1].until());
        }}
      />
      {waitFor.map((w) => (
        <button
          key={w.id}
          className="menu-item"
          title="Comes back when this merges or closes"
          onClick={() => act("later", null, `${note.trim()} (${w.repo}#${w.number})`)}
        >
          ⏳ Until {shortRef(w)} merges: <span className="muted">{w.title}</span>
        </button>
      ))}
      {SNOOZE_PRESETS.map((p) => (
        <button key={p.id} className="menu-item" onClick={() => act("later", p.until())}>
          📌 {p.label}
        </button>
      ))}
      <div className="menu-label">Mark as</div>
      {MANUAL_STATES.map((s) => (
        <button
          key={s}
          className={`menu-item${single?.personal?.state === s ? " active" : ""}`}
          onClick={() => act(s)}
        >
          {STATES[s].emoji} {STATES[s].label}
        </button>
      ))}
      <div className="menu-sep" />
      <button
        className="menu-item"
        onClick={() => run(() => unfollow(items))}
        title="Gone from Today, counts and notifications. Re-follow from the Following tab."
      >
        🔕 Stop following{items.length > 1 ? ` all ${items.length}` : ""}
      </button>
      {single && single.kind !== "run" && (
        <>
          <div className="menu-label">Act</div>
          <button className="menu-item" onClick={() => run(() => openDialog("reply", single))}>
            ↩︎ {single.context && single.state === "replied" ? `Reply to ${single.context.who}…` : "Comment…"}
          </button>
          {single.kind === "pr" && !single.isMine && (
            <>
              <button className="menu-item" onClick={() => run(() => openDialog("approve", single))}>
                ✅ Approve…
              </button>
              <button className="menu-item" onClick={() => run(() => openDialog("request_changes", single))}>
                ✋ Request changes…
              </button>
            </>
          )}
          {single.kind === "pr" && (
            <button className="menu-item" onClick={() => run(() => openDialog("merge", single))}>
              🔀 Merge…
            </button>
          )}
          {single.kind === "pr" && (
            <button className="menu-item" onClick={() => run(() => openInEditor(single, true))}>
              ⎇ Check out & open in editor
            </button>
          )}
          <button className="menu-item" onClick={() => run(() => openInEditor(single, false))}>
            ⌨︎ Open repo in editor
          </button>
        </>
      )}
      {single?.kind === "pr" && (
        <>
          {single.ci === "failure" && (
            <button className="menu-item" onClick={() => run(() => rerunChecks(single))}>
              ↻ Re-run failed checks
            </button>
          )}
          <button className="menu-item" onClick={() => run(() => copyCheckout(single))}>
            ⧉ Copy checkout command
          </button>
        </>
      )}
      {items.some((i) => i.personal) && (
        <>
          <div className="menu-sep" />
          <button className="menu-item" onClick={() => act(null)}>
            ↺ Back to automatic
            {single ? ` (${STATES[single.autoState].label.toLowerCase()})` : ""}
          </button>
        </>
      )}
    </div>
  );
}
