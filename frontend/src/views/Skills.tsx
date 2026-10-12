import { useState } from "react";
import { api, ApiError, type Skill } from "../api";
import { Empty, Err, Page, useLoader } from "./common";

type Inspection = {
  name: string;
  version: string;
  description: string;
  content_hash: string;
  instructions_preview: string;
  scripts: string[];
  scripts_will_run: boolean;
  declared_tools: {
    allowed_by_policy: { tool: string; permission: string }[];
    blocked_by_policy: { tool: string; permission: string; why: string }[];
    unknown: string[];
  };
};

/** The whole skill lifecycle without touching application source. */
export function Skills({ tick }: { tick: number }) {
  const list = useLoader<{ skills: Skill[] }>("/skills", [tick]);
  const [error, setError] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [path, setPath] = useState("examples/skills/daily-digest");
  const [inspection, setInspection] = useState<Inspection | null>(null);
  const [runOutput, setRunOutput] = useState<{ skill: string; status: string; output: string | null } | null>(null);

  const guard = async (fn: () => Promise<unknown>) => {
    setError(null);
    try {
      await fn();
      void list.reload();
    } catch (e) {
      setError(e instanceof ApiError ? e.message : String(e));
    }
  };

  return (
    <Page
      title="Skills"
      sub="Portable skill.toml + SKILL.md packages. A skill describes a workflow; it never grants itself a capability."
    >
      <Err>{error ?? list.error}</Err>

      <div className="grid">
        <div className="card">
          <strong>New from template</strong>
          <div style={{ marginTop: 8 }}>
            <label>Name (lowercase-kebab-case)</label>
            <input type="text" value={name} onChange={(e) => setName(e.target.value)} placeholder="weekly-review" />
          </div>
          <div style={{ marginTop: 8 }}>
            <label>Description</label>
            <input
              type="text"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="Summarize the week."
            />
          </div>
          <button
            className="btn primary"
            style={{ marginTop: 10 }}
            disabled={!name.trim()}
            onClick={() =>
              void guard(async () => {
                await api.post("/skills", {
                  mode: "template",
                  name: name.trim(),
                  description: description.trim() || "Describe this skill.",
                  scope: "project",
                });
                setName("");
                setDescription("");
              })
            }
          >
            Create
          </button>
          <p className="muted small">Created disabled, so you can inspect it first.</p>
        </div>

        <div className="card">
          <strong>Import a folder</strong>
          <div style={{ marginTop: 8 }}>
            <label>Path to the package directory</label>
            <input type="text" value={path} onChange={(e) => setPath(e.target.value)} />
          </div>
          <div className="row" style={{ marginTop: 10 }}>
            <button
              className="btn"
              onClick={() =>
                void guard(async () => setInspection(await api.post<Inspection>("/skills/inspect", { path })))
              }
            >
              Inspect
            </button>
            <button
              className="btn primary"
              onClick={() => void guard(() => api.post("/skills", { mode: "path", path, scope: "project" }))}
            >
              Import
            </button>
            <button className="btn" onClick={() => void guard(() => api.post("/skills/rescan"))}>
              Rescan dirs
            </button>
          </div>
        </div>
      </div>

      {inspection && (
        <div className="card">
          <div className="spread">
            <strong>
              {inspection.name} <span className="muted">v{inspection.version}</span>
            </strong>
            <button className="btn small" onClick={() => setInspection(null)}>
              Close
            </button>
          </div>
          <p className="muted small">{inspection.description}</p>
          <div className="row small">
            {inspection.declared_tools.allowed_by_policy.map((t) => (
              <span key={t.tool} className="tag ok">
                {t.tool}
              </span>
            ))}
            {inspection.declared_tools.blocked_by_policy.map((t) => (
              <span key={t.tool} className="tag bad" title={t.why}>
                {t.tool} (blocked)
              </span>
            ))}
            {inspection.declared_tools.unknown.map((t) => (
              <span key={t} className="tag warn">
                {t} (unknown)
              </span>
            ))}
          </div>
          <p className="small muted" style={{ marginTop: 8 }}>
            {inspection.scripts.length
              ? `${inspection.scripts.length} script(s) present — they will NOT be executed.`
              : "No scripts."}{" "}
            Content hash <span className="mono">{inspection.content_hash}</span>.
          </p>
          <pre style={{ maxHeight: 220, overflow: "auto" }}>{inspection.instructions_preview}</pre>
        </div>
      )}

      {runOutput && (
        <div className="card">
          <div className="spread">
            <strong>
              {runOutput.skill} · {runOutput.status}
            </strong>
            <button className="btn small" onClick={() => setRunOutput(null)}>
              Close
            </button>
          </div>
          <pre style={{ whiteSpace: "pre-wrap" }}>{runOutput.output ?? "(no output)"}</pre>
        </div>
      )}

      <h2>Installed</h2>
      {list.data?.skills.length ? (
        list.data.skills.map((s) => (
          <div key={s.id} className="card">
            <div className="spread">
              <div>
                <strong>{s.name}</strong> <span className="muted small">v{s.version}</span>{" "}
                <span className="tag">{s.scope}</span>{" "}
                <span className={`tag ${s.enabled ? "ok" : ""}`}>{s.enabled ? "enabled" : "disabled"}</span>
                <div className="muted small">{s.description}</div>
                <div className="row small" style={{ marginTop: 6 }}>
                  {s.declared_tools.map((t) => (
                    <span key={t} className="tag mono">
                      {t}
                    </span>
                  ))}
                </div>
                <div className="muted small mono" style={{ marginTop: 4 }}>
                  {s.path}
                </div>
              </div>
              <div className="row">
                <button
                  className="btn small"
                  onClick={() => void guard(() => api.put(`/skills/${s.name}/enabled`, { enabled: !s.enabled }))}
                >
                  {s.enabled ? "Disable" : "Enable"}
                </button>
                <button
                  className="btn small primary"
                  disabled={!s.enabled}
                  onClick={() =>
                    void guard(async () =>
                      setRunOutput(
                        await api.post<{ skill: string; status: string; output: string | null }>(
                          `/skills/${s.name}/run`,
                          {},
                        ),
                      ),
                    )
                  }
                >
                  Run
                </button>
                {s.scope !== "builtin" && (
                  <>
                    <button
                      className="btn small danger"
                      onClick={() => void guard(() => api.del(`/skills/${s.name}`))}
                    >
                      Remove
                    </button>
                    <button
                      className="btn small danger"
                      title="Detach and delete the package directory"
                      onClick={() =>
                        void guard(() => api.del(`/skills/${s.name}?delete_files=true`))
                      }
                    >
                      Remove + files
                    </button>
                  </>
                )}
              </div>
            </div>
          </div>
        ))
      ) : (
        <Empty>No skills installed.</Empty>
      )}
    </Page>
  );
}
