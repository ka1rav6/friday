import { useState } from "react";
import { api, type AgentRun, type Task, type TaskDetail } from "../api";
import { Empty, Err, Page, ago, statusTag, useLoader } from "./common";

/** Tasks, their subagents, and the evidence behind each result. */
export function Activity({ tick }: { tick: number }) {
  const tasks = useLoader<{ tasks: Task[] }>("/tasks?limit=30", [tick]);
  const agents = useLoader<{
    limits: { max_concurrent_subagents: number; hard_max: number; max_depth: number };
    available_permits: number;
    agents: AgentRun[];
  }>("/agents?limit=30", [tick]);

  const [open, setOpen] = useState<string | null>(null);
  const detail = useLoader<TaskDetail>(open ? `/tasks/${open}` : "/health", [open]);

  const limits = agents.data?.limits;

  return (
    <Page
      title="Activity"
      sub="Parent tasks, the specialist subagents under them, and what each one actually did."
      actions={
        <button className="btn" onClick={() => void (tasks.reload(), agents.reload())}>
          Refresh
        </button>
      }
    >
      <Err>{tasks.error ?? agents.error}</Err>

      {limits && (
        <div className="card row small">
          <span className="tag accent">
            {limits.max_concurrent_subagents} concurrent / {limits.hard_max} max
          </span>
          <span className="tag">depth ≤ {limits.max_depth}</span>
          <span className="muted">{agents.data?.available_permits} permits free</span>
          <span className="muted">
            Subagents cannot nest and cannot widen their own tool grant.
          </span>
        </div>
      )}

      <h2>Tasks</h2>
      {tasks.data?.tasks.length ? (
        <table>
          <thead>
            <tr>
              <th>Task</th>
              <th>Status</th>
              <th>Due</th>
              <th>Created</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {tasks.data.tasks.map((t) => (
              <tr key={t.id}>
                <td>{t.title}</td>
                <td>
                  {statusTag(t.status)} {t.overdue && <span className="tag bad">overdue</span>}
                </td>
                <td className="muted small">{t.due_at?.slice(0, 10) ?? "—"}</td>
                <td className="muted small">{ago(t.created_at)}</td>
                <td className="row">
                  <button className="btn small" onClick={() => setOpen(open === t.id ? null : t.id)}>
                    {open === t.id ? "Hide" : "Evidence"}
                  </button>
                  {(t.status === "running" || t.status === "pending") && (
                    <button
                      className="btn small danger"
                      onClick={async () => {
                        await api.post(`/tasks/${t.id}/cancel`);
                        void tasks.reload();
                      }}
                    >
                      Cancel
                    </button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        <Empty>No tasks yet.</Empty>
      )}

      {open && detail.data?.task && (
        <div className="card" style={{ marginTop: 12 }}>
          <div className="spread">
            <strong>{detail.data.task.title}</strong>
            <span className={`tag ${detail.data.evidence.verified ? "ok" : "warn"}`}>
              {detail.data.evidence.verified ? "evidence verified" : "not verified"}
            </span>
          </div>

          {detail.data.evidence.missing.length > 0 && (
            <ul className="small" style={{ color: "var(--warn)" }}>
              {detail.data.evidence.missing.map((m, i) => (
                <li key={i}>{m}</li>
              ))}
            </ul>
          )}

          {detail.data.subtasks.length > 0 && (
            <>
              <h2>Subagents</h2>
              {detail.data.subtasks.map((s) => (
                <div key={s.id} className="row small" style={{ padding: "3px 0" }}>
                  <span className="tag accent">{s.role ?? "—"}</span>
                  {statusTag(s.status)}
                  <span className="muted">{s.title}</span>
                </div>
              ))}
            </>
          )}

          <h2>Tool calls</h2>
          {detail.data.tool_calls.length ? (
            <table>
              <thead>
                <tr>
                  <th>Tool</th>
                  <th>Permission</th>
                  <th>Result</th>
                  <th>Duration</th>
                </tr>
              </thead>
              <tbody>
                {detail.data.tool_calls.map((c, i) => (
                  <tr key={i}>
                    <td className="mono">{c.tool}</td>
                    <td className="small muted">{c.permission}</td>
                    <td>
                      <span className={`tag ${c.ok ? "ok" : "bad"}`}>{c.ok ? "ok" : "failed"}</span>
                    </td>
                    <td className="small muted">{c.duration_ms} ms</td>
                  </tr>
                ))}
              </tbody>
            </table>
          ) : (
            <p className="muted small">No tools were called, so nothing about this task is verified.</p>
          )}
        </div>
      )}

      <h2>Agent runs</h2>
      {agents.data?.agents.length ? (
        <table>
          <thead>
            <tr>
              <th>Role</th>
              <th>Status</th>
              <th>Turns</th>
              <th>Tools</th>
              <th>Started</th>
              <th>Parent</th>
            </tr>
          </thead>
          <tbody>
            {agents.data.agents.map((a) => (
              <tr key={a.run_id}>
                <td>
                  <span className="tag accent">{a.role}</span>
                </td>
                <td>{statusTag(a.status)}</td>
                <td>{a.iterations}</td>
                <td>{a.tool_calls}</td>
                <td className="muted small">{ago(a.started_at)}</td>
                <td className="muted small">{a.parent_task_id ? "subagent" : "top level"}</td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        <Empty>No agent runs yet.</Empty>
      )}
    </Page>
  );
}
