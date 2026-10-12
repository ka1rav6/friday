import { useEffect, useState } from "react";
import { api, ApiError, type Approval, type AuditEntry, type ToolDef } from "../api";
import { Empty, Err, Page, ago, statusTag, useLoader } from "./common";

type ConfigEnvelope = {
  config: Record<string, unknown>;
  sources: {
    precedence: string[];
    user_config: string | null;
    project_config: string | null;
    env_overrides: string[];
    user_config_path: string;
    project_config_path: string;
  };
  restart_required_for: string[];
};

/** Settings, pending approvals, the tool catalogue and the audit log — the
 *  places where the user stays in control of what FRIDAY may do. */
export function Settings({ tick }: { tick: number }) {
  const envelope = useLoader<ConfigEnvelope>("/config", [tick]);
  const approvals = useLoader<{ approvals: Approval[] }>("/approvals", [tick]);
  const tools = useLoader<{ tools: ToolDef[] }>("/tools", []);
  const audit = useLoader<{ entries: AuditEntry[] }>("/audit?limit=60", [tick]);

  const [draft, setDraft] = useState("");
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (envelope.data) setDraft(JSON.stringify(envelope.data.config, null, 2));
  }, [envelope.data]);

  const apply = async (validateOnly: boolean) => {
    setError(null);
    setNote(null);
    let parsed: unknown;
    try {
      parsed = JSON.parse(draft);
    } catch (e) {
      setError(`Not valid JSON: ${String(e)}`);
      return;
    }
    try {
      if (validateOnly) {
        const r = await api.post<{ valid: boolean; error?: string }>("/config/validate", { config: parsed });
        r.valid ? setNote("Valid.") : setError(r.error ?? "invalid");
        return;
      }
      const r = await api.put<{ written_to: string; restart_required: boolean; restart_note: string }>(
        "/config",
        { config: parsed },
      );
      setNote(`Saved to ${r.written_to}. ${r.restart_note}`);
      void envelope.reload();
    } catch (e) {
      // A rejected edit leaves the live configuration untouched.
      setError(e instanceof ApiError ? e.message : String(e));
    }
  };

  const src = envelope.data?.sources;
  const byPermission = (tools.data?.tools ?? []).reduce<Record<string, number>>((acc, t) => {
    acc[t.permission] = (acc[t.permission] ?? 0) + 1;
    return acc;
  }, {});

  return (
    <Page title="Settings" sub="Configuration, approvals, capabilities and the audit trail.">
      <Err>{error}</Err>
      {note && <div className="card small">{note}</div>}

      {approvals.data?.approvals.some((a) => a.status === "pending") && (
        <div className="card">
          <strong>Approval required</strong>
          <p className="muted small">
            FRIDAY paused and is waiting for you. Nothing happens until you decide.
          </p>
          {approvals.data.approvals
            .filter((a) => a.status === "pending")
            .map((a) => (
              <div key={a.id} className="spread" style={{ marginTop: 10 }}>
                <div>
                  <div>
                    <span className={`tag ${a.risk === "high" ? "bad" : "warn"}`}>{a.risk} risk</span>{" "}
                    <strong>{a.action}</strong>
                  </div>
                  <div className="muted small">{a.reason}</div>
                  {a.preview && <pre className="small">{a.preview}</pre>}
                </div>
                <div className="row">
                  <button
                    className="btn small primary"
                    onClick={async () => {
                      await api.post(`/approvals/${a.id}/decide`, { decision: "approved" });
                      void approvals.reload();
                    }}
                  >
                    Approve
                  </button>
                  <button
                    className="btn small danger"
                    onClick={async () => {
                      await api.post(`/approvals/${a.id}/decide`, { decision: "denied" });
                      void approvals.reload();
                    }}
                  >
                    Deny
                  </button>
                </div>
              </div>
            ))}
        </div>
      )}

      {src && (
        <div className="card small">
          <strong>Where settings come from</strong>
          <p className="muted" style={{ margin: "4px 0" }}>
            {src.precedence.join("  <  ")}
          </p>
          <div>
            User file: <span className="mono">{src.user_config ?? `${src.user_config_path} (absent)`}</span>
          </div>
          <div>
            Project file:{" "}
            <span className="mono">{src.project_config ?? `${src.project_config_path} (absent)`}</span>
          </div>
          <div>
            Environment overrides:{" "}
            <span className="mono">{src.env_overrides.length ? src.env_overrides.join(", ") : "none"}</span>
          </div>
          <div className="muted" style={{ marginTop: 4 }}>
            Restart needed only for: {envelope.data?.restart_required_for.join(", ")}. Secrets are never
            shown here — they live in the environment.
          </div>
        </div>
      )}

      <div className="card">
        <strong>Effective configuration</strong>
        <p className="muted small">
          Edit and apply. An invalid edit is rejected and the running configuration is kept.
        </p>
        <textarea rows={18} className="mono" value={draft} onChange={(e) => setDraft(e.target.value)} />
        <div className="row" style={{ marginTop: 10 }}>
          <button className="btn" onClick={() => void apply(true)}>
            Validate
          </button>
          <button className="btn primary" onClick={() => void apply(false)}>
            Apply
          </button>
          <button className="btn" onClick={() => void envelope.reload()}>
            Revert
          </button>
        </div>
      </div>

      <h2>Capabilities</h2>
      <div className="card">
        <div className="row small">
          {Object.entries(byPermission).map(([p, n]) => (
            <span key={p} className="tag">
              {p}: {n}
            </span>
          ))}
          <span className="muted">{tools.data?.tools.length ?? 0} tools registered</span>
        </div>
        <p className="muted small" style={{ marginTop: 6 }}>
          A permission category is not an authorization — every call is also checked against the caller's
          grant, the workspace scope and the connectivity mode.
        </p>
      </div>

      <h2>Audit log</h2>
      {audit.data?.entries.length ? (
        <table>
          <thead>
            <tr>
              <th>When</th>
              <th>Actor</th>
              <th>Category</th>
              <th>Action</th>
              <th>Detail</th>
            </tr>
          </thead>
          <tbody>
            {audit.data.entries.map((e) => (
              <tr key={e.id}>
                <td className="muted small">{ago(e.created_at)}</td>
                <td className="small mono">{e.actor.slice(0, 8)}</td>
                <td className="small">{e.category}</td>
                <td className="small mono">{e.action}</td>
                <td className="small muted">{e.detail}</td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        <Empty>Nothing audited yet.</Empty>
      )}

      {approvals.data?.approvals.some((a) => a.status !== "pending") && (
        <>
          <h2>Past decisions</h2>
          {approvals.data.approvals
            .filter((a) => a.status !== "pending")
            .map((a) => (
              <div key={a.id} className="row small" style={{ padding: "3px 0" }}>
                {statusTag(a.status)} <span>{a.action}</span>
                <span className="muted">{ago(a.created_at)}</span>
              </div>
            ))}
        </>
      )}
    </Page>
  );
}
