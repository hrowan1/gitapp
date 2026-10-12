// Deterministic "natural language" for the command bar. No AI: keyword intents
// plus author/repo recognition cover most of the questions people actually ask.

import type { Item, StateId } from "./api";
import { STATES, isActionable, sectionOf } from "./states";

export interface ParsedQuery {
  /** Human summary of what was understood, e.g. "to review · by sarah". */
  label: string | null;
  /** True when the query was understood as a question rather than a text search. */
  understood: boolean;
  /** Suggests a view instead of a PR list (e.g. "what did I do yesterday"). */
  view?: "history";
  filter: (item: Item) => boolean;
}

const FORGOTTEN_DAYS = 3;

interface Intent {
  test: RegExp;
  label: string;
  match: (i: Item) => boolean;
}

const has = (...states: StateId[]) => (i: Item) => states.includes(i.state);

const INTENTS: Intent[] = [
  {
    test: /\bnew\b|changed since|since i (last )?(looked|checked)|unread|what'?s new|what changed/,
    label: "new since you last looked",
    match: (i) => i.isNew,
  },
  { test: /\bassigned\b/, label: "assigned to you", match: (i) => i.state === "assigned" || (i.kind === "issue" && i.isMine) },
  { test: /\b(main|master|branch(es)?|builds?|broken|red)\b/, label: "failing branches", match: (i) => i.kind === "run" },
  { test: /\bissues?\b/, label: "issues", match: (i) => i.kind === "issue" },
  { test: /\bre-?review/, label: "to re-review", match: has("needs_rereview") },
  { test: /\breview(?!ed)|\bto look at/, label: "to review", match: has("needs_review", "needs_rereview") },
  { test: /repl|respond|response|answer|got back/, label: "with replies", match: has("replied") },
  { test: /\bfail|\bci\b|checks?\b/, label: "failing checks", match: (i) => i.ci === "failure" },
  { test: /merge|ready to ship|approved/, label: "ready to merge", match: has("ready_to_merge") },
  { test: /later|postpone|snooz|come back|parked/, label: "parked for later", match: (i) => i.state === "later" || i.reminderDue },
  {
    test: /forg[eo]t|stale|neglect|slipp|dropped|lost track/,
    label: "you may have forgotten",
    match: (i) =>
      i.reminderDue ||
      (i.state === "stale" && i.isMine) ||
      ((isActionable(i.state) || i.state === "waiting") && Date.now() - new Date(i.since).getTime() > FORGOTTEN_DAYS * 86400_000),
  },
  { test: /wait|blocked on|blocking|pending/, label: "waiting on others", match: (i) => i.state === "waiting" || i.state === "waiting_ci" },
  { test: /draft/, label: "drafts", match: has("draft") },
  { test: /changes|fix|need to (make|change)/, label: "needing your changes", match: has("needs_changes") },
  {
    test: /attention|focus|today|should i|need(s)? (me|my)|my plate|important/,
    label: "need you",
    match: (i) => sectionOf(i) === "needs",
  },
];

const STOPWORDS = new Set(
  "what which where when who show find list give need needs needing review reviews someone replied reply failing fail checks waiting wait have forgotten forget about work worked yesterday should focus today from with mine open pull pulls request requests that this there them they their your mean anything everything things thing prs the and for are was did does any all some been".split(
    " ",
  ),
);

/** Words that only express intent; never used as title text. */
const INTENT_WORD = /^(re-?review|review|repl|respon|answer|fail|broken|checks?$|merge|approv|ready|later|postpon|snooz|park|forg|stale|neglect|wait|block|pend|draft|change|fix|attention|focus|today|important|plate|new$|unread|changed|muted?$|mine$|yours?$|opened|wrote|issues?$|assigned$|branch|builds?$|main$|master$)/;

const HISTORY = /yesterday|did i|i did|worked on|history|this week|last week|have i (done|reviewed)/;

