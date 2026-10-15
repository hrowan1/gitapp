import { useEffect, useMemo, useRef, useState } from "react";
import { api, type FollowItem, type Kind } from "../lib/api";
import { shortRef } from "../lib/states";
import { StateIcon } from "./StateIcon";
import { toast } from "../lib/toast";

/** Choose exactly which PRs, issues and branches you listen to. Everything is followed by default. */
export function Following({ version }: { version: number }) {
  const [rows, setRows] = useState<FollowItem[] | null>(null);
  const [text, setText] = useState("");
  const [kind, setKind] = useState<"" | Kind>("");
  const [show, setShow] = useState<"all" | "followed" | "unfollowed">("all");
  const [mineOnly, setMineOnly] = useState(false);
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});

  const load = () => api.getFollowing().then(setRows).catch((e) => toast(String(e), true));
  useEffect(() => {
    load();
  }, [version]);

  const shown = useMemo(() => {
    const q = text.toLowerCase().trim();
    return (rows ?? []).filter(
      (r) =>
        (!kind || r.kind === kind) &&
        (show === "all" || (show === "followed") === r.followed) &&
        (!mineOnly || r.isMine) &&
        (!q || `${r.title} ${r.repo} ${r.author} #${r.number}`.toLowerCase().includes(q)),
    );
  }, [rows, text, kind, show, mineOnly]);

  const byRepo = useMemo(() => {
    const m = new Map<string, FollowItem[]>();
    for (const r of shown) m.set(r.repo, [...(m.get(r.repo) ?? []), r]);
    return [...m.entries()];
  }, [shown]);

  const set = async (items: FollowItem[], followed: boolean) => {
    const ids = new Set(items.filter((i) => i.followed !== followed).map((i) => i.id));
    if (!ids.size) return;
    setRows((rs) => rs?.map((r) => (ids.has(r.id) ? { ...r, followed } : r)) ?? rs);
    try {
      await api.setFollowed([...ids], followed);
    } catch (e) {
      toast(String(e), true);
      load();
    }
  };

  const hideRepo = async (repo: string) => {
    await api.setRepoHidden(repo, true);
    toast(`Hid ${repo} — including future items. Undo in Settings → Repositories.`);
    load();
  };

  const total = rows?.length ?? 0;
  const following = rows?.filter((r) => r.followed).length ?? 0;

  return (
    <div className="page following">
      <div className="headline compact">
        <h1>
          Following {following} of {total}
        </h1>
        <p className="muted">
          Untick anything you don't want to hear about. Unfollowed items disappear from Today, counts and
          notifications. New PRs and issues are followed automatically.
        </p>
      </div>
      <div className="filters">
        <input className="search" placeholder="Filter…" value={text} onChange={(e) => setText(e.target.value)} />
        <select value={kind} onChange={(e) => setKind(e.target.value as typeof kind)}>
          <option value="">Everything</option>
          <option value="pr">Pull requests</option>
          <option value="issue">Issues</option>
          <option value="run">Branches</option>
        </select>
        <select value={show} onChange={(e) => setShow(e.target.value as typeof show)}>
          <option value="all">Followed or not</option>
          <option value="followed">Followed</option>
          <option value="unfollowed">Not followed</option>
        </select>
        <label className="check">
          <input type="checkbox" checked={mineOnly} onChange={(e) => setMineOnly(e.target.checked)} /> Mine only
        </label>
      </div>
      <div className="bulk muted small">
        {shown.length} shown ·{" "}
        <button className="link" onClick={() => set(shown, true)}>
          follow all shown
        </button>{" "}
        ·{" "}
        <button className="link" onClick={() => set(shown, false)}>
          unfollow all shown
        </button>
      </div>

      {rows && byRepo.length === 0 && <div className="empty big">Nothing matches.</div>}
      {byRepo.map(([repo, items]) => {
        const on = items.filter((i) => i.followed).length;
        const closed = collapsed[repo] ?? false;
        return (
          <section key={repo} className="section follow-repo">
            <div className="follow-head">
              <button className={`chevron-btn`} onClick={() => setCollapsed({ ...collapsed, [repo]: !closed })}>
                <span className={`chevron${closed ? " closed" : ""}`}>▾</span>
              </button>
              <TriCheck
                checked={on === items.length}
                indeterminate={on > 0 && on < items.length}
                onChange={(v) => set(items, v)}
              />
              <span className="strong grow">{repo}</span>
              <span className="muted small">
                {on}/{items.length} followed
              </span>
              <button className="link small muted" onClick={() => hideRepo(repo)} title="Hide this repo and anything new in it">
                hide repo
              </button>
            </div>
            {!closed &&
              items.map((r) => (
                <label key={r.id} className={`follow-row${r.followed ? "" : " off"}`}>
                  <input type="checkbox" checked={r.followed} onChange={(e) => set([r], e.target.checked)} />
                  <StateIcon state={r.state} size={13} />
                  {r.kind !== "pr" && <span className={`kind kind-${r.kind}`}>{r.kind === "issue" ? "Issue" : "Branch"}</span>}
                  <span className="grow ellipsis">
                    {r.title} <span className="muted">· {shortRef(r)} · {r.isMine ? "you" : r.author}</span>
                  </span>
                </label>
              ))}
          </section>
        );
      })}
    </div>
  );
}

function TriCheck({
  checked,
  indeterminate,
  onChange,
}: {
  checked: boolean;
  indeterminate: boolean;
  onChange: (v: boolean) => void;
}) {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.indeterminate = indeterminate;
  }, [indeterminate]);
  return <input ref={ref} type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />;
}
