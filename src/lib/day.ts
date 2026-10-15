// Helpers for the Day view: local day boundaries, durations, colours and the copyable summary.

import type { DayReport, TimeBlock } from "./api";

const pad = (n: number) => String(n).padStart(2, "0");

/** "YYYY-MM-DD" for a local date. */
export function dayKey(d: Date): string {
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

export function parseDay(day: string): Date {
  const [y, m, d] = day.split("-").map(Number);
  return new Date(y, m - 1, d);
}

/** The local day's bounds as instants, for the backend. */
export function dayBounds(day: string): { start: string; end: string } {
  const s = parseDay(day);
  const e = new Date(s.getFullYear(), s.getMonth(), s.getDate() + 1);
  return { start: s.toISOString(), end: e.toISOString() };
}

export function addDays(day: string, n: number): string {
  const d = parseDay(day);
  d.setDate(d.getDate() + n);
  return dayKey(d);
}

/** Monday-to-Sunday around a day. */
export function weekOf(day: string): string[] {
  const d = parseDay(day);
  const monday = addDays(day, -((d.getDay() + 6) % 7));
  return Array.from({ length: 7 }, (_, i) => addDays(monday, i));
}

export function longDate(day: string): string {
  return parseDay(day).toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long" });
}

export function duration(minutes: number): string {
  if (minutes <= 0) return "0m";
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  return h ? (m ? `${h}h ${m}m` : `${h}h`) : `${m}m`;
}

export function hhmm(iso: string): string {
  const d = new Date(iso);
  return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** Set the time of day on a block's date, from "HH:MM". */
export function withTime(day: string, time: string): string {
  const [h, m] = time.split(":").map(Number);
  const d = parseDay(day);
  d.setHours(h, m, 0, 0);
  return d.toISOString();
}

/** A stable hue per ticket/PR so the same work is the same colour every day. */
export function hueFor(key: string): number {
  let h = 0;
  for (const c of key) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return h % 360;
}

/** The ticket key, if the group is a ticket ("T:IO-2641" → "IO-2641"). */
export function ticketOf(key: string): string | null {
  return key.startsWith("T:") ? key.slice(2) : null;
}

const KIND_WORDS: Record<string, [string, string]> = {
  committed: ["commit", "commits"],
  local_commit: ["commit", "commits"],
  pushed: ["push", "pushes"],
  reviewed: ["review", "reviews"],
  commented: ["comment", "comments"],
  opened: ["PR opened", "PRs opened"],
  merged: ["merge", "merges"],
};

export function eventSummary(kinds: string[]): string {
  const counts = new Map<string, number>();
  for (const k of kinds) {
    if (k === "edit") continue;
    const word = KIND_WORDS[k]?.[0] ?? k;
    counts.set(word, (counts.get(word) ?? 0) + 1);
  }
  const parts = [...counts.entries()].map(([word, n]) => {
    const plural = Object.values(KIND_WORDS).find((p) => p[0] === word)?.[1] ?? `${word}s`;
    return `${n} ${n === 1 ? word : plural}`;
  });
  // Local edits are 5-minute samples, not discrete things: just say there was editing.
  return (kinds.includes("edit") ? ["local editing", ...parts] : parts).join(", ");
}

function ranges(blocks: TimeBlock[]): string {
  return blocks.map((b) => `${hhmm(b.start)}–${hhmm(b.end)}`).join(", ");
}

/** Readable text for a day: what you worked on, for how long, and when. */
export function summaryText(r: DayReport): string {
  const lines = [`${longDate(r.day)} — ${duration(r.totalMinutes)}`, ""];
  for (const g of r.groups.filter((g) => g.minutes > 0 || g.events.length)) {
    const bs = r.blocks.filter((b) => b.key === g.key);
    lines.push(`${g.label} — ${duration(g.minutes)}${bs.length ? ` (${ranges(bs)})` : ""}`);
    if (g.events.length) lines.push(`  ${eventSummary(g.events.map((e) => e.kind))}`);
    for (const b of bs) if (b.note) lines.push(`  ${b.note}`);
  }
  return lines.join("\n").trim() + "\n";
}
