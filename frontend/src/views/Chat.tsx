import { useEffect, useRef, useState } from "react";
import { api, ApiError, type ChatReply, type Event } from "../api";
import { Err } from "./common";

type Turn = { role: "user" | "assistant"; content: string; meta?: ChatReply };

/** The primary surface: ask something, watch the tools run, read the answer
 *  with its evidence footer. */
export function Chat({ events }: { events: Event[] }) {
  const [turns, setTurns] = useState<Turn[]>([]);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const conversation = useRef<string | null>(null);
  const bottom = useRef<HTMLDivElement>(null);

  useEffect(() => bottom.current?.scrollIntoView({ behavior: "smooth" }), [turns, busy]);

  const send = async () => {
    const message = draft.trim();
    if (!message || busy) return;
    setTurns((t) => [...t, { role: "user", content: message }]);
    setDraft("");
    setBusy(true);
    setError(null);
    try {
      const reply = await api.post<ChatReply>("/chat", {
        message,
        conversation_id: conversation.current,
      });
      conversation.current = reply.conversation_id;
      setTurns((t) => [...t, { role: "assistant", content: reply.reply, meta: reply }]);
    } catch (e) {
      setError(e instanceof ApiError ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  // While a run is in flight, show the live tool activity instead of a spinner.
  const activity = events
    .filter((e) => e.type === "tool_call" || e.type === "agent_update")
    .slice(-8);

  return (
    <>
      <div className="spread">
        <div>
          <h1>Chat</h1>
          <p className="sub">
            Every answer ends with the tools it actually used. A failed tool is shown, not hidden.
          </p>
        </div>
      </div>

      <Err>{error}</Err>

      <div className="chat">
        {turns.length === 0 && (
          <div className="empty">
            Try: <em>“what can you do offline right now?”</em> ·{" "}
            <em>“remember that my hackathon deadline is 2026-10-30”</em> ·{" "}
            <em>“give me today’s briefing”</em>
          </div>
        )}
        {turns.map((t, i) => (
          <div key={i} className={`msg ${t.role}`}>
            <div className="who">{t.role === "user" ? "you" : "friday"}</div>
            {t.content}
            {t.meta && (
              <div className="row small muted" style={{ marginTop: 8 }}>
                <span className="tag accent">{t.meta.model}</span>
                <span className={`tag ${t.meta.status === "completed" ? "ok" : "bad"}`}>{t.meta.status}</span>
                <span>
                  {t.meta.iterations} turn{t.meta.iterations === 1 ? "" : "s"} · {t.meta.tool_calls} tool
                  call{t.meta.tool_calls === 1 ? "" : "s"}
                  {t.meta.tokens.prompt + t.meta.tokens.completion > 0 &&
                    ` · ${t.meta.tokens.prompt + t.meta.tokens.completion} tokens`}
                </span>
              </div>
            )}
          </div>
        ))}
        {busy && (
          <div className="msg assistant">
            <div className="who">working</div>
            {activity.length === 0 ? (
              <span className="muted">thinking…</span>
            ) : (
              <div className="feed">
                {activity.map((e, i) => (
                  <div key={i}>
                    {e.type === "tool_call"
                      ? `${e.ok ? "✓" : "✗"} ${e.tool}`
                      : `· ${e.role} ${e.status}${e.detail ? `: ${e.detail}` : ""}`}
                  </div>
                ))}
              </div>
            )}
          </div>
        )}
        <div ref={bottom} />
      </div>

      <div className="composer">
        <textarea
          value={draft}
          placeholder="Ask FRIDAY…  (Enter to send, Shift+Enter for a new line)"
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              void send();
            }
          }}
        />
        <button className="btn primary" onClick={() => void send()} disabled={busy || !draft.trim()}>
          {busy ? "…" : "Send"}
        </button>
      </div>
    </>
  );
}
