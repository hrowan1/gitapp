import { useEffect, useMemo, useRef, useState } from "react";
import type { Item } from "../lib/api";
import { copyCheckout, openInEditor, openItem, rerunChecks, setStates, unfollow } from "../lib/actions";
import { openDialog } from "../lib/dialog";
import { parseAction, parseQuery, SUGGESTIONS, type PaletteAction } from "../lib/query";
import { compareItems, groupItems, shortRef } from "../lib/states";
import { Icon } from "./Icon";
import { StateIcon } from "./StateIcon";
import { SNOOZE_PRESETS, age } from "../lib/time";

export type View = "today" | "all" | "day" | "tickets" | "history" | "following" | "settings";

interface Command {
  id: string;
  label: string;
  hint?: string;
  run: () => void;
}

type Entry =
  | { kind: "command"; command: Command }
  | { kind: "pr"; item: Item }
  | { kind: "suggestion"; text: string }
  | { kind: "action"; label: string; action: PaletteAction };

function runAction(action: PaletteAction, item: Item) {
  switch (action.kind) {
    case "state":
      return setStates([item], action.state, { until: action.until, note: action.note });
    case "reset":
      return setStates([item], null);
    case "open":
      return openItem(item);
    case "rerun":
      return rerunChecks(item);
    case "checkout":
      return copyCheckout(item);
    case "unfollow":
      return unfollow([item]);
    case "dialog":
      return openDialog(action.dialog, item);
    case "editor":
      return openInEditor(item, action.checkout && item.kind === "pr");
  }
}

/** Everything you can do to one PR, listed when you press Tab on it. */
function actionsFor(item: Item): { label: string; action: PaletteAction }[] {
  const list: { label: string; action: PaletteAction }[] = [
    { label: "↗ Open on GitHub", action: { kind: "open" } },
    { label: "✅ Mark done", action: { kind: "state", state: "done" } },
    ...SNOOZE_PRESETS.map((p) => ({
      label: `📌 Come back ${p.label.charAt(0).toLowerCase()}${p.label.slice(1)}`,
      action: { kind: "state", state: "later", until: p.until() } as PaletteAction,
    })),
    { label: "⏳ Mark as waiting for response", action: { kind: "state", state: "waiting" } },
    { label: "🔧 Mark as I need to make changes", action: { kind: "state", state: "needs_changes" } },
    { label: "🔕 Stop following", action: { kind: "unfollow" } },
  ];
  if (item.kind !== "run") {
    list.splice(1, 0, {
      label: item.context && item.state === "replied" ? `↩︎ Reply to ${item.context.who}…` : "↩︎ Comment…",
      action: { kind: "dialog", dialog: "reply" },
    });
    list.push({ label: "⌨︎ Open repo in editor", action: { kind: "editor", checkout: false } });
  }
  if (item.kind === "pr") {
    if (!item.isMine) {
      list.splice(2, 0, { label: "✅ Approve…", action: { kind: "dialog", dialog: "approve" } });
      list.splice(3, 0, { label: "✋ Request changes…", action: { kind: "dialog", dialog: "request_changes" } });
    }
    list.push({ label: "🔀 Merge…", action: { kind: "dialog", dialog: "merge" } });
    list.push({ label: "⎇ Check out & open in editor", action: { kind: "editor", checkout: true } });
    list.push({ label: "⧉ Copy checkout command", action: { kind: "checkout" } });
  }
  if (item.kind === "pr" && item.ci === "failure") list.splice(2, 0, { label: "↻ Re-run failed checks", action: { kind: "rerun" } });
  if (item.personal) list.push({ label: "↺ Back to automatic", action: { kind: "reset" } });
  return list;
}

