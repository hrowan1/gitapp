// Builds a standup update from the personal history and the current inbox. No AI:
// it only rearranges facts the app already knows.

import type { Activity, Item } from "./api";
import { compareItems, isActionable, shortRef } from "./states";
import { age } from "./time";

export type Range = "last_working_day" | "today" | "week" | "7d";
export type Format = "markdown" | "plain";

export const RANGES: { id: Range; label: string }[] = [
  { id: "last_working_day", label: "Since last working day" },
  { id: "today", label: "Today so far" },
  { id: "week", label: "This week" },
  { id: "7d", label: "Last 7 days" },
];

const MAX_PER_LINE = 5;

function startOfDay(d: Date) {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate());
}

/** Start of the window "what I did" covers. */
export function rangeStart(range: Range, now = new Date()): Date {
  const today = startOfDay(now);
  switch (range) {
    case "today":
      return today;
    case "week": {
      const back = (today.getDay() + 6) % 7; // days since Monday
      return new Date(today.getTime() - back * 86400_000);
    }
    case "7d":
      return new Date(today.getTime() - 7 * 86400_000);
    case "last_working_day": {
      // Monday → Friday; Sunday → Friday; otherwise yesterday.
      const back = today.getDay() === 1 ? 3 : today.getDay() === 0 ? 2 : 1;
      return new Date(today.getTime() - back * 86400_000);
    }
  }
}

function heading(range: Range, now = new Date()): string {
  if (range === "today") return "Today so far";
  if (range === "week") return "This week";
  if (range === "7d") return "Last 7 days";
  const day = now.getDay();
  return day === 1 || day === 0 ? "Since Friday" : "Yesterday";
}

interface Ref {
  title: string;
  url?: string | null;
  ref: string;
}

function link(r: Ref, format: Format) {
  return format === "markdown" && r.url ? `[${r.title}](${r.url}) (${r.ref})` : `${r.title} (${r.ref})`;
}

function list(refs: string[]): string {
  if (refs.length <= MAX_PER_LINE) return refs.join(", ");
  return `${refs.slice(0, MAX_PER_LINE).join(", ")} +${refs.length - MAX_PER_LINE} more`;
}

function itemRef(i: Item): Ref {
  return { title: i.title, url: i.url, ref: shortRef(i) };
}

function activityRef(a: Activity): Ref {
  return {
    title: a.title ?? "",
    url: a.url,
    ref: a.repo ? shortRef({ kind: a.itemKind ?? (a.number ? "pr" : "run"), repo: a.repo, number: a.number ?? 0 }) : "",
  };
}

