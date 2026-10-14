// A single app-wide dialog for actions that write to GitHub (reply, review, merge),
// so rows, groups and the command bar can all open it.

import type { Item } from "./api";

export type DialogKind = "reply" | "approve" | "request_changes" | "review_comment" | "merge";
export interface DialogRequest {
  kind: DialogKind;
  item: Item;
}

type Listener = (d: DialogRequest | null) => void;
const listeners = new Set<Listener>();

export function openDialog(kind: DialogKind, item: Item) {
  listeners.forEach((l) => l({ kind, item }));
}

export function onDialog(l: Listener): () => void {
  listeners.add(l);
  return () => listeners.delete(l);
}
