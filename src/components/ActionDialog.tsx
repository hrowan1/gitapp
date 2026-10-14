import { useEffect, useRef, useState } from "react";
import { api, type MergeOptions } from "../lib/api";
import type { DialogRequest } from "../lib/dialog";
import { shortRef } from "../lib/states";
import { toast } from "../lib/toast";

const METHOD_LABEL: Record<string, string> = {
  SQUASH: "Squash and merge",
  MERGE: "Create a merge commit",
  REBASE: "Rebase and merge",
};

const TITLES = {
  reply: "Reply",
  approve: "Approve",
  request_changes: "Request changes",
  review_comment: "Comment",
  merge: "Merge",
};

/** Confirm-and-write dialog for actions that act on GitHub as the user. */
export function ActionDialog({ request, onClose }: { request: DialogRequest; onClose: () => void }) {
  const { kind, item } = request;
  const [body, setBody] = useState("");
  const [busy, setBusy] = useState(false);
  const [merge, setMerge] = useState<MergeOptions | null>(null);
  const [method, setMethod] = useState("");
  const [loadError, setLoadError] = useState<string | null>(null);
  const textRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    if (kind !== "merge") return;
    api
      .mergeOptions(item.id)
      .then((m) => {
        setMerge(m);
        setMethod(m.methods.includes(m.default as never) ? m.default : (m.methods[0] ?? "MERGE"));
      })
      .catch((e) => setLoadError(String(e)));
  }, [kind, item.id]);

  const needsBody = kind === "reply" || kind === "request_changes" || kind === "review_comment";
  const ready = !busy && (!needsBody || body.trim().length > 0) && (kind !== "merge" || (merge && method));
  const ctx = item.context;

  const submit = async () => {
    if (!ready) return;
    setBusy(true);
    try {
      let msg: string;
      switch (kind) {
        case "reply":
          msg = await api.sendComment(item.id, body, ctx?.threadId);
          break;
        case "approve":
          msg = await api.submitReview(item.id, "APPROVE", body);
          break;
        case "request_changes":
          msg = await api.submitReview(item.id, "REQUEST_CHANGES", body);
          break;
        case "review_comment":
          msg = await api.submitReview(item.id, "COMMENT", body);
          break;
        case "merge":
          msg = await api.mergePr(item.id, method);
          break;
      }
      toast(`${msg} · ${shortRef(item)}`);
      onClose();
    } catch (e) {
      toast(String(e), true);
      setBusy(false);
    }
  };

  const blocked = merge && (merge.mergeable === "CONFLICTING" || ["BLOCKED", "DIRTY", "BEHIND"].includes(merge.state));
  const verb =
    kind === "reply"
      ? ctx?.threadId
        ? "Reply in thread"
        : "Comment"
      : kind === "approve"
        ? "Approve"
        : kind === "request_changes"
          ? "Request changes"
          : kind === "review_comment"
            ? "Comment"
            : "Merge";

  return (
    <div className="overlay" onMouseDown={onClose}>
      <div
        className="dialog"
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Escape") onClose();
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) submit();
        }}
      >
        <div className="dialog-head">
          <span className="strong">{TITLES[kind]}</span>
          <span className="muted ellipsis">
            {shortRef(item)} · {item.title}
          </span>
        </div>

        {kind === "reply" && ctx && (
          <blockquote className="quote">
            <span className="strong">{ctx.who}</span> {ctx.body}
          </blockquote>
        )}

        {kind === "merge" ? (
          <div className="merge-options">
            {loadError && <p className="error">{loadError}</p>}
            {!merge && !loadError && <p className="muted">Checking what this repo allows…</p>}
            {merge?.methods.map((m) => (
              <label key={m} className="check">
                <input type="radio" name="method" checked={method === m} onChange={() => setMethod(m)} />
                {METHOD_LABEL[m] ?? m}
              </label>
            ))}
            {blocked && (
              <p className="warn">
                GitHub reports this PR as {merge.mergeable === "CONFLICTING" ? "having conflicts" : merge.state.toLowerCase()}
                ; the merge will probably be refused.
              </p>
            )}
          </div>
        ) : (
          <textarea
            ref={textRef}
            autoFocus
            className="composer"
            rows={kind === "approve" ? 3 : 5}
            placeholder={
              kind === "approve"
                ? "Optional comment"
                : kind === "request_changes"
                  ? "What needs to change?"
                  : "Write a reply… (markdown works)"
            }
            value={body}
            onChange={(e) => setBody(e.target.value)}
          />
        )}

        <div className="dialog-foot">
          <span className="muted small">Posts to GitHub as you · ⌘↵</span>
          <button onClick={onClose}>Cancel</button>
          <button
            className={kind === "request_changes" ? "danger" : "primary"}
            disabled={!ready}
            autoFocus={kind === "merge"}
            onClick={submit}
          >
            {busy ? "Sending…" : verb}
          </button>
        </div>
      </div>
    </div>
  );
}
