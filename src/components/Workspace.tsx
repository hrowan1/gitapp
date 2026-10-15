import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import type { Item } from "../lib/api";
import { DetailPane, type Selection } from "./DetailPane";

/** Selection that survives refreshes: re-resolves against the latest items by id. */
export function useSelection(items: Item[]) {
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [raw, setRaw] = useState<Selection>(null);

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
