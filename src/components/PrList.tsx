import { useEffect, useMemo, useState } from "react";
import type { Item } from "../lib/api";
import { openInEditor, openItem, setStates } from "../lib/actions";
import { openDialog } from "../lib/dialog";
import { STATES } from "../lib/states";
import { ActionMenu, PrRow } from "./PrRow";

/** A repo with at least this many PRs in one section is collapsed into a single group row. */
const GROUP_MIN = 4;

export interface ListSection {
  id: string;
  title: string;
  items: Item[];
  empty?: string;
  collapsedByDefault?: boolean;
}

type Block = { kind: "item"; item: Item } | { kind: "group"; key: string; repo: string; items: Item[] };

/** Keep sort order, but fold repos with many PRs into one group at the position of their first PR. */
function toBlocks(sectionId: string, items: Item[]): Block[] {
  const counts = new Map<string, number>();
  for (const i of items) counts.set(i.repo, (counts.get(i.repo) ?? 0) + 1);
  const groups = new Map<string, Block & { kind: "group" }>();
  const out: Block[] = [];
  for (const item of items) {
    if ((counts.get(item.repo) ?? 0) < GROUP_MIN) {
      out.push({ kind: "item", item });
      continue;
    }
    let g = groups.get(item.repo);
    if (!g) {
      g = { kind: "group", key: `group:${sectionId}:${item.repo}`, repo: item.repo, items: [] };
      groups.set(item.repo, g);
      out.push(g);
    }
    g.items.push(item);
  }
  return out;
}

function summarise(items: Item[]): string {
  const by = new Map<string, number>();
  for (const i of items) by.set(i.state, (by.get(i.state) ?? 0) + 1);
  return [...by.entries()]
    .sort((a, b) => b[1] - a[1])
    .map(([s, n]) => `${n} ${STATES[s as Item["state"]]?.label.toLowerCase() ?? s}`)
    .join(" · ");
}

type Entry = { id: string; item?: Item; group?: Block & { kind: "group" } };

