import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  api,
  type HarvestBucket,
  type HarvestProject,
  type HarvestStatus,
  type TimesheetCell,
  type TimesheetRow,
  type TimesheetWeek,
} from "../lib/api";
import { addDays, dayKey, parseDay, weekOf } from "../lib/day";
import { toast } from "../lib/toast";
import { Icon } from "./Icon";

const OTHER = "__other";

/** Minutes as hours for a cell: "", "0.25", "1.5", "2". */
function hoursText(minutes: number): string {
  if (minutes <= 0) return "";
  return String(Math.round((minutes / 60) * 100) / 100);
}

/** Total as "7h 30m". */
function total(minutes: number): string {
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  return h && m ? `${h}h ${m}m` : h ? `${h}h` : `${m}m`;
}

/** What you typed, in minutes: "1.5", "1:30", "90m", "1h30". Undefined if it isn't a time. */
function parseHours(text: string): number | undefined {
  const t = text.trim().toLowerCase();
  if (t === "") return 0;
  let m = t.match(/^(\d+):(\d{1,2})$/);
  if (m) return Number(m[1]) * 60 + Number(m[2]);
  m = t.match(/^(\d+)\s*m$/);
  if (m) return Number(m[1]);
  m = t.match(/^(\d+)\s*h\s*(\d+)\s*m?$/);
  if (m) return Number(m[1]) * 60 + Number(m[2]);
  m = t.match(/^(\d*\.?\d+)\s*h?$/);
  if (m) return Math.round(Number(m[1]) * 60);
  return undefined;
}

const round15 = (m: number) => Math.round(Math.max(0, m) / 15) * 15;

function weekLabel(days: string[]): string {
  if (days.length === 0) return "";
  const a = parseDay(days[0]);
  const b = parseDay(days[days.length - 1]);
  const month = (d: Date) => d.toLocaleDateString(undefined, { month: "short" });
  return a.getMonth() === b.getMonth()
    ? `${a.getDate()} – ${b.getDate()} ${month(b)}`
    : `${a.getDate()} ${month(a)} – ${b.getDate()} ${month(b)}`;
}

/**
 * The week's hours per ticket, from the Day view, ready for Harvest: adjust any cell, choose each
 * ticket's Harvest bucket once (remembered for its Jira project), then submit a day or the week.
 */
/** Weeks loaded this session (by Monday), and Harvest's connection and projects: coming back to
 * the Timesheet shows them at once, then refreshes quietly. */
const weekCache = new Map<string, TimesheetWeek>();
let harvestCache: { status: HarvestStatus; projects: HarvestProject[] | null } | null = null;

