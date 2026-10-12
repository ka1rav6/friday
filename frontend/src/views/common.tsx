import { useCallback, useEffect, useState } from "react";
import { api, ApiError } from "../api";

/** Loads JSON from the API, exposing the backend's own error message. */
export function useLoader<T>(path: string, deps: unknown[] = []) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const reload = useCallback(async () => {
    setLoading(true);
    try {
      setData(await api.get<T>(path));
      setError(null);
    } catch (e) {
      setError(e instanceof ApiError ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [path]);

  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => void reload(), [reload, ...deps]);
  return { data, error, loading, reload, setError };
}

export function Err({ children }: { children: string | null }) {
  return children ? <div className="err">{children}</div> : null;
}

export function Empty({ children }: { children: React.ReactNode }) {
  return <div className="empty">{children}</div>;
}

export function Page(props: { title: string; sub: string; children: React.ReactNode; actions?: React.ReactNode }) {
  return (
    <>
      <div className="spread">
        <div>
          <h1>{props.title}</h1>
          <p className="sub">{props.sub}</p>
        </div>
        {props.actions}
      </div>
      {props.children}
    </>
  );
}

export function when(cond: boolean, ok: string, bad: string) {
  return cond ? ok : bad;
}

export function statusTag(status: string) {
  const cls =
    status === "completed" || status === "approved"
      ? "ok"
      : status === "failed" || status === "denied"
        ? "bad"
        : status === "running" || status === "pending"
          ? "warn"
          : "";
  return <span className={`tag ${cls}`}>{status}</span>;
}

export function ago(iso: string | null) {
  if (!iso) return "—";
  const ms = Date.now() - new Date(iso).getTime();
  const m = Math.round(ms / 60000);
  if (m < 1) return "just now";
  if (m < 60) return `${m}m ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h}h ago`;
  return `${Math.round(h / 24)}d ago`;
}
