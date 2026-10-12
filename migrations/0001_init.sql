CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS conversations (
  id TEXT PRIMARY KEY, title TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS messages (
  id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  role TEXT NOT NULL, content TEXT NOT NULL, created_at TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS idx_messages_conv ON messages(conversation_id, created_at);

CREATE TABLE IF NOT EXISTS memories (
  id TEXT PRIMARY KEY, category TEXT NOT NULL, content TEXT NOT NULL,
  source TEXT NOT NULL, provenance TEXT NOT NULL DEFAULT 'user_stated',
  confidence REAL NOT NULL DEFAULT 1.0, tags TEXT NOT NULL DEFAULT '[]',
  retention TEXT NOT NULL DEFAULT 'durable', approved INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(id UNINDEXED, content, tags, tokenize='porter unicode61');

CREATE TABLE IF NOT EXISTS tasks (
  id TEXT PRIMARY KEY, parent_id TEXT, title TEXT NOT NULL, description TEXT NOT NULL DEFAULT '',
  kind TEXT NOT NULL DEFAULT 'user', role TEXT, status TEXT NOT NULL DEFAULT 'pending',
  priority TEXT NOT NULL DEFAULT 'normal', due_at TEXT, result TEXT, error TEXT,
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL, finished_at TEXT);
CREATE INDEX IF NOT EXISTS idx_tasks_parent ON tasks(parent_id);
CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);

CREATE TABLE IF NOT EXISTS task_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT, task_id TEXT NOT NULL, kind TEXT NOT NULL,
  detail TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS idx_task_events ON task_events(task_id, id);

CREATE TABLE IF NOT EXISTS agent_runs (
  id TEXT PRIMARY KEY, task_id TEXT NOT NULL, parent_task_id TEXT, role TEXT NOT NULL,
  objective TEXT NOT NULL, allowed_tools TEXT NOT NULL DEFAULT '[]', status TEXT NOT NULL,
  model TEXT, iterations INTEGER NOT NULL DEFAULT 0, tool_calls INTEGER NOT NULL DEFAULT 0,
  prompt_tokens INTEGER NOT NULL DEFAULT 0, completion_tokens INTEGER NOT NULL DEFAULT 0,
  result TEXT, error TEXT, started_at TEXT NOT NULL, finished_at TEXT);

CREATE TABLE IF NOT EXISTS tool_calls (
  id TEXT PRIMARY KEY, task_id TEXT, agent_run_id TEXT, tool TEXT NOT NULL,
  args TEXT NOT NULL, result TEXT, ok INTEGER NOT NULL DEFAULT 0, permission TEXT NOT NULL,
  duration_ms INTEGER NOT NULL DEFAULT 0, error TEXT, created_at TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS idx_tool_calls_task ON tool_calls(task_id, created_at);

CREATE TABLE IF NOT EXISTS evidence (
  id TEXT PRIMARY KEY, task_id TEXT NOT NULL, claim TEXT NOT NULL, kind TEXT NOT NULL,
  tool_call_id TEXT, detail TEXT NOT NULL DEFAULT '', verified INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS idx_evidence_task ON evidence(task_id);

CREATE TABLE IF NOT EXISTS skills (
  id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, version TEXT NOT NULL, scope TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '', path TEXT NOT NULL, source TEXT NOT NULL DEFAULT 'local',
  declared_tools TEXT NOT NULL DEFAULT '[]', instructions TEXT NOT NULL DEFAULT '',
  content_hash TEXT NOT NULL DEFAULT '', enabled INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS skill_runs (
  id TEXT PRIMARY KEY, skill_id TEXT NOT NULL, task_id TEXT, status TEXT NOT NULL,
  output TEXT, created_at TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS scheduled_jobs (
  id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, skill TEXT NOT NULL,
  at_time TEXT NOT NULL DEFAULT '07:30', timezone TEXT NOT NULL DEFAULT 'UTC',
  enabled INTEGER NOT NULL DEFAULT 1, last_run_at TEXT, last_status TEXT, last_error TEXT,
  next_run_at TEXT, created_at TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS job_runs (
  id TEXT PRIMARY KEY, job_id TEXT NOT NULL, idempotency_key TEXT NOT NULL UNIQUE,
  status TEXT NOT NULL, output TEXT, error TEXT, started_at TEXT NOT NULL, finished_at TEXT);

CREATE TABLE IF NOT EXISTS research_documents (
  id TEXT PRIMARY KEY, title TEXT NOT NULL, question TEXT NOT NULL DEFAULT '',
  summary TEXT NOT NULL DEFAULT '', content TEXT NOT NULL, tags TEXT NOT NULL DEFAULT '[]',
  freshness_days INTEGER NOT NULL DEFAULT 180, created_at TEXT NOT NULL);
CREATE VIRTUAL TABLE IF NOT EXISTS research_fts USING fts5(id UNINDEXED, title, summary, content, tokenize='porter unicode61');

CREATE TABLE IF NOT EXISTS research_sources (
  id TEXT PRIMARY KEY, document_id TEXT NOT NULL REFERENCES research_documents(id) ON DELETE CASCADE,
  url TEXT NOT NULL, title TEXT NOT NULL DEFAULT '', snippet TEXT NOT NULL DEFAULT '',
  published TEXT, fetched_at TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS approval_requests (
  id TEXT PRIMARY KEY, task_id TEXT, action TEXT NOT NULL, reason TEXT NOT NULL DEFAULT '',
  risk TEXT NOT NULL DEFAULT 'medium', preview TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL DEFAULT 'pending', decided_at TEXT, created_at TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS inbox (
  id TEXT PRIMARY KEY, provider TEXT NOT NULL, external_id TEXT NOT NULL,
  subject TEXT NOT NULL, sender TEXT NOT NULL, body TEXT NOT NULL,
  received_at TEXT NOT NULL, link TEXT NOT NULL DEFAULT '',
  UNIQUE(provider, external_id));

CREATE TABLE IF NOT EXISTS calendar_events (
  id TEXT PRIMARY KEY, provider TEXT NOT NULL, external_id TEXT NOT NULL, title TEXT NOT NULL,
  starts_at TEXT NOT NULL, ends_at TEXT, location TEXT NOT NULL DEFAULT '',
  notes TEXT NOT NULL DEFAULT '', UNIQUE(provider, external_id));

CREATE TABLE IF NOT EXISTS briefings (
  id TEXT PRIMARY KEY, for_date TEXT NOT NULL UNIQUE, content TEXT NOT NULL,
  sources TEXT NOT NULL DEFAULT '[]', created_at TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS audit_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT, actor TEXT NOT NULL, category TEXT NOT NULL,
  action TEXT NOT NULL, detail TEXT NOT NULL DEFAULT '', task_id TEXT,
  created_at TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS idx_audit ON audit_log(created_at);
