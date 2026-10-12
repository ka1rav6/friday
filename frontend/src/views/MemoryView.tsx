import { useState } from "react";
import { api, type Memory } from "../api";
import { Empty, Err, Page, useLoader } from "./common";

const CATEGORIES = ["profile", "project", "task", "knowledge", "episodic"] as const;

/** Memory is inspectable, editable and deletable — including its provenance,
 *  so a model inference is never mistaken for something the user said. */
export function MemoryView({ tick }: { tick: number }) {
  const [query, setQuery] = useState("");
  const list = useLoader<{ memories: Memory[] }>(
    query ? `/memories?q=${encodeURIComponent(query)}` : "/memories",
    [tick, query],
  );
  const conflicts = useLoader<{ conflicts: { topic: string; a: Memory; b: Memory }[] }>(
    "/memories/conflicts",
    [tick],
  );

  const [content, setContent] = useState("");
  const [category, setCategory] = useState<string>("profile");
  const [editing, setEditing] = useState<{ id: string; content: string } | null>(null);

  const add = async () => {
    if (!content.trim()) return;
    await api.post("/memories", {
      content: content.trim(),
      category,
      source: "entered in the Memory view",
      provenance: "user_stated",
    });
    setContent("");
    void list.reload();
  };

  return (
    <Page title="Memory" sub="What FRIDAY remembers, where each item came from, and how sure it is.">
      <Err>{list.error}</Err>

      <div className="card">
        <label>Remember something</label>
        <textarea
          rows={2}
          value={content}
          placeholder="e.g. I prefer Rust over Python for systems work"
          onChange={(e) => setContent(e.target.value)}
        />
        <div className="row" style={{ marginTop: 8 }}>
          <select value={category} onChange={(e) => setCategory(e.target.value)} style={{ width: 160 }}>
            {CATEGORIES.map((c) => (
              <option key={c} value={c}>
                {c}
              </option>
            ))}
          </select>
          <button className="btn primary" onClick={() => void add()} disabled={!content.trim()}>
            Save
          </button>
          <span className="muted small">Saved from the UI counts as user_stated.</span>
        </div>
      </div>

      {conflicts.data?.conflicts.length ? (
        <div className="card">
          <strong>Possible contradictions</strong>
          <p className="muted small">
            FRIDAY will not resolve these on its own. Delete whichever is wrong.
          </p>
          {conflicts.data.conflicts.map((c, i) => (
            <div key={i} className="small" style={{ marginTop: 8 }}>
              <span className="tag warn">{c.topic}</span>
              <div>A: {c.a.content}</div>
              <div>B: {c.b.content}</div>
            </div>
          ))}
        </div>
      ) : null}

      <div className="row" style={{ marginBottom: 12 }}>
        <input
          type="text"
          placeholder="Search memories…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
      </div>

      {list.data?.memories.length ? (
        list.data.memories.map((m) => (
          <div key={m.id} className="card">
            <div className="spread">
              <div style={{ flex: 1 }}>
                {editing?.id === m.id ? (
                  <textarea
                    rows={2}
                    value={editing.content}
                    onChange={(e) => setEditing({ id: m.id, content: e.target.value })}
                  />
                ) : (
                  <div>{m.content}</div>
                )}
                <div className="row small" style={{ marginTop: 6 }}>
                  <span className="tag accent">{m.category}</span>
                  <span className={`tag ${m.provenance === "model_inference" ? "warn" : ""}`}>
                    {m.provenance}
                  </span>
                  {!m.approved && <span className="tag warn">awaiting approval</span>}
                  <span className="muted">
                    confidence {m.confidence.toFixed(2)} · {m.source} · {m.created_at.slice(0, 10)}
                  </span>
                </div>
              </div>
              <div className="row">
                {!m.approved && (
                  <button
                    className="btn small"
                    onClick={async () => {
                      await api.post(`/memories/${m.id}/approve`);
                      void list.reload();
                    }}
                  >
                    Approve
                  </button>
                )}
                {editing?.id === m.id ? (
                  <button
                    className="btn small primary"
                    onClick={async () => {
                      await api.put(`/memories/${m.id}`, { content: editing.content });
                      setEditing(null);
                      void list.reload();
                    }}
                  >
                    Save
                  </button>
                ) : (
                  <button className="btn small" onClick={() => setEditing({ id: m.id, content: m.content })}>
                    Edit
                  </button>
                )}
                <button
                  className="btn small danger"
                  onClick={async () => {
                    await api.del(`/memories/${m.id}`);
                    void list.reload();
                  }}
                >
                  Delete
                </button>
              </div>
            </div>
          </div>
        ))
      ) : (
        <Empty>{query ? "Nothing matched." : "No memories yet."}</Empty>
      )}
    </Page>
  );
}