export function CommandPalette({
  items,
  onClose,
  onNavigate,
  onSync,
}: {
  items: Item[];
  onClose: () => void;
  onNavigate: (v: View) => void;
  onSync: () => void;
}) {
  const [q, setQ] = useState("");
  const [sel, setSel] = useState(0);
  /** When set, the palette lists actions for this PR instead of searching. */
  const [focus, setFocus] = useState<Item | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  const commands: Command[] = [
    { id: "today", label: "Go to Today", hint: "1", run: () => onNavigate("today") },
    { id: "all", label: "Go to All pull requests", hint: "2", run: () => onNavigate("all") },
    { id: "day", label: "Open Day view: what I worked on, and for how long", hint: "3", run: () => onNavigate("day") },
    { id: "tickets", label: "Go to Tickets: my Jira tickets and how they moved", hint: "4", run: () => onNavigate("tickets") },
    { id: "history", label: "Go to History", hint: "5", run: () => onNavigate("history") },
    { id: "following", label: "Choose what to follow", hint: "6", run: () => onNavigate("following") },
    { id: "settings", label: "Go to Settings", hint: "7", run: () => onNavigate("settings") },
    { id: "sync", label: "Sync with GitHub now", hint: "r", run: onSync },
  ];

  const action = useMemo(() => (focus ? null : parseAction(q)), [q, focus]);
  const parsed = useMemo(() => parseQuery(action ? action.target : q, items), [q, action, items]);

  const entries: Entry[] = useMemo(() => {
    if (focus) {
      return actionsFor(focus)
        .filter((a) => !q || a.label.toLowerCase().includes(q.toLowerCase()))
        .map((a) => ({ kind: "action" as const, ...a }));
    }
    if (!q.trim()) {
      const needs = groupItems(items).needs.slice(0, 6);
      return [
        ...SUGGESTIONS.map((text) => ({ kind: "suggestion" as const, text })),
        ...needs.map((item) => ({ kind: "pr" as const, item })),
      ];
    }
    if (action) {
      const pool = action.target ? items.filter(parsed.filter) : groupItems(items).needs;
      return pool.sort(compareItems).slice(0, 50).map((item) => ({ kind: "pr" as const, item }));
    }
    const lower = q.toLowerCase();
    const cmds = commands
      .filter((c) => c.label.toLowerCase().includes(lower))
      .map((command) => ({ kind: "command" as const, command }));
    if (parsed.view === "history") {
      return [
        { kind: "command", command: { id: "day", label: "Open Day view", hint: "3", run: () => onNavigate("day") } },
        { kind: "command", command: { id: "h", label: "Show your history", run: () => onNavigate("history") } },
      ];
    }
    const prs = items.filter(parsed.filter).sort(compareItems).slice(0, 50);
    return [...cmds, ...prs.map((item) => ({ kind: "pr" as const, item }))];
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [q, items, parsed, action, focus]);

  useEffect(() => setSel(0), [q, focus]);
  useEffect(() => {
    listRef.current?.querySelector(".pal-item.selected")?.scrollIntoView({ block: "nearest" });
  }, [sel]);

  const enterActions = (item: Item) => {
    setFocus(item);
    setQ("");
    inputRef.current?.focus();
  };

  const activate = (e: Entry, alt = false) => {
    if (e.kind === "suggestion") {
      setQ(e.text);
      return;
    }
    if (e.kind === "command") e.command.run();
    else if (e.kind === "action" && focus) runAction(e.action, focus);
    else if (e.kind === "pr") {
      if (action) runAction(action.action, e.item);
      else if (alt) setStates([e.item], "done");
      else openItem(e.item);
    }
    onClose();
  };

  const prCount = entries.filter((e) => e.kind === "pr").length;
  let summary: string | null = null;
  if (focus) summary = `${shortRef(focus)} · ${focus.title}`;
  else if (action) summary = `${action.label} → ${prCount ? "pick a PR" : "no matching PR"}`;
  else if (q.trim() && parsed.understood && !parsed.view) summary = `${prCount} ${prCount === 1 ? "PR" : "PRs"} ${parsed.label}`;

  return (
    <div className="overlay" onMouseDown={onClose}>
      <div className="palette" onMouseDown={(e) => e.stopPropagation()}>
        <div className="pal-input-row">
          {focus && (
            <span className="pal-chip" title="Backspace to go back">
              {focus.kind === "run" ? "branch" : `#${focus.number}`}
            </span>
          )}
          <input
            ref={inputRef}
            autoFocus
            className="pal-input"
            placeholder={focus ? "Do what?" : "Ask, or act: “remind me about checkout tomorrow”"}
            value={q}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => {
              const cur = entries[sel];
              if (e.key === "Escape") {
                e.preventDefault();
                if (q) setQ("");
                else if (focus) setFocus(null);
                else onClose();
              } else if (e.key === "Backspace" && !q && focus) {
                e.preventDefault();
                setFocus(null);
              } else if (e.key === "ArrowDown") {
                e.preventDefault();
                setSel((s) => Math.min(entries.length - 1, s + 1));
              } else if (e.key === "ArrowUp") {
                e.preventDefault();
                setSel((s) => Math.max(0, s - 1));
              } else if ((e.key === "Tab" || e.key === "ArrowRight") && cur?.kind === "pr" && !action) {
                if (e.key === "ArrowRight" && (e.currentTarget.selectionStart ?? 0) < q.length) return;
                e.preventDefault();
                enterActions(cur.item);
              } else if (e.key === "Enter" && cur) {
                e.preventDefault();
                activate(cur, e.metaKey || e.ctrlKey);
              }
            }}
          />
        </div>
        {summary && <div className="pal-summary">{summary}</div>}
        <div className="pal-list" ref={listRef}>
          {entries.length === 0 && <div className="empty">Nothing matches “{q}”.</div>}
          {entries.map((e, i) => (
            <div
              key={
                e.kind === "pr" ? e.item.id : e.kind === "command" ? e.command.id : e.kind === "action" ? e.label : e.text
              }
              className={`pal-item${i === sel ? " selected" : ""}${e.kind === "suggestion" ? " suggestion" : ""}`}
              onMouseMove={() => setSel(i)}
              onClick={() => activate(e)}
            >
              {e.kind === "suggestion" && (
                <>
                  <span className="pal-icon">
                    <Icon name="search" size={15} />
                  </span>
                  <span className="grow">{e.text}</span>
                </>
              )}
              {e.kind === "command" && (
                <>
                  <span className="pal-icon">
                    <Icon name="chevronRight" size={15} />
                  </span>
                  <span className="grow">{e.command.label}</span>
                  {e.command.hint && <kbd>{e.command.hint}</kbd>}
                </>
              )}
              {e.kind === "action" && <span className="grow">{e.label}</span>}
              {e.kind === "pr" && (
                <>
                  <span className="pal-icon">
                    <StateIcon state={e.item.state} size={14} />
                  </span>
                  <span className="grow ellipsis">
                    {e.item.isNew && <span className="new-dot" />}
                    <span className="strong">{e.item.title}</span>
                    <span className="muted">
                      {" "}
                      · {shortRef(e.item)} · {e.item.isMine ? "you" : e.item.author} ·{" "}
                      {e.item.reason}
                    </span>
                  </span>
                  <span className="muted">{age(e.item.updatedAt)}</span>
                </>
              )}
            </div>
          ))}
        </div>
        <div className="pal-foot muted">
          {focus ? (
            <>
              <span>
                <kbd>↵</kbd> run
              </span>
              <span>
                <kbd>⌫</kbd> back
              </span>
            </>
          ) : action ? (
            <span>
              <kbd>↵</kbd> apply to selected PR
            </span>
          ) : (
            <>
              <span>
                <kbd>↵</kbd> open
              </span>
              <span>
                <kbd>tab</kbd> actions
              </span>
              <span>
                <kbd>⌘↵</kbd> done
              </span>
            </>
          )}
          <span>
            <kbd>esc</kbd> close
          </span>
        </div>
      </div>
    </div>
  );
}