export function TimesheetView({
  version,
  onSettings,
  onOpenTicket,
}: {
  version: number;
  onSettings: () => void;
  onOpenTicket: (key: string) => void;
}) {
  const today = dayKey(new Date());
  const [day, setDay] = useState(today);
  const [week, setWeek] = useState<TimesheetWeek | null>(() => weekCache.get(weekOf(today)[0]) ?? null);
  const [status, setStatus] = useState<HarvestStatus | null>(harvestCache?.status ?? null);
  const [projects, setProjects] = useState<HarvestProject[] | null>(harvestCache?.projects ?? null);
  const [submitting, setSubmitting] = useState<string | null>(null);
  const [picking, setPicking] = useState<string | null>(null);

  // The day on screen now, for answers that arrive after you've moved to another week.
  const shown = useRef(day);
  shown.current = day;

  const load = useCallback(() => {
    const monday = weekOf(day)[0];
    api
      .timesheetWeek(day)
      .then((w) => {
        weekCache.set(monday, w);
        // Only if that's still the week on screen.
        setWeek((cur) => (weekOf(shown.current)[0] === monday ? w : cur));
      })
      .catch((e) => toast(String(e), true));
  }, [day]);

  // Another week: show it from the cache straight away if it's been loaded before.
  useEffect(() => setWeek(weekCache.get(weekOf(day)[0]) ?? null), [day]);
  useEffect(load, [load, version]);
  useEffect(() => {
    api.harvestStatus().then((s) => {
      setStatus(s);
      harvestCache = { status: s, projects: harvestCache?.projects ?? null };
      if (s.connected)
        api
          .harvestProjects()
          .then((p) => {
            setProjects(p);
            harvestCache = { status: s, projects: p };
          })
          .catch(() => setProjects([]));
    });
  }, []);

  const days = week?.days ?? [];
  const rows = week?.rows ?? [];
  const dayTotals = useMemo(() => days.map((_, i) => rows.reduce((n, r) => n + (r.cells[i]?.minutes ?? 0), 0)), [days, rows]);
  const weekTotal = dayTotals.reduce((a, b) => a + b, 0);
  const waiting = (d: string) => rows.some((r) => r.cells.some((c) => c.day === d && !c.submitted));
  const weekWaiting = days.filter((d) => d <= today && waiting(d));

  const submit = async (which: string[], label: string) => {
    setSubmitting(label);
    try {
      const r = await api.timesheetSubmit(which);
      const sent = r.created + r.updated + r.removed;
      if (sent > 0) {
        const parts = [
          r.created && `${r.created} added`,
          r.updated && `${r.updated} updated`,
          r.removed && `${r.removed} removed`,
        ].filter(Boolean);
        toast(`Harvest: ${parts.join(", ")}`);
      }
      if (r.skipped.length > 0) toast(`Not sent — ${r.skipped.join("; ")}`, true);
      else if (sent === 0) toast("Nothing new to send");
      load();
    } catch (e) {
      toast(String(e), true);
      load();
    } finally {
      setSubmitting(null);
    }
  };

  const setCell = async (row: TimesheetRow, cell: TimesheetCell, minutes: number | null) => {
    try {
      await api.timesheetSet(cell.day, row.key, minutes);
      load();
    } catch (e) {
      toast(String(e), true);
    }
  };

  const chooseBucket = async (row: string, p: HarvestProject, t: { id: number; name: string }) => {
    setPicking(null);
    try {
      await api.timesheetBucket(row, p.id, p.name, t.id, t.name);
      load();
    } catch (e) {
      toast(String(e), true);
    }
  };

  const connected = status?.connected ?? false;

  return (
    <div className="page timesheet-page">
      <div className="day-header">
        <div className="headline compact">
          <h1>Timesheet</h1>
          <p className="muted">
            {week ? (
              <>
                {weekLabel(days)} · <span className="strong total">{total(weekTotal)}</span>
                {connected && status?.name && <> · Harvest as {status.name}</>}
              </>
            ) : (
              "Loading…"
            )}
          </p>
        </div>
        <div className="day-nav">
          <button className="ghost icon-only" onClick={() => setDay(addDays(day, -7))} title="Previous week">
            <Icon name="chevronRight" className="flip" />
          </button>
          <button onClick={() => setDay(today)} disabled={days.includes(today)}>
            This week
          </button>
          <button
            className="ghost icon-only"
            onClick={() => setDay(addDays(day, 7))}
            disabled={days.includes(today) || day > today}
            title="Next week"
          >
            <Icon name="chevronRight" />
          </button>
          {connected && (
            <button
              className="primary"
              disabled={!!submitting || weekWaiting.length === 0}
              onClick={() => void submit(weekWaiting, "week")}
              title="Send every day of this week that has changes"
            >
              {submitting === "week" ? "Sending…" : "Submit week"}
            </button>
          )}
        </div>
      </div>

      {status && !connected && (
        <div className="banner">
          <Icon name="clock" size={14} />
          <span className="grow">These are your hours from the Day view. Connect Harvest to send them as timesheet entries.</span>
          <button className="link" onClick={onSettings}>
            Connect Harvest
          </button>
        </div>
      )}
      {week?.harvestError && (
        <div className="banner error">
          <Icon name="alert" size={14} />
          <span className="grow">Couldn't read Harvest: {week.harvestError}</span>
          <button className="link" onClick={load}>
            Retry
          </button>
        </div>
      )}

      {week && rows.length === 0 && (
        <div className="empty big">
          <Icon name="clock" size={22} />
          <p>No time tracked this week yet.</p>
          <p className="muted small">Hours come from the Day view: commits, reviews, meetings and the tickets they're for.</p>
        </div>
      )}

      {rows.length > 0 && (
        <div className="ts-scroll">
          <table className="ts">
            <thead>
              <tr>
                <th className="ts-what">What</th>
                {days.map((d) => (
                  <th key={d} className={`ts-day${d === today ? " today" : ""}`}>
                    <span className="wd-name">{parseDay(d).toLocaleDateString(undefined, { weekday: "short" })}</span>
                    <span className="wd-date">{parseDay(d).getDate()}</span>
                  </th>
                ))}
                <th className="ts-sum">Total</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => (
                <tr key={row.key} className={row.key === OTHER ? "ts-other" : undefined}>
                  <td className="ts-what">
                    <div className="ts-title">
                      {row.key === OTHER ? (
                        <span className="strong">Other work</span>
                      ) : (
                        <button className="ticket-key link" onClick={() => onOpenTicket(row.key)}>
                          {row.key}
                        </button>
                      )}
                      <span className="ts-label" title={row.label}>
                        {row.key === OTHER ? "Meetings, reviews, and anything not on a ticket" : row.label}
                      </span>
                    </div>
                    {connected && (
                      <BucketButton
                        row={row}
                        open={picking === row.key}
                        projects={projects}
                        onOpen={() => setPicking(picking === row.key ? null : row.key)}
                        onClose={() => setPicking(null)}
                        onChoose={(p, t) => void chooseBucket(row.key, p, t)}
                      />
                    )}
                  </td>
                  {row.cells.map((c) => (
                    <td key={c.day} className="ts-cell-td">
                      <HoursCell cell={c} future={c.day > today} connected={connected} onSet={(m) => void setCell(row, c, m)} />
                    </td>
                  ))}
                  <td className="ts-sum">{total(row.cells.reduce((n, c) => n + c.minutes, 0))}</td>
                </tr>
              ))}
            </tbody>
            <tfoot>
              <tr>
                <td className="ts-what muted small">Rounded to the nearest 15 minutes</td>
                {days.map((d, i) => (
                  <td key={d} className="ts-foot">
                    <div className="strong">{dayTotals[i] ? total(dayTotals[i]) : "–"}</div>
                    {connected && d <= today && (
                      waiting(d) ? (
                        <button
                          className="small-btn"
                          disabled={!!submitting}
                          onClick={() => void submit([d], d)}
                          title="Send this day to Harvest"
                        >
                          {submitting === d ? "…" : "Submit"}
                        </button>
                      ) : (
                        dayTotals[i] > 0 && (
                          <span className="ts-sent" title="In Harvest">
                            <Icon name="check" size={11} /> Sent
                          </span>
                        )
                      )
                    )}
                  </td>
                ))}
                <td className="ts-sum strong">{total(weekTotal)}</td>
              </tr>
            </tfoot>
          </table>
        </div>
      )}
    </div>
  );
}