export function buildStandup(
  activity: Activity[],
  items: Item[],
  opts: { range: Range; format: Format; exclude: Set<string>; now?: Date },
): string {
  const now = opts.now ?? new Date();
  const start = rangeStart(opts.range, now).toISOString();
  const keep = (repo?: string | null) => !repo || !opts.exclude.has(repo);
  const acts = activity.filter((a) => a.at >= start && keep(a.repo));
  const its = items.filter((i) => keep(i.repo));
  const bullet = opts.format === "markdown" ? "- " : "• ";
  const out: string[] = [];
  const section = (title: string, lines: string[]) => {
    if (!lines.length) return;
    out.push(opts.format === "markdown" ? `**${title}**` : title);
    out.push(...lines.map((l) => bullet + l));
    out.push("");
  };

  // ----- What I did -----
  const did: string[] = [];
  const uniq = (as: Activity[]) => {
    const seen = new Map<string, Activity>();
    for (const a of as) if (a.prId && !seen.has(a.prId)) seen.set(a.prId, a); // newest first
    return [...seen.values()];
  };
  const merged = uniq(acts.filter((a) => a.kind === "merged" && a.isMine));
  if (merged.length) did.push(`Merged: ${list(merged.map((a) => link(activityRef(a), opts.format)))}`);
  const opened = uniq(acts.filter((a) => a.kind === "opened" && a.isMine));
  const openedPrs = opened.filter((a) => a.itemKind !== "issue");
  const openedIssues = opened.filter((a) => a.itemKind === "issue");
  if (openedPrs.length) did.push(`Opened: ${list(openedPrs.map((a) => link(activityRef(a), opts.format)))}`);
  if (openedIssues.length) did.push(`Filed: ${list(openedIssues.map((a) => link(activityRef(a), opts.format)))}`);
  const closedIssues = uniq(acts.filter((a) => a.kind === "closed" && a.isMine && a.itemKind === "issue"));
  if (closedIssues.length) did.push(`Closed: ${list(closedIssues.map((a) => link(activityRef(a), opts.format)))}`);
  const reviews = uniq(acts.filter((a) => a.kind === "reviewed" && !a.isMine));
  if (reviews.length) {
    const verdict = (d: string) =>
      d === "Approved" ? "approved" : d === "Requested changes" ? "requested changes" : "commented";
    const refs = reviews.map((a) => `${link(activityRef(a), opts.format)} – ${verdict(a.detail)}`);
    did.push(`Reviewed ${reviews.length} PR${reviews.length === 1 ? "" : "s"}: ${list(refs)}`);
  }
  const reviewed = new Set(reviews.map((a) => a.prId));
  const discussed = uniq(acts.filter((a) => a.kind === "commented" && !reviewed.has(a.prId)));
  if (discussed.length) did.push(`Discussed: ${list(discussed.map((a) => link(activityRef(a), opts.format)))}`);
  section(heading(opts.range, now), did.length ? did : ["Nothing tracked in this period"]);

  // ----- Today -----
  const today: string[] = [];
  const sorted = [...its].sort(compareItems);
  const by = (f: (i: Item) => boolean) => sorted.filter(f);
  const fixes = by((i) => i.isMine && (i.state === "needs_changes" || (i.kind === "run" && i.state === "broken")));
  for (const i of fixes) today.push(`Fix: ${link(itemRef(i), opts.format)} — ${i.reason}`);
  const merges = by((i) => i.state === "ready_to_merge");
  if (merges.length) today.push(`Merge: ${list(merges.map((i) => link(itemRef(i), opts.format)))}`);
  const toReview = by((i) => i.state === "needs_review" || i.state === "needs_rereview");
  if (toReview.length) today.push(`Review: ${list(toReview.map((i) => link(itemRef(i), opts.format)))}`);
  const replies = by((i) => i.state === "replied");
  if (replies.length) today.push(`Reply on: ${list(replies.map((i) => link(itemRef(i), opts.format)))}`);
  const assigned = by((i) => i.state === "assigned");
  if (assigned.length) today.push(`Pick up: ${list(assigned.map((i) => link(itemRef(i), opts.format)))}`);
  const endOfDay = new Date(startOfDay(now).getTime() + 86400_000).toISOString();
  const reminders = by(
    (i) => i.reminderDue || (i.personal?.state === "later" && !!i.personal.until && i.personal.until <= endOfDay),
  );
  for (const i of reminders) {
    const note = i.personal?.note ? ` (${i.personal.note.replace(/^“|”$/g, "")})` : "";
    today.push(`Come back to: ${link(itemRef(i), opts.format)}${note}`);
  }
  section("Today", today);

  // ----- Blocked / waiting -----
  const waiting: string[] = [];
  for (const i of by((i) => i.isMine && i.kind === "pr" && (i.state === "waiting" || i.state === "waiting_ci"))) {
    waiting.push(`${link(itemRef(i), opts.format)} — ${i.reason} (${age(i.since, now.getTime())})`);
  }
  for (const i of by((i) => i.kind === "run" && i.state === "broken" && !i.isMine)) {
    waiting.push(`${i.title} in ${i.repo} — ${i.reason} (${age(i.since, now.getTime())} ago)`);
  }
  for (const i of by((i) => i.personal?.state === "later" && !!i.personal.waitsOn)) {
    waiting.push(`${link(itemRef(i), opts.format)} — waiting on ${i.personal!.waitsOn}`);
  }
  section("Blocked / waiting", waiting);

  return out.join("\n").trim() + "\n";
}

/** Repos that would appear in the standup, for the exclude toggles. */
export function standupRepos(activity: Activity[], items: Item[], range: Range): string[] {
  const start = rangeStart(range).toISOString();
  const repos = new Set<string>();
  for (const a of activity) if (a.at >= start && a.repo && ["merged", "opened", "reviewed", "commented", "closed"].includes(a.kind)) repos.add(a.repo);
  for (const i of items) if (isActionable(i.state) || i.reminderDue || i.state === "waiting") repos.add(i.repo);
  return [...repos].sort((a, b) => a.localeCompare(b));
}
