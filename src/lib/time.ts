export function age(iso: string, now = Date.now()): string {
  const s = Math.max(0, (now - new Date(iso).getTime()) / 1000);
  if (s < 60) return "now";
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  if (s < 86400 * 14) return `${Math.floor(s / 86400)}d`;
  if (s < 86400 * 60) return `${Math.floor(s / (86400 * 7))}w`;
  return `${Math.floor(s / (86400 * 30))}mo`;
}

export function clock(iso: string): string {
  return new Date(iso).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

export function friendlyDate(iso: string): string {
  const d = new Date(iso);
  const today = new Date();
  const startOf = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const diff = Math.round((startOf(today) - startOf(d)) / 86400000);
  if (diff === 0) return "Today";
  if (diff === 1) return "Yesterday";
  return d.toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "short" });
}

export function untilLabel(iso: string): string {
  const d = new Date(iso);
  const diff = Math.round(
    (new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime() -
      new Date(new Date().getFullYear(), new Date().getMonth(), new Date().getDate()).getTime()) /
      86400000,
  );
  const time = clock(iso);
  if (diff === 0) return `today ${time}`;
  if (diff === 1) return `tomorrow ${time}`;
  if (diff < 7) return `${d.toLocaleDateString(undefined, { weekday: "short" })} ${time}`;
  return d.toLocaleDateString(undefined, { day: "numeric", month: "short" });
}

export type SnoozePreset = { id: string; label: string; until: () => string | null };

function at(date: Date, hour: number): Date {
  const d = new Date(date);
  d.setHours(hour, 0, 0, 0);
  return d;
}

export const SNOOZE_PRESETS: SnoozePreset[] = [
  { id: "3h", label: "In 3 hours", until: () => new Date(Date.now() + 3 * 3600_000).toISOString() },
  {
    id: "tomorrow",
    label: "Tomorrow morning",
    until: () => at(new Date(Date.now() + 86400_000), 9).toISOString(),
  },
  {
    id: "monday",
    label: "Next Monday",
    until: () => {
      const d = new Date();
      const days = ((8 - d.getDay()) % 7) || 7;
      return at(new Date(d.getTime() + days * 86400_000), 9).toISOString();
    },
  },
  { id: "change", label: "Until something changes", until: () => null },
];
