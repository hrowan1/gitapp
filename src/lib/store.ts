// The current inbox items, for components that need to look across everything
// (e.g. suggesting which PR a snooze should wait for).

import type { Item } from "./api";

let current: Item[] = [];

export const setAllItems = (items: Item[]) => {
  current = items;
};

export const allItems = () => current;

const STOP = new Set(
  "the this that these those then than with lands landed merges merged ships shipped done goes out are has have been gets get and for into after once until when".split(
    " ",
  ),
);

/** Open PRs/issues whose title contains every meaningful word of `text` ("after the API migration"). */
export function matchTitles(text: string, excludeId: string, limit = 3): Item[] {
  const words = text
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter((w) => w.length >= 3 && !STOP.has(w));
  if (!words.length) return [];
  return current
    .filter((i) => i.id !== excludeId && i.kind !== "run")
    .filter((i) => words.every((w) => i.title.toLowerCase().includes(w)))
    .slice(0, limit);
}
