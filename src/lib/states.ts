import type { Item, StateId } from "./api";
import type { IconName } from "../components/Icon";

export type Tone = "blue" | "violet" | "orange" | "red" | "green" | "yellow" | "gray";

/** How each state looks: an icon and a colour tone (replaces emojis in the UI). */
export const STATE_STYLE: Record<StateId, { icon: IconName; tone: Tone }> = {
  needs_review: { icon: "eye", tone: "blue" },
  needs_rereview: { icon: "refresh", tone: "blue" },
  replied: { icon: "message", tone: "violet" },
  needs_changes: { icon: "alert", tone: "orange" },
  ready_to_merge: { icon: "merge", tone: "green" },
  waiting: { icon: "clock", tone: "gray" },
  waiting_ci: { icon: "loader", tone: "yellow" },
  draft: { icon: "pencil", tone: "gray" },
  later: { icon: "pin", tone: "gray" },
  done: { icon: "checkCircle", tone: "green" },
  fyi: { icon: "eye", tone: "gray" },
  stale: { icon: "clock", tone: "gray" },
  muted: { icon: "bellOff", tone: "gray" },
  assigned: { icon: "issue", tone: "blue" },
  broken: { icon: "xCircle", tone: "red" },
};

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

/** Due reminders first, then latest GitHub activity first. */
export function compareItems(a: Item, b: Item): number {
  if (a.reminderDue !== b.reminderDue) return a.reminderDue ? -1 : 1;
  if (a.state === "later" && b.state === "later") {
    return (a.personal?.until ?? "9999").localeCompare(b.personal?.until ?? "9999");
  }
  return b.updatedAt.localeCompare(a.updatedAt);
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

/** Lists are organised by what you'd do next, most urgent first (no repo folders). */
export type TodoId =
  | "reminders"
  | "replies"
  | "review"
  | "fix"
  | "merge"
  | "assigned"
  | "waiting"
  | "parked"
  | "quiet"
  | "done";

export const TODO_SECTIONS: { id: TodoId; title: string; actionable: boolean }[] = [
  { id: "reminders", title: "Reminders", actionable: true },
  { id: "replies", title: "Replies to you", actionable: true },
  { id: "review", title: "Review requested", actionable: true },
  { id: "fix", title: "Fix: changes requested or failing", actionable: true },
  { id: "merge", title: "Ready to merge", actionable: true },
  { id: "assigned", title: "Assigned to you", actionable: true },
  { id: "waiting", title: "Waiting on others", actionable: false },
  { id: "parked", title: "Parked", actionable: false },
  { id: "quiet", title: "Gone quiet · 30+ days", actionable: false },
  { id: "done", title: "Done & FYI", actionable: false },
];

export function todoOf(item: Item): TodoId {
  if (item.reminderDue) return "reminders";
  switch (item.state) {
    case "replied":
      return "replies";
    case "needs_review":
    case "needs_rereview":
      return "review";
    case "needs_changes":
    case "broken":
      return "fix";
    case "ready_to_merge":
      return "merge";
    case "assigned":
      return "assigned";
    case "later":
      return "parked";
    case "stale":
      return "quiet";
    case "done":
    case "fyi":
    case "muted":
      return "done";
    default:
      return "waiting";
  }
}

export function groupTodo(items: Item[]): Record<TodoId, Item[]> {
  const out = Object.fromEntries(TODO_SECTIONS.map((s) => [s.id, [] as Item[]])) as Record<TodoId, Item[]>;
  for (const item of items) out[todoOf(item)].push(item);
  for (const k of Object.keys(out) as TodoId[]) out[k].sort(compareItems);
  return out;
}
