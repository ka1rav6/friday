import { useEffect, useMemo, useState } from "react";
import { subscribe, type Event, type Status } from "./api";
import { Activity } from "./views/Activity";
import { Chat } from "./views/Chat";
import { Knowledge } from "./views/Knowledge";
import { MemoryView } from "./views/MemoryView";
import { Settings } from "./views/Settings";
import { Skills } from "./views/Skills";
import { useLoader } from "./views/common";

const TABS = ["Chat", "Activity", "Memory", "Knowledge", "Skills", "Settings"] as const;
type Tab = (typeof TABS)[number];

export function App() {
  const [tab, setTab] = useState<Tab>("Chat");
  const [events, setEvents] = useState<Event[]>([]);
  // Bumped on every backend event so the open view refetches rather than
  // polling on a timer.
  const [tick, setTick] = useState(0);
  const status = useLoader<Status>("/status", [tick === 0]);

  useEffect(
    () =>
      subscribe((e) => {
        setEvents((prev) => [...prev.slice(-200), e]);
        if (e.type !== "tool_call") setTick((t) => t + 1);
      }),
    [],
  );

  const pendingApprovals = useMemo(
    () => events.filter((e) => e.type === "approval_required").length,
    [events],
  );
  const notifications = useMemo(
    () => events.filter((e) => e.type === "notification").slice(-3),
    [events],
  );

  const model = status.data?.selected_model ?? "no model";
  const online = status.data?.hosted_nvidia_model_reachable ?? false;

  return (
    <div className="app">
      <nav className="side">
        <div className="brand">
          FRIDAY
          <small>personal AI OS</small>
        </div>
        {TABS.map((t) => (
          <button key={t} className="nav" aria-current={tab === t} onClick={() => setTab(t)}>
            {t}
            {t === "Settings" && pendingApprovals > 0 && <span className="count">{pendingApprovals}</span>}
            {t === "Memory" && status.data?.counts.memories ? (
              <span className="count">{status.data.counts.memories}</span>
            ) : null}
          </button>
        ))}

        <div className="side-status">
          <div>
            <span className={`tag ${online ? "ok" : "warn"}`}>{status.data?.mode ?? "…"}</span>
          </div>
          <div className="mono" style={{ marginTop: 6, wordBreak: "break-all" }}>
            {model}
          </div>
          <div style={{ marginTop: 4 }}>
            {online
              ? "hosted NVIDIA model reachable"
              : status.data?.hosted_credential_present
                ? "hosted model unreachable"
                : "no NEBIUS_API_KEY set"}
          </div>
          {status.data?.local_model_reachable && <div>local model available</div>}
          {notifications.map((n, i) => (
            <div key={i} style={{ marginTop: 6, color: "var(--accent)" }}>
              {n.type === "notification" ? n.message : ""}
            </div>
          ))}
        </div>
      </nav>

      <main>
        {tab === "Chat" && <Chat events={events} />}
        {tab === "Activity" && <Activity tick={tick} />}
        {tab === "Memory" && <MemoryView tick={tick} />}
        {tab === "Knowledge" && <Knowledge tick={tick} />}
        {tab === "Skills" && <Skills tick={tick} />}
        {tab === "Settings" && <Settings tick={tick} />}
      </main>
    </div>
  );
}