export function parseQuery(raw: string, items: Item[]): ParsedQuery {
  const q = raw.toLowerCase().trim();
  if (!q) return { label: null, understood: false, filter: () => true };

  const parts: string[] = [];
  const preds: ((i: Item) => boolean)[] = [];

  for (const intent of INTENTS) {
    if (intent.test.test(q)) {
      parts.push(intent.label);
      preds.push(intent.match);
      break; // first (most specific) intent wins
    }
  }

  if (/\b(my|mine|i opened|i wrote)\b/.test(q) && /\bpr|work|mine|opened|wrote/.test(q)) {
    parts.push("yours");
    preds.push((i) => i.isMine);
  }

  // People and repos mentioned by name.
  const words = q.replace(/'s\b/g, "").split(/[^a-z0-9_.\-/]+/).filter((w) => w.length >= 3);
  const authors = [...new Set(items.map((i) => i.author.toLowerCase()))];
  const author = words
    .map((w) => authors.find((a) => a === w || (w.length >= 4 && !STOPWORDS.has(w) && a.startsWith(w))))
    .find(Boolean);
  if (author) {
    parts.push(`by ${author}`);
    preds.push((i) => i.author.toLowerCase() === author);
  }
  const repos = [...new Set(items.map((i) => i.repo.toLowerCase()))];
  const repo = words
    .map((w) => repos.find((r) => r === w || r.split("/")[1] === w || r.split("/")[0] === w))
    .find(Boolean);
  if (repo) {
    parts.push(`in ${repo}`);
    preds.push((i) => i.repo.toLowerCase() === repo || i.repo.toLowerCase().startsWith(repo + "/"));
  }

  if (preds.length > 0) {
    // Leftover words ("sarah's *checkout* PR") narrow by title, but only if that still finds something.
    const leftovers = words.filter(
      (w) => !STOPWORDS.has(w) && !INTENT_WORD.test(w) && w !== author && !(repo && repo.includes(w)) && !(author && author.startsWith(w)),
    );
    const base = (i: Item) => preds.every((p) => p(i));
    const textMatch = (i: Item) => leftovers.every((w) => i.title.toLowerCase().includes(w));
    if (leftovers.length && items.some((i) => base(i) && textMatch(i))) {
      parts.push(`“${leftovers.join(" ")}”`);
      return { label: parts.join(" · "), understood: true, filter: (i) => base(i) && textMatch(i) };
    }
    return { label: parts.join(" · "), understood: true, filter: base };
  }
  if (HISTORY.test(q)) {
    return { label: "your history", understood: true, view: "history", filter: () => false };
  }

  // Plain text search over title, repo, author and number.
  const terms = q.replace(/^#/, "").split(/\s+/).filter((t) => t && !["the", "pr", "my", "a"].includes(t));
  return {
    label: null,
    understood: false,
    filter: (i) => {
      const hay = `${i.title} ${i.repo} ${i.author} #${i.number} ${i.number}`.toLowerCase();
      return terms.every((t) => hay.includes(t));
    },
  };
}

export const SUGGESTIONS = [
  "What needs my attention?",
  "What's new since I last looked?",
  "What do I need to review?",
  "Show me PRs where someone replied to me",
  "What am I waiting on?",
  "What PRs are failing checks?",
  "What have I forgotten about?",
  "What did I work on yesterday?",
];

// ---------- actions ("remind me about the checkout PR tomorrow") ----------

export type PaletteAction =
  | { kind: "state"; state: StateId; until?: string | null; note?: string | null }
  | { kind: "reset" }
  | { kind: "open" }
  | { kind: "rerun" }
  | { kind: "checkout" }
  | { kind: "unfollow" };

export interface ParsedAction {
  action: PaletteAction;
  /** e.g. "📌 Remind tomorrow 09:00 — after the API migration" */
  label: string;
  /** The part of the query that identifies the PR(s). */
  target: string;
}

function atHour(d: Date, h: number) {
  const x = new Date(d);
  x.setHours(h, 0, 0, 0);
  return x;
}

/** Recognise a time phrase; returns the ISO time and the phrase so it can be removed. */
function parseWhen(q: string): { until: string; phrase: string; label: string } | null {
  const now = new Date();
  let m: RegExpMatchArray | null;
  if ((m = q.match(/\b(tomorrow( morning)?)\b/)))
    return { until: atHour(new Date(now.getTime() + 86400_000), 9).toISOString(), phrase: m[0], label: "tomorrow morning" };
  if ((m = q.match(/\b(next week|on monday|monday)\b/))) {
    const days = ((8 - now.getDay()) % 7) || 7;
    return { until: atHour(new Date(now.getTime() + days * 86400_000), 9).toISOString(), phrase: m[0], label: "next Monday" };
  }
  if ((m = q.match(/\bin (\d+|an?|one) ?(h|hrs?|hours?)\b/))) {
    const n = /\d/.test(m[1]) ? Number(m[1]) : 1;
    return { until: new Date(now.getTime() + n * 3600_000).toISOString(), phrase: m[0], label: `in ${n}h` };
  }
  if ((m = q.match(/\bin (\d+|a|one) ?(d|days?)\b/))) {
    const n = /\d/.test(m[1]) ? Number(m[1]) : 1;
    return { until: atHour(new Date(now.getTime() + n * 86400_000), 9).toISOString(), phrase: m[0], label: `in ${n} day${n === 1 ? "" : "s"}` };
  }
  if ((m = q.match(/\b(later today|this afternoon|later)\b/)))
    return { until: new Date(now.getTime() + 3 * 3600_000).toISOString(), phrase: m[0], label: "in 3 hours" };
  return null;
}

const MARK_AS: [RegExp, StateId][] = [
  [/re-?review/, "needs_rereview"],
  [/review/, "needs_review"],
  [/waiting|blocked/, "waiting"],
  [/\bci\b|checks/, "waiting_ci"],
  [/changes|fix/, "needs_changes"],
  [/done|finished|complete/, "done"],
];

export function parseAction(raw: string): ParsedAction | null {
  const original = raw.trim();
  let q = original.toLowerCase();
  if (!q) return null;
  const strip = (re: RegExp) => {
    q = q.replace(re, " ").replace(/\s+/g, " ").trim();
  };

  // Snooze / remind / come back to, with an optional time and an optional "after X" note.
  let m = q.match(/^(remind me( about| of| to look at)?|snooze|come back to|revisit|park|look at .* later)\b/);
  if (m) {
    const verb = m[1];
    let note: string | null = null;
    const nm = original.match(/\b(after|once|when|until)\b(.+)$/i);
    if (nm && !parseWhen(nm[0].toLowerCase())) {
      note = (nm[1] + nm[2]).trim();
      q = q.slice(0, nm.index).trim();
    }
    const when = parseWhen(q);
    if (when) strip(new RegExp(`\\b${when.phrase}\\b`));
    strip(/^(remind me( about| of| to look at)?|snooze|come back to|revisit|park)\b/);
    strip(/\blater\b/);
    // "remind me" with no time means tomorrow; "come back to X after Y" waits for something to change.
    const fallback = verb.startsWith("remind") && !note ? parseWhen("tomorrow") : null;
    const until = when?.until ?? fallback?.until ?? null;
    const whenLabel = when?.label ?? fallback?.label ?? "when something changes";
    return {
      action: { kind: "state", state: "later", until, note },
      label: `📌 Come back ${whenLabel}${note ? ` — ${note}` : ""}`,
      target: q,
    };
  }

  if ((m = q.match(/^mark (.+?) as (.+)$/))) {
    const [, what, as] = m;
    const hit = MARK_AS.find(([re]) => re.test(as));
    if (hit) return { action: { kind: "state", state: hit[1] }, label: `${STATES[hit[1]].emoji} Mark as ${STATES[hit[1]].label.toLowerCase()}`, target: what };
  }
  if (/^(done|complete|finish(ed)?|archive|dismiss)\b/.test(q) || /\b(is )?done$/.test(q)) {
    strip(/^(done|complete|finish(ed)?|archive|dismiss)\b|\b(mark|as|is)\b|\bdone$/g);
    return { action: { kind: "state", state: "done" }, label: "✅ Mark done", target: q };
  }
  if ((m = q.match(/^(mute|unfollow|stop following|ignore)\b/)))
    return { action: { kind: "unfollow" }, label: "🔕 Stop following", target: q.slice(m[0].length).trim() };
  if (/^(unmute|reset|unsnooze)\b/.test(q))
    return { action: { kind: "reset" }, label: "↺ Back to automatic", target: q.replace(/^\S+/, "").trim() };
  if (/^open\b/.test(q)) return { action: { kind: "open" }, label: "↗ Open", target: q.replace(/^open\b/, "").trim() };
  if (/^(re-?run|retry)\b/.test(q))
    return {
      action: { kind: "rerun" },
      label: "↻ Re-run failed checks",
      target: q.replace(/^(re-?run|retry)\b( (the )?(failed )?(checks?|ci|workflows?|jobs?|builds?)( on| for)?)?/, "").trim(),
    };
  if (/^(check ?out|copy checkout)\b/.test(q))
    return { action: { kind: "checkout" }, label: "⧉ Copy checkout command", target: q.replace(/^(check ?out|copy checkout)\b/, "").trim() };
  return null;
}