function HoursCell({
  cell,
  future,
  connected,
  onSet,
}: {
  cell: TimesheetCell;
  future: boolean;
  connected: boolean;
  onSet: (minutes: number | null) => void;
}) {
  const [text, setText] = useState(hoursText(cell.minutes));
  useEffect(() => setText(hoursText(cell.minutes)), [cell.minutes]);

  const commit = () => {
    const m = parseHours(text);
    if (m === undefined) {
      toast("Use hours like 1.5, 1:30 or 90m", true);
      setText(hoursText(cell.minutes));
      return;
    }
    const rounded = round15(m);
    if (rounded === cell.minutes) {
      setText(hoursText(cell.minutes));
      return;
    }
    // Back to what the Day view proposed: stop overriding it.
    onSet(!cell.edited || rounded !== Math.max(0, cell.proposed - cell.logged) ? rounded : null);
  };

  const hints: string[] = [];
  if (cell.proposed) hints.push(`Day view: ${total(cell.proposed)}`);
  if (cell.logged) hints.push(`Already in Harvest: ${total(cell.logged)}`);
  if (cell.edited) hints.push("You changed this");
  if (connected && cell.minutes > 0) hints.push(cell.submitted ? "Sent to Harvest" : "Not sent yet");

  return (
    <div
      className={`ts-cell${cell.edited ? " edited" : ""}${connected && cell.submitted && cell.minutes > 0 ? " sent" : ""}`}
      title={hints.join("\n")}
    >
      <input
        inputMode="decimal"
        value={text}
        disabled={future}
        placeholder={cell.logged ? "" : "–"}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") (e.target as HTMLInputElement).blur();
          if (e.key === "Escape") {
            setText(hoursText(cell.minutes));
            (e.target as HTMLInputElement).blur();
          }
        }}
      />
      {cell.logged > 0 && <span className="ts-logged">+{hoursText(cell.logged)} in Harvest</span>}
      {cell.edited && (
        <button className="ts-reset" title="Back to the Day view's estimate" onClick={() => onSet(null)}>
          <Icon name="undo" size={10} />
        </button>
      )}
    </div>
  );
}

