import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import type { Item } from "../lib/api";
import { DetailPane, type Selection } from "./DetailPane";

/** Selection that survives refreshes: re-resolves against the latest items by id. `focus` opens an
 *  item from elsewhere (a notification, the menu bar panel) once it's in the list. */
export function useSelection(items: Item[], focus?: { id: string; n: number } | null) {
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [raw, setRaw] = useState<Selection>(null);

  useEffect(() => {
    if (!focus) return;
    const item = items.find((i) => i.id === focus.id);
    if (item) {
      setSelectedId(item.id);
      setRaw({ item });
      // Bring its row into view once it's rendered.
      requestAnimationFrame(() => {
        document.querySelector(`[data-item-id="${CSS.escape(item.id)}"]`)?.scrollIntoView({ block: "center" });
      });
    }
    // Only when asked to focus (or the item arrives); not on every refresh.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focus, items.some((i) => i.id === focus?.id)]);

  const selection: Selection = useMemo(() => {
    if (!raw) return null;
    const byId = new Map(items.map((i) => [i.id, i]));
    if ("item" in raw) {
      const latest = byId.get(raw.item.id);
      return latest ? { item: latest } : null;
    }
    const latest = raw.group.items.map((i) => byId.get(i.id)).filter((i): i is Item => !!i);
    return latest.length ? { group: { repo: raw.group.repo, items: latest } } : null;
  }, [raw, items]);

  // Whatever was selected is gone (done, merged, unfollowed): clear it.
  useEffect(() => {
    if (raw && !selection) {
      setSelectedId(null);
      setRaw(null);
    }
  }, [raw, selection]);

  const onSelect = useCallback((id: string | null, sel: Selection) => {
    setSelectedId(id);
    setRaw(sel);
  }, []);
  const clear = useCallback(() => onSelect(null, null), [onSelect]);
  return { selectedId, selection, onSelect, clear };
}

/** List on the left, details on the right (a drawer on narrow windows). */
export function Workspace({
  header,
  children,
  selection,
  now,
  onClose,
}: {
  header: ReactNode;
  children: ReactNode;
  selection: Selection;
  now: number;
  onClose: () => void;
}) {
  return (
    <div className={`workspace${selection ? " has-selection" : ""}`}>
      <section className="pane-list">
        <div className="pane-header">{header}</div>
        <div className="pane-scroll">{children}</div>
      </section>
      <DetailPane selection={selection} now={now} onClose={selection ? onClose : undefined} />
    </div>
  );
}
