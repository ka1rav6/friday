import { useState } from "react";
import { api, type Job, type ResearchDoc } from "../api";
import { Empty, Err, Page, ago, statusTag, useLoader } from "./common";

type Briefing = { for_date?: string; content?: string; created_at?: string; note?: string };

/** Briefing plus the research library — the two places saved knowledge shows up. */
export function Knowledge({ tick }: { tick: number }) {
  const briefing = useLoader<Briefing>("/briefing", [tick]);
  const jobs = useLoader<{ jobs: Job[]; recovery: { missed: { job: string; cause: string }[] } }>(
    "/jobs",
    [tick],
  );
  const [query, setQuery] = useState("");
  const research = useLoader<{ research: ResearchDoc[]; stale: ResearchDoc[] }>(
    query ? `/research?q=${encodeURIComponent(query)}` : "/research",
    [tick, query],
  );
  const [running, setRunning] = useState(false);
  const [open, setOpen] = useState<string | null>(null);
  const report = useLoader<{
    title: string;
    question: string;
    content: string;
    sources: { url: string; title: string; published: string | null; fetched_at: string }[];
  }>(open ? `/research/${open}` : "/health", [open]);

  const runBriefing = async () => {
    setRunning(true);
    try {
      await api.post("/jobs/morning-briefing/run?force=true");
      void briefing.reload();
      void jobs.reload();
    } finally {
      setRunning(false);
    }
  };

  return (
    <Page
      title="Knowledge"
      sub="Today's briefing and the research library. Everything here works with the network off."
      actions={
        <button className="btn primary" onClick={() => void runBriefing()} disabled={running}>
          {running ? "Running…" : "Run briefing now"}
        </button>
      }
    >
      <Err>{briefing.error ?? research.error}</Err>

      <div className="card">
        <div className="spread">
          <strong>Morning briefing {briefing.data?.for_date ? `· ${briefing.data.for_date}` : ""}</strong>
          {briefing.data?.created_at && <span className="muted small">{ago(briefing.data.created_at)}</span>}
        </div>
        {briefing.data?.content ? (
          <pre style={{ whiteSpace: "pre-wrap" }}>{briefing.data.content}</pre>
        ) : (
          <p className="muted small">{briefing.data?.note ?? "No briefing saved yet."}</p>
        )}
      </div>

      {jobs.data?.jobs.length ? (
        <div className="card">
          <strong>Scheduled jobs</strong>
          <p className="muted small">
            The scheduler is local: it only fires while FRIDAY is running, so it cannot deliver a
            briefing while the machine is asleep.
          </p>
          <table>
            <thead>
              <tr>
                <th>Job</th>
                <th>Skill</th>
                <th>At</th>
                <th>Next</th>
                <th>Last</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {jobs.data.jobs.map((j) => (
                <tr key={j.id}>
                  <td>{j.name}</td>
                  <td className="mono small">{j.skill}</td>
                  <td className="small">
                    {j.at_time} <span className="muted">{j.timezone}</span>
                  </td>
                  <td className="small muted">{j.next_run_at?.slice(0, 16).replace("T", " ") ?? "—"}</td>
                  <td className="small">
                    {j.last_status ? statusTag(j.last_status) : <span className="muted">never</span>}
                    {j.last_error && <div className="small" style={{ color: "var(--bad)" }}>{j.last_error}</div>}
                  </td>
                  <td>
                    <button
                      className="btn small"
                      onClick={async () => {
                        await api.post(`/jobs/${j.name}/run?force=true`);
                        void jobs.reload();
                        void briefing.reload();
                      }}
                    >
                      Run now
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {jobs.data.recovery.missed.length > 0 && (
            <p className="small" style={{ color: "var(--warn)" }}>
              Missed: {jobs.data.recovery.missed.map((m) => `${m.job} (${m.cause})`).join("; ")}
            </p>
          )}
        </div>
      ) : null}

      <h2>Research library</h2>
      <div className="row" style={{ marginBottom: 12 }}>
        <input
          type="text"
          placeholder="Search saved research…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
      </div>

      {research.data?.research.length ? (
        research.data.research.map((r) => (
          <div key={r.id} className="card">
            <div className="spread">
              <div>
                <strong>{r.title}</strong>
                <div className="muted small">{r.summary}</div>
                <div className="row small" style={{ marginTop: 6 }}>
                  <span className="tag">{r.created_at.slice(0, 10)}</span>
                  {r.age_days !== undefined && <span className="tag">{r.age_days}d old</span>}
                  {r.stale && <span className="tag warn">past freshness policy</span>}
                  {r.sources && <span className="muted">{r.sources.length} sources</span>}
                </div>
              </div>
              <button className="btn small" onClick={() => setOpen(open === r.id ? null : r.id)}>
                {open === r.id ? "Hide" : "Open"}
              </button>
            </div>
            {open === r.id && report.data && (
              <div style={{ marginTop: 10 }}>
                {report.data.question && <p className="muted small">Question: {report.data.question}</p>}
                <pre style={{ whiteSpace: "pre-wrap" }}>{report.data.content}</pre>
                <h2>Sources</h2>
                <ul className="small">
                  {report.data.sources.map((s, i) => (
                    <li key={i}>
                      <a href={s.url} target="_blank" rel="noreferrer noopener">
                        {s.title || s.url}
                      </a>{" "}
                      <span className="muted">
                        {s.published ? `published ${s.published} · ` : ""}fetched {s.fetched_at.slice(0, 10)}
                      </span>
                    </li>
                  ))}
                </ul>
              </div>
            )}
          </div>
        ))
      ) : (
        <Empty>
          {query ? "Nothing matched." : "No saved research yet. Ask FRIDAY to research a topic."}
        </Empty>
      )}
    </Page>
  );
}