const FROM: Record<string, string> = {
  ticket: "",
  project: "Remembered for this Jira project",
  default: "Your default for other work",
  rollover: "Same as last month, this month's project",
};

function BucketButton({
  row,
  open,
  projects,
  onOpen,
  onClose,
  onChoose,
}: {
  row: TimesheetRow;
  open: boolean;
  projects: HarvestProject[] | null;
  onOpen: () => void;
  onClose: () => void;
  onChoose: (p: HarvestProject, t: { id: number; name: string }) => void;
}) {
  const b: HarvestBucket | null = row.bucket;
  return (
    <div className="ts-bucket-wrap">
      <button
        className={`ts-bucket${b ? "" : " missing"}`}
        onClick={onOpen}
        title={b ? [FROM[b.from], "Change the Harvest project and task"].filter(Boolean).join(" · ") : "Choose where this goes in Harvest"}
      >
        {b ? (
          <>
            <span className="ts-bucket-name">{b.projectName}</span>
            <span className="muted"> · {b.taskName}</span>
            {b.from === "rollover" && <span className="pill">new month</span>}
          </>
        ) : (
          <>
            <Icon name="alert" size={11} /> Choose a Harvest project
          </>
        )}
        <Icon name="chevronDown" size={11} />
      </button>
      {open && <BucketPicker rowKey={row.key} projects={projects} onClose={onClose} onChoose={onChoose} />}
    </div>
  );
}

function BucketPicker({
  rowKey,
  projects,
  onClose,
  onChoose,
}: {
  rowKey: string;
  projects: HarvestProject[] | null;
  onClose: () => void;
  onChoose: (p: HarvestProject, t: { id: number; name: string }) => void;
}) {
  const [q, setQ] = useState("");
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const away = (e: MouseEvent) => {
      if (box.current && !box.current.contains(e.target as Node)) onClose();
    };
    document.addEventListener("mousedown", away);
    return () => document.removeEventListener("mousedown", away);
  }, [onClose]);

  const project = rowKey === OTHER ? null : rowKey.split("-")[0];
  const options = useMemo(() => {
    const words = q.toLowerCase().split(/\s+/).filter(Boolean);
    const out: { p: HarvestProject; t: { id: number; name: string } }[] = [];
    for (const p of projects ?? []) {
      for (const t of p.tasks) {
        const hay = `${p.client ?? ""} ${p.name} ${p.code ?? ""} ${t.name}`.toLowerCase();
        if (words.every((w) => hay.includes(w))) out.push({ p, t });
      }
    }
    // Projects that look like this ticket's Jira project first ("HAIR-12" → "Project Hair …").
    if (project) {
      const like = (p: HarvestProject) => `${p.name} ${p.code ?? ""}`.toLowerCase().includes(project.toLowerCase());
      out.sort((a, b) => Number(like(b.p)) - Number(like(a.p)));
    }
    return out.slice(0, 80);
  }, [projects, q, project]);

  return (
    <div className="ts-picker" ref={box}>
      <input
        autoFocus
        placeholder="Search Harvest projects and tasks…"
        value={q}
        onChange={(e) => setQ(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") onClose();
          if (e.key === "Enter" && options[0]) onChoose(options[0].p, options[0].t);
        }}
      />
      <div className="ts-picker-list">
        {projects === null && <div className="muted small pad">Loading Harvest projects…</div>}
        {projects !== null && options.length === 0 && <div className="muted small pad">No matching projects</div>}
        {options.map(({ p, t }) => (
          <button key={`${p.id}:${t.id}`} className="ts-picker-row" onClick={() => onChoose(p, t)}>
            <span className="grow">
              <span className="strong">{p.name}</span> <span className="muted">· {t.name}</span>
            </span>
            {p.client && <span className="muted small">{p.client}</span>}
          </button>
        ))}
      </div>
      <div className="muted small pad">
        {rowKey === OTHER
          ? "Used for all time that isn't on a ticket."
          : `Remembered for ${rowKey} and other ${project} tickets. Monthly projects roll over automatically.`}
      </div>
    </div>
  );
}
