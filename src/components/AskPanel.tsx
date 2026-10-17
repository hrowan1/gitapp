import { useEffect, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, type AskAnswer, type AskSource } from "../lib/api";
import { Icon } from "./Icon";

interface Turn {
  question: string;
  answer?: AskAnswer;
  error?: string;
}

/** "claude-haiku-5-5" → "Claude Haiku 5.5"; "apple" → Apple Intelligence. */
function engineName(by: string): string {
  if (by === "apple") return "Apple Intelligence on this Mac";
  const m = by.match(/^claude-([a-z]+)-(\d+)-(\d+)$/);
  return m ? `Claude ${m[1].charAt(0).toUpperCase()}${m[1].slice(1)} ${m[2]}.${m[3]}` : by;
}

/** Ask about your own work; answers cite what they're based on, and each citation opens it. */
export function AskPanel({
  question,
  onClose,
  onOpenTicket,
  onOpenMeeting,
}: {
  question: string;
  onClose: () => void;
  onOpenTicket: (key: string) => void;
  onOpenMeeting: (id: string, at?: number | null) => void;
}) {
  const [turns, setTurns] = useState<Turn[]>([]);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const end = useRef<HTMLDivElement>(null);

  const asked = useRef(0);
  const ask = async (q: string) => {
    const text = q.trim();
    if (!text || busy) return;
    setBusy(true);
    // Each answer goes to its own question, whatever order they come back in.
    const n = asked.current++;
    setTurns((t) => [...t, { question: text }]);
    const settle = (patch: Partial<Turn>) => setTurns((t) => t.map((x, i) => (i === n ? { ...x, ...patch } : x)));
    try {
      settle({ answer: await api.askHistory(text) });
    } catch (e) {
      settle({ error: String(e) });
    } finally {
      setBusy(false);
    }
  };

  // The opening question, once (effects can run twice in development).
  const started = useRef(false);
  useEffect(() => {
    if (started.current) return;
    started.current = true;
    void ask(question);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => {
    end.current?.scrollIntoView({ block: "end", behavior: "smooth" });
  }, [turns]);

  const open = (s: AskSource) => {
    if (s.kind === "ticket" && s.target) onOpenTicket(s.target);
    else if (s.kind === "meeting" && s.target) onOpenMeeting(s.target, s.at);
    else if (s.url) void openUrl(s.url);
  };

  return (
    <div className="overlay" onMouseDown={onClose}>
      <div className="dialog ask" onMouseDown={(e) => e.stopPropagation()} onKeyDown={(e) => e.key === "Escape" && onClose()}>
        <div className="dialog-head">
          <Icon name="sparkle" size={15} />
          <span className="strong">Ask your history</span>
          <span className="grow" />
          <button className="ghost icon-only" onClick={onClose} title="Close">
            <Icon name="x" size={14} />
          </button>
        </div>
        <div className="ask-turns">
          {turns.map((t, i) => (
            <div key={i} className="ask-turn">
              <div className="ask-question">{t.question}</div>
              {!t.answer && !t.error && (
                <div className="muted small ask-thinking">
                  <Icon name="loader" size={13} className="spin" /> Looking through your work…
                </div>
              )}
              {t.error && <div className="error small">{t.error}</div>}
              {t.answer && <AnswerView answer={t.answer} onOpen={open} />}
            </div>
          ))}
          <div ref={end} />
        </div>
        <div className="ask-input">
          <input
            autoFocus
            placeholder="Ask a follow-up — e.g. “what did Sam say about VAT?”"
            value={draft}
            disabled={busy}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                void ask(draft);
                setDraft("");
              }
            }}
          />
        </div>
      </div>
    </div>
  );
}

function AnswerView({ answer, onOpen }: { answer: AskAnswer; onOpen: (s: AskSource) => void }) {
  const byId = new Map(answer.sources.map((s) => [s.id, s]));
  const cited = [...new Set([...answer.answer.matchAll(/\[(S\d+)\]/g)].map((m) => m[1]))]
    .map((id) => byId.get(id))
    .filter((s): s is AskSource => !!s);
  const number = (id: string) => cited.findIndex((s) => s.id === id) + 1;
  return (
    <div className="ask-answer">
      <div className="ask-text">
        {answer.answer.split(/(\[S\d+\](?:\s*\[S\d+\])*)/).map((part, i) => {
          const ids = [...part.matchAll(/\[(S\d+)\]/g)].map((m) => m[1]).filter((id) => byId.has(id));
          if (!ids.length) return <span key={i}>{part}</span>;
          return (
            <span key={i}>
              {ids.map((id) => (
                <button key={id} className="cite" title={byId.get(id)!.label} onClick={() => onOpen(byId.get(id)!)}>
                  {number(id)}
                </button>
              ))}
            </span>
          );
        })}
      </div>
      {cited.length > 0 && (
        <div className="ask-sources">
          {cited.map((s, i) => (
            <button key={s.id} className="ask-source" onClick={() => onOpen(s)}>
              <span className="cite">{i + 1}</span>
              <Icon name={s.kind === "meeting" ? "mic" : s.kind === "ticket" ? "ticket" : "pr"} size={12} />
              <span className="ellipsis">{s.label}</span>
            </button>
          ))}
        </div>
      )}
      <div className="muted small ask-by">
        From {answer.period} · {engineName(answer.by)} · AI answers can be wrong — check the sources.
      </div>
    </div>
  );
}
