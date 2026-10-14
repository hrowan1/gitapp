import { useEffect, useMemo, useState } from "react";
import { api, type Activity, type Item } from "../lib/api";
import { buildStandup, RANGES, standupRepos, type Format, type Range } from "../lib/standup";
import { toast } from "../lib/toast";

const PREFS_KEY = "dev-inbox.standup";

function loadPrefs(): { format: Format; exclude: string[] } {
  try {
    return { format: "markdown", exclude: [], ...JSON.parse(localStorage.getItem(PREFS_KEY) ?? "{}") };
  } catch {
    return { format: "markdown", exclude: [] };
  }
}

/** "What did I do yesterday?" as copyable standup text. */
export function Standup({ items, onClose }: { items: Item[]; onClose: () => void }) {
  const prefs = useMemo(loadPrefs, []);
  const [range, setRange] = useState<Range>("last_working_day");
  const [format, setFormat] = useState<Format>(prefs.format);
  const [exclude, setExclude] = useState<Set<string>>(new Set(prefs.exclude));
  const [activity, setActivity] = useState<Activity[] | null>(null);
  const [text, setText] = useState("");
  const [edited, setEdited] = useState(false);

  useEffect(() => {
    api.getActivity(10).then(setActivity).catch((e) => toast(String(e), true));
  }, []);

  useEffect(() => {
    try {
      localStorage.setItem(PREFS_KEY, JSON.stringify({ format, exclude: [...exclude] }));
    } catch {
      /* storage unavailable: preferences just won't persist */
    }
  }, [format, exclude]);

  const generated = useMemo(
    () => (activity ? buildStandup(activity, items, { range, format, exclude }) : ""),
    [activity, items, range, format, exclude],
  );
  useEffect(() => {
    if (!edited) setText(generated);
  }, [generated, edited]);

  const repos = useMemo(() => (activity ? standupRepos(activity, items, range) : []), [activity, items, range]);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      toast("Standup copied");
      onClose();
    } catch {
      toast("Couldn't copy — select the text and copy it manually", true);
    }
  };

  return (
    <div className="overlay" onMouseDown={onClose}>
      <div
        className="dialog wide"
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Escape") onClose();
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) copy();
        }}
      >
        <div className="dialog-head">
          <span className="strong">Standup</span>
          <span className="muted">from your history and inbox</span>
        </div>
        <div className="filters tight">
          <select value={range} onChange={(e) => setRange(e.target.value as Range)}>
            {RANGES.map((r) => (
              <option key={r.id} value={r.id}>
                {r.label}
              </option>
            ))}
          </select>
          <select value={format} onChange={(e) => setFormat(e.target.value as Format)}>
            <option value="markdown">Markdown with links</option>
            <option value="plain">Plain text</option>
          </select>
          {edited && (
            <button className="link small" onClick={() => setEdited(false)}>
              discard edits
            </button>
          )}
        </div>
        {repos.length > 1 && (
          <div className="chips">
            {repos.map((r) => (
              <button
                key={r}
                className={`chip${exclude.has(r) ? " off" : ""}`}
                title={exclude.has(r) ? "Include this repo" : "Leave this repo out"}
                onClick={() => {
                  const next = new Set(exclude);
                  if (next.has(r)) next.delete(r);
                  else next.add(r);
                  setExclude(next);
                }}
              >
                {r.split("/")[1]}
              </button>
            ))}
          </div>
        )}
        <textarea
          autoFocus
          className="composer standup-text"
          value={activity ? text : "Loading your history…"}
          onChange={(e) => {
            setText(e.target.value);
            setEdited(true);
          }}
        />
        <div className="dialog-foot">
          <span className="muted small">Only covers what Dev Inbox tracks · ⌘↵ copies</span>
          <button onClick={onClose}>Close</button>
          <button className="primary" disabled={!activity} onClick={copy}>
            Copy
          </button>
        </div>
      </div>
    </div>
  );
}
