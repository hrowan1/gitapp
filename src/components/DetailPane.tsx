import { useEffect, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, type Activity, type CiExplanation, type Item, type StateId } from "../lib/api";
import { copyCheckout, openInEditor, openItem, rerunChecks, setStates, unfollow } from "../lib/actions";
import { openDialog } from "../lib/dialog";
import { on } from "../lib/events";
import { MANUAL_STATES, STATES, shortRef } from "../lib/states";
import { matchTitles } from "../lib/store";
import { SNOOZE_PRESETS, clock, friendlyDate, untilLabel } from "../lib/time";
import { Icon, type IconName } from "./Icon";

export type Selection = { item: Item } | null;

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

/** "claude-haiku-5-5" → "Claude Haiku 5.5"; "apple" → Apple Intelligence. */
function engineName(by: string): string {
  if (by === "apple") return "Apple Intelligence";
  const m = by.match(/^claude-([a-z]+)-(\d+)-(\d+)$/);
  return m ? `Claude ${m[1].charAt(0).toUpperCase()}${m[1].slice(1)} ${m[2]}.${m[3]}` : by;
}

/** "Explain the failure": what broke, where, and the likely fix, from the failing checks' logs. */
function CiExplainer({ item }: { item: Item }) {
  const [state, setState] = useState<{ busy: boolean; result?: CiExplanation; error?: string }>({ busy: false });
  useEffect(() => setState({ busy: false }), [item.id]);
  const explain = async () => {
    setState({ busy: true });
    try {
      setState({ busy: false, result: await api.explainCi(item.id) });
    } catch (e) {
      setState({ busy: false, error: String(e) });
    }
  };
  if (!state.busy && !state.result && !state.error) {
    return <Action icon="sparkle" label="Explain the failure" onClick={explain} />;
  }
  return (
    <div className="ci-explain">
      {state.busy && (
        <div className="muted small ask-thinking">
          <Icon name="loader" size={13} className="spin" /> Reading the failed checks' logs…
        </div>
      )}
      {state.error && (
        <div className="error small">
          {state.error}{" "}
          <button className="link small" onClick={explain}>
            Try again
          </button>
        </div>
      )}
      {state.result && (
        <>
          <div className="ai-summary-text">{state.result.text}</div>
          <div className="muted small ci-explain-by">
            <Icon name="sparkle" size={12} /> {engineName(state.result.by)} ·{" "}
            {state.result.checks.map(([name, url], i) => (
              <span key={i}>
                {i > 0 && ", "}
                {url ? (
                  <button className="link small" onClick={() => openUrl(url)}>
                    {name}
                  </button>
                ) : (
                  name
                )}
              </span>
            ))}{" "}
            ·{" "}
            <button className="link small" onClick={explain}>
              Again
            </button>
          </div>
        </>
      )}
    </div>
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

/** Under the selected row in a list: the reply or note, the actions it needs, and the rest a click away. */
export function InlineDetail({ item }: { item: Item }) {
  const [snoozing, setSnoozing] = useState(false);
  const [timeline, setTimeline] = useState<Activity[] | null>(null);
  const [showHistory, setShowHistory] = useState(false);
  useEffect(() => on("focus-snooze", () => setSnoozing(true)), []);
  useEffect(() => {
    if (showHistory) api.getItemActivity(item.id).then(setTimeline).catch(() => setTimeline([]));
  }, [showHistory, item.id, item.updatedAt]);

  const isPr = item.kind === "pr";
  const isRun = item.kind === "run";
  const reviewing = isPr && !item.isMine && (item.state === "needs_review" || item.state === "needs_rereview");
  const note = stateNote(item);
  const stop = (e: React.MouseEvent) => e.stopPropagation();

  return (
    <div className="inline-detail" onClick={stop} onDoubleClick={stop}>
      {item.context?.body && (
        <blockquote className="quote">
          <div className="quote-who">{item.context.who}</div>
          {item.context.body}
        </blockquote>
      )}
      {note && (
        <div className="callout">
          <Icon name="pin" size={14} />
          <span>{note}</span>
        </div>
      )}

      <div className="inline-actions">
        {!isRun && item.state === "replied" && (
          <button className="primary" title="R" onClick={() => openDialog("reply", item)}>
            <Icon name="reply" size={14} /> Reply to {item.context?.who ?? "them"}
          </button>
        )}
        {reviewing && (
          <button className="primary" title="a" onClick={() => openDialog("approve", item)}>
            <Icon name="checkCircle" size={14} /> Approve…
          </button>
        )}
        {isPr && item.state === "ready_to_merge" && (
          <button className="primary" title="m" onClick={() => openDialog("merge", item)}>
            <Icon name="merge" size={14} /> Merge…
          </button>
        )}
        {isPr && item.ci === "failure" && (
          <button className={item.isMine ? "primary" : ""} onClick={() => rerunChecks(item)}>
            <Icon name="rerun" size={14} /> Re-run failed checks
          </button>
        )}
        <button title="↵" onClick={() => openItem(item)}>
          <Icon name="external" size={14} /> {isRun ? "Open the failing run" : "Open on GitHub"}
        </button>
        {!isRun && item.state !== "replied" && (
          <button title="R" onClick={() => openDialog("reply", item)}>
            <Icon name="message" size={14} /> Comment…
          </button>
        )}
        {item.state !== "done" && (
          <button title="e" onClick={() => setStates([item], "done")}>
            <Icon name="check" size={14} /> Done
          </button>
        )}
        <button title="l" onClick={() => setSnoozing((s) => !s)}>
          <Icon name="pin" size={14} /> Later…
        </button>
      </div>
      {snoozing && <SnoozePanel items={[item]} onDone={() => setSnoozing(false)} />}
      {isPr && item.ci === "failure" && <CiExplainer item={item} />}

      <div className="inline-foot">
        {item.branch && (
          <span className="mono" title="Branch">
            <Icon name="branch" size={12} /> {item.branch}
          </span>
        )}
        {reviewing && (
          <button className="link small" onClick={() => openDialog("request_changes", item)}>
            Request changes…
          </button>
        )}
        {isPr && (
          <button className="link small" title="c" onClick={() => openInEditor(item, true)}>
            Check out in editor
          </button>
        )}
        {isPr && (
          <button className="link small" onClick={() => copyCheckout(item)}>
            Copy checkout command
          </button>
        )}
        <label className="inline-mark">
          <select
            value=""
            onChange={(e) => {
              if (e.target.value) setStates([item], e.target.value as StateId);
            }}
          >
            <option value="">Mark as…</option>
            {MANUAL_STATES.map((s) => (
              <option key={s} value={s}>
                {STATES[s].label}
              </option>
            ))}
          </select>
        </label>
        {item.personal && (
          <button className="link small" title="x" onClick={() => setStates([item], null)}>
            Back to automatic
          </button>
        )}
        <button className="link small" onClick={() => unfollow([item])}>
          Stop following
        </button>
        <span className="grow" />
        <button className="link small" onClick={() => setShowHistory((s) => !s)}>
          {showHistory ? "Hide history" : "History"}
        </button>
      </div>
      {showHistory && (
        <div className="timeline">
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
      )}
    </div>
  );
}