/** Sectioned PR list with keyboard navigation (j/k, ↵ open/expand, e done, l later, x reset). */
export function PrList({ sections, now, active }: { sections: ListSection[]; now: number; active: boolean }) {
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [menuFor, setMenuFor] = useState<string | null>(null);
  const [focusNote, setFocusNote] = useState(false);
  const openMenu = (id: string | null, note = false) => {
    setFocusNote(note);
    setMenuFor(id);
  };

  const isCollapsed = (s: ListSection) => collapsed[s.id] ?? !!s.collapsedByDefault;
  const blocks = useMemo(
    () => Object.fromEntries(sections.map((s) => [s.id, toBlocks(s.id, s.items)])),
    [sections],
  );

  // Everything keyboard navigation can land on, in display order.
  const entries: Entry[] = useMemo(
    () =>
      sections.flatMap((s) =>
        isCollapsed(s)
          ? []
          : blocks[s.id].flatMap((b): Entry[] =>
              b.kind === "item"
                ? [{ id: b.item.id, item: b.item }]
                : [
                    { id: b.key, group: b },
                    ...(expanded[b.key] ? b.items.map((item) => ({ id: item.id, item })) : []),
                  ],
            ),
      ),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [sections, blocks, collapsed, expanded],
  );

  useEffect(() => {
    if (!active) return;
    const onKey = (e: KeyboardEvent) => {
      const tag = (document.activeElement?.tagName ?? "").toLowerCase();
      if (tag === "input" || tag === "textarea" || e.metaKey || e.ctrlKey || e.altKey || menuFor) return;
      const idx = entries.findIndex((x) => x.id === selectedId);
      const cur = idx >= 0 ? entries[idx] : null;
      const targets = cur?.group ? cur.group.items : cur?.item ? [cur.item] : [];
      const move = (d: number) => {
        if (!entries.length) return;
        const next = idx < 0 ? 0 : Math.min(entries.length - 1, Math.max(0, idx + d));
        setSelectedId(entries[next].id);
      };
      switch (e.key) {
        case "j":
        case "ArrowDown":
          move(1);
          break;
        case "k":
        case "ArrowUp":
          move(-1);
          break;
        case "Enter":
        case "o":
          if (cur?.group) setExpanded({ ...expanded, [cur.id]: !expanded[cur.id] });
          else if (cur?.item) openItem(cur.item);
          break;
        case "e":
          if (targets.length) {
            move(1);
            setStates(targets, "done");
          }
          break;
        case "l":
        case ".":
          if (cur) openMenu(cur.id, e.key === "l");
          break;
        case "x":
          if (targets.some((t) => t.personal)) setStates(targets, null);
          break;
        case "R":
          if (cur?.item && cur.item.kind !== "run") openDialog("reply", cur.item);
          break;
        case "a":
          if (cur?.item?.kind === "pr" && !cur.item.isMine) openDialog("approve", cur.item);
          break;
        case "m":
          if (cur?.item?.kind === "pr") openDialog("merge", cur.item);
          break;
        case "c":
          if (cur?.item && cur.item.kind !== "run") openInEditor(cur.item, cur.item.kind === "pr");
          break;
        default:
          return;
      }
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [active, entries, selectedId, menuFor, expanded]);

  const row = (item: Item, nested = false) => (
    <PrRow
      key={item.id}
      item={item}
      now={now}
      nested={nested}
      selected={item.id === selectedId}
      onSelect={() => setSelectedId(item.id)}
      menuOpen={menuFor === item.id}
      onMenu={(open) => openMenu(open ? item.id : null)}
      focusNote={focusNote}
    />
  );

  return (
    <div className="list">
      {sections.map((s) => {
        if (!s.items.length && !s.empty) return null;
        const closed = isCollapsed(s);
        return (
          <section key={s.id} className="section">
            <button className="section-head" onClick={() => setCollapsed({ ...collapsed, [s.id]: !closed })}>
              <span className={`chevron${closed ? " closed" : ""}`}>▾</span>
              <span className="section-title">{s.title}</span>
              <span className="count">{s.items.length}</span>
            </button>
            {!closed && !s.items.length && <div className="empty">{s.empty}</div>}
            {!closed &&
              blocks[s.id].map((b) => {
                if (b.kind === "item") return row(b.item);
                const open = !!expanded[b.key];
                const newCount = b.items.filter((i) => i.isNew).length;
                return (
                  <div key={b.key} className="group">
                    <div
                      className={`row group-row${selectedId === b.key ? " selected" : ""}`}
                      role="button"
                      onMouseEnter={() => setSelectedId(b.key)}
                      onClick={() =>
                        menuFor === b.key ? setMenuFor(null) : setExpanded({ ...expanded, [b.key]: !open })
                      }
                      onContextMenu={(e) => {
                        e.preventDefault();
                        openMenu(b.key);
                      }}
                    >
                      <span className={`chevron${open ? "" : " closed"}`}>▾</span>
                      <span className="group-count">{b.items.length}</span>
                      <div className="row-main">
                        <div className="row-title">
                          {newCount > 0 && <span className="new-dot" title={`${newCount} new`} />}
                          <span className="who">{b.repo}</span>
                        </div>
                        <div className="row-sub">
                          <span className="reason">{summarise(b.items)}</span>
                        </div>
                      </div>
                      <div className="row-right">
                        <div className="row-actions" onClick={(e) => e.stopPropagation()}>
                          <button
                            className="icon-btn"
                            title="Mark all done (e)"
                            onClick={() => setStates(b.items, "done")}
                          >
                            ✓ all
                          </button>
                          <button
                            className="icon-btn"
                            title="Snooze, mark or mute all (.)"
                            onClick={() => openMenu(menuFor === b.key ? null : b.key)}
                          >
                            ⋯
                          </button>
                        </div>
                      </div>
                      {menuFor === b.key && <ActionMenu items={b.items} onClose={() => setMenuFor(null)} />}
                    </div>
                    {open && b.items.map((item) => row(item, true))}
                  </div>
                );
              })}
          </section>
        );
      })}
    </div>
  );
}
