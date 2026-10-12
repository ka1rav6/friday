/** Typed client for the FRIDAY HTTP API. Errors surface the backend's own
 *  actionable message rather than a generic failure. */

export class ApiError extends Error {}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(`/api${path}`, {
    ...init,
    headers: init?.body ? { "content-type": "application/json" } : undefined,
  });
  const text = await res.text();
  const body = text ? JSON.parse(text) : {};
  if (!res.ok) throw new ApiError(body.error ?? `${res.status} ${res.statusText}`);
  return body as T;
}

export const api = {
  get: <T>(p: string) => request<T>(p),
  post: <T>(p: string, body?: unknown) =>
    request<T>(p, { method: "POST", body: body ? JSON.stringify(body) : undefined }),
  put: <T>(p: string, body?: unknown) =>
    request<T>(p, { method: "PUT", body: body ? JSON.stringify(body) : undefined }),
  del: <T>(p: string) => request<T>(p, { method: "DELETE" }),
};

// ---------------------------------------------------------------- shapes

export type Status = {
  mode: string;
  timezone: string;
  selected_provider: string | null;
  selected_model: string | null;
  routing_reason: string | null;
  hosted_credential_present: boolean;
  hosted_nvidia_model_reachable: boolean;
  local_model_reachable: boolean;
  network_allowed: boolean;
  counts: Record<string, number>;
};

export type ChatReply = {
  conversation_id: string;
  task_id: string;
  status: string;
  reply: string;
  model: string;
  iterations: number;
  tool_calls: number;
  tokens: { prompt: number; completion: number };
};

export type Memory = {
  id: string;
  category: string;
  content: string;
  source: string;
  provenance: string;
  confidence: number;
  approved: boolean;
  created_at: string;
};

export type Task = {
  id: string;
  title: string;
  status: string;
  priority: string;
  due_at: string | null;
  created_at: string;
  overdue: boolean;
};

export type AgentRun = {
  run_id: string;
  task_id: string;
  parent_task_id: string | null;
  role: string;
  status: string;
  started_at: string;
  finished_at: string | null;
  iterations: number;
  tool_calls: number;
};

export type ToolCall = {
  tool: string;
  permission: string;
  ok: boolean;
  duration_ms: number;
  created_at: string;
};

export type TaskDetail = {
  task: { id: string; title: string; status: string; result: string | null; error: string | null };
  subtasks: { id: string; title: string; status: string; role: string | null }[];
  tool_calls: ToolCall[];
  evidence: {
    verified: boolean;
    successful_tools: string[];
    failed_tools: { tool: string; error: string | null }[];
    missing: string[];
    instruction: string;
  };
};

export type Skill = {
  id: string;
  name: string;
  version: string;
  scope: string;
  description: string;
  path: string;
  declared_tools: string[];
  enabled: boolean;
  updated_at: string;
};

export type ResearchDoc = {
  id: string;
  title: string;
  summary: string;
  created_at: string;
  age_days?: number;
  stale?: boolean;
  sources?: string[];
};

export type Approval = {
  id: string;
  action: string;
  reason: string;
  risk: string;
  preview: string;
  status: string;
  created_at: string;
};

export type ToolDef = {
  name: string;
  description: string;
  permission: string;
  network: boolean;
  timeout_ms: number;
};

export type AuditEntry = {
  id: number;
  actor: string;
  category: string;
  action: string;
  detail: string;
  created_at: string;
};

export type Job = {
  id: string;
  name: string;
  skill: string;
  at_time: string;
  timezone: string;
  enabled: boolean;
  next_run_at: string | null;
  last_run_at: string | null;
  last_status: string | null;
  last_error: string | null;
};

// ---------------------------------------------------------------- events

export type Event =
  | { type: "message"; conversation_id: string; role: string; content: string }
  | { type: "task_update"; task_id: string; status: string; title: string }
  | { type: "agent_update"; run_id: string; task_id: string; role: string; status: string; detail: string }
  | { type: "tool_call"; task_id: string | null; tool: string; ok: boolean; summary: string }
  | { type: "approval_required"; id: string; action: string; risk: string; preview: string }
  | { type: "notification"; category: string; message: string };

/** Subscribes to the backend event stream, reconnecting on drop. */
export function subscribe(onEvent: (e: Event) => void): () => void {
  let socket: WebSocket | null = null;
  let timer: number | undefined;
  let closed = false;

  const connect = () => {
    if (closed) return;
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    socket = new WebSocket(`${proto}//${location.host}/api/events`);
    socket.onmessage = (m) => {
      try {
        onEvent(JSON.parse(m.data) as Event);
      } catch {
        /* a malformed frame must not kill the stream */
      }
    };
    socket.onclose = () => {
      if (!closed) timer = window.setTimeout(connect, 1500);
    };
  };
  connect();

  return () => {
    closed = true;
    window.clearTimeout(timer);
    socket?.close();
  };
}
