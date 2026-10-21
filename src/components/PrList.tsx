import { useEffect, useMemo, useState } from "react";
import type { Item } from "../lib/api";
import { openInEditor, openItem, setStates } from "../lib/actions";
import { openDialog } from "../lib/dialog";
import { emit } from "../lib/events";
import { InlineDetail, type Selection } from "./DetailPane";
import { Icon } from "./Icon";
import { PrRow } from "./PrRow";

/** Rows shown per section before "Show all". */
const PREVIEW = 8;

export interface ListSection {
  id: string;
  title: string;
  items: Item[];
  empty?: string;
  collapsedByDefault?: boolean;
}

/** Sections of rows, one item per row (no repo folders). Click a row to open its details right
 * under it; click again, or Esc, to close. Keys: j/k move, ↵ open on GitHub, e done, l later, x reset,
 * R reply, a approve, m merge, c check out. */
export function PrList({
  sections,
  now,
  active,
  selectedId,
  onSelect,
}: {
  sections: ListSection[];
  now: number;
  active: boolean;
  selectedId: string | null;
  onSelect: (id: string | null, selection: Selection) => void;
}) {
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [showAll, setShowAll] = useState<Record<string, boolean>>({});
  const [menuFor, setMenuFor] = useState<string | null>(null);
  const [focusNote, setFocusNote] = useState(false);
  const openMenu = (id: string | null, note = false) => {
    setFocusNote(note);
    setMenuFor(id);
  };

  const holdsSelection = (s: ListSection) => s.items.some((i) => i.id === selectedId);
  // A section with the selected item stays open and shows it, even past the preview.
  const isCollapsed = (s: ListSection) => (collapsed[s.id] ?? !!s.collapsedByDefault) && !holdsSelection(s);
  const visible = (s: ListSection) => {
    if (showAll[s.id] || s.items.length <= PREVIEW) return s.items;
    const at = s.items.findIndex((i) => i.id === selectedId);
    return s.items.slice(0, Math.max(PREVIEW, at + 1));
  };

  const select = (item: Item | null) => onSelect(item?.id ?? null, item ? { item } : null);

  // Everything keyboard navigation can land on, in display order.
  const entries: Item[] = useMemo(
    () => sections.flatMap((s) => (isCollapsed(s) ? [] : visible(s))),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [sections, collapsed, showAll, selectedId],
  );

  useEffect(() => {
    if (!active) return;
    const onKey = (e: KeyboardEvent) => {
      const tag = (document.activeElement?.tagName ?? "").toLowerCase();
      if (tag === "input" || tag === "textarea" || tag === "select" || e.metaKey || e.ctrlKey || e.altKey || menuFor) return;
      const idx = entries.findIndex((x) => x.id === selectedId);
      const cur = idx >= 0 ? entries[idx] : null;
      const move = (d: number) => {
        if (!entries.length) return;
        select(entries[idx < 0 ? 0 : Math.min(entries.length - 1, Math.max(0, idx + d))]);
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
          if (cur) openItem(cur);
          break;
        case "e":
          if (cur) {
            move(1);
            setStates([cur], "done");
          }
          break;
        case "l":
          if (cur) emit("focus-snooze");
          break;
        case ".":
          if (cur) openMenu(cur.id);
          break;
        case "Escape":
          if (!cur) return;
          select(null);
          break;
        case "x":
          if (cur?.personal) setStates([cur], null);
          break;
        case "R":
          if (cur && cur.kind !== "run") openDialog("reply", cur);
          break;
        case "a":
          if (cur?.kind === "pr" && !cur.isMine) openDialog("approve", cur);
          break;
        case "m":
          if (cur?.kind === "pr") openDialog("merge", cur);
          break;
        case "c":
          if (cur && cur.kind !== "run") openInEditor(cur, cur.kind === "pr");
          break;
        default:
          return;
      }
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active, entries, selectedId, menuFor]);

  return (
    <div className="list">
      {sections.map((s) => {
        if (!s.items.length && !s.empty) return null;
        const closed = isCollapsed(s);
        const shown = visible(s);
        const hidden = s.items.length - shown.length;
        return (
          <section key={s.id} className="section">
            <button className="section-head" onClick={() => setCollapsed({ ...collapsed, [s.id]: !closed })}>
              <Icon name="chevronDown" size={13} className={`chevron${closed ? " closed" : ""}`} />
              <span className="section-title">{s.title}</span>
              <span className="count">{s.items.length}</span>
            </button>
            {!closed && !s.items.length && <div className="empty">{s.empty}</div>}
            {!closed &&
              shown.map((item) => {
                const selected = item.id === selectedId;
                return (
                  <div key={item.id} className={`entry${selected ? " open" : ""}`}>
                    <PrRow
                      item={item}
                      now={now}
                      selected={selected}
                      onSelect={() => select(selected ? null : item)}
                      menuOpen={menuFor === item.id}
                      onMenu={(open) => openMenu(open ? item.id : null)}
                      focusNote={focusNote}
                    />
                    {selected && <InlineDetail item={item} />}
                  </div>
                );
              })}
            {!closed && hidden > 0 && (
              <button className="show-more" onClick={() => setShowAll({ ...showAll, [s.id]: true })}>
                Show {hidden} more
              </button>
            )}
            {!closed && showAll[s.id] && s.items.length > PREVIEW && (
              <button className="show-more" onClick={() => setShowAll({ ...showAll, [s.id]: false })}>
                Show fewer
              </button>
            )}
          </section>
        );
      })}
    </div>
  );
}
