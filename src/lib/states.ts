import type { Item, StateId } from "./api";

export const STATES: Record<StateId, { emoji: string; label: string }> = {
  needs_review: { emoji: "👀", label: "Need to review" },
  needs_rereview: { emoji: "🔄", label: "Needs re-review" },
  replied: { emoji: "💬", label: "Replied to you" },
  needs_changes: { emoji: "🔧", label: "I need to make changes" },
  ready_to_merge: { emoji: "🚀", label: "Ready to merge" },
  waiting: { emoji: "⏳", label: "Waiting for response" },
  waiting_ci: { emoji: "🧪", label: "Waiting for CI" },
  draft: { emoji: "✏️", label: "Draft" },
  later: { emoji: "📌", label: "Come back later" },
  done: { emoji: "✅", label: "Done" },
  fyi: { emoji: "👋", label: "FYI" },
  stale: { emoji: "🕸️", label: "Gone quiet" },
  muted: { emoji: "🔇", label: "Muted" },
  assigned: { emoji: "📋", label: "Assigned to you" },
  broken: { emoji: "🔴", label: "Branch failing" },
};

/** States the user can pick by hand. */
export const MANUAL_STATES: StateId[] = [
  "needs_review",
  "needs_changes",
  "waiting",
  "needs_rereview",
  "waiting_ci",
  "done",
];

const ACTIONABLE: StateId[] = [
  "needs_rereview",
  "replied",
  "needs_review",
  "assigned",
  "needs_changes",
  "broken",
  "ready_to_merge",
];

export const isActionable = (s: StateId) => ACTIONABLE.includes(s);

export type SectionId = "needs" | "later" | "mine" | "waiting" | "stale" | "done";

export const SECTIONS: { id: SectionId; title: string; empty: string }[] = [
  { id: "needs", title: "Needs you", empty: "Nothing needs you right now." },
  { id: "later", title: "Come back to", empty: "" },
  { id: "mine", title: "Your work", empty: "" },
  { id: "waiting", title: "Waiting on others", empty: "" },
  { id: "stale", title: "Gone quiet · 30+ days", empty: "" },
  { id: "done", title: "Done & FYI", empty: "" },
];

export function sectionOf(item: Item): SectionId {
  if (item.reminderDue) return "needs";
  if (item.state === "later") return "later";
  if (isActionable(item.state)) return "needs";
  if (item.state === "stale") return "stale";
  if (item.state === "done" || item.state === "fyi") return "done";
  return item.isMine ? "mine" : "waiting";
}

/** Most urgent first: due reminders, then by state priority, then most recent. */
export function compareItems(a: Item, b: Item): number {
  if (a.reminderDue !== b.reminderDue) return a.reminderDue ? -1 : 1;
  const pa = ACTIONABLE.indexOf(a.state);
  const pb = ACTIONABLE.indexOf(b.state);
  if (pa !== pb) return (pa === -1 ? 99 : pa) - (pb === -1 ? 99 : pb);
  if (a.state === "later" && b.state === "later") {
    return (a.personal?.until ?? "9999").localeCompare(b.personal?.until ?? "9999");
  }
  return b.since.localeCompare(a.since);
}

export function groupItems(items: Item[]): Record<SectionId, Item[]> {
  const out: Record<SectionId, Item[]> = { needs: [], later: [], mine: [], waiting: [], stale: [], done: [] };
  for (const item of items) out[sectionOf(item)].push(item);
  for (const k of Object.keys(out) as SectionId[]) out[k].sort(compareItems);
  return out;
}

/** "web#482", "web issue #88", or just the repo for a failing branch. */
export function shortRef(item: { kind: string; repo: string; number: number }): string {
  const name = item.repo.split("/")[1] ?? item.repo;
  if (item.kind === "run") return name;
  return item.kind === "issue" ? `${name} issue #${item.number}` : `${name}#${item.number}`;
}

export const KIND_LABEL: Record<string, string> = { pr: "Pull request", issue: "Issue", run: "Branch" };
