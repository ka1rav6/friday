# Deployment

FRIDAY is one binary plus a SQLite file. There are three sensible profiles.

## 1. Local, single user (the intended mode)

```bash
cp .env.example .env            # add NEBIUS_API_KEY
cd frontend && npm install && npm run build && cd ..
cargo run --release             # http://127.0.0.1:8787
```

The backend serves the built frontend from `frontend/dist`, so this is one
process on one port. For frontend development run `npm run dev` instead and use
`http://localhost:5173`, which proxies `/api` (WebSocket included) to the
backend.

Keep `bind = "127.0.0.1:8787"`. There is **no authentication layer** — anyone
who can reach the port can read your memory and run your tools.

### Running it as a service

```ini
# ~/.config/systemd/user/friday.service
[Unit]
Description=FRIDAY personal AI OS
After=network-online.target

[Service]
WorkingDirectory=/home/you/dev/projects/friday
EnvironmentFile=/home/you/dev/projects/friday/.env
ExecStart=/home/you/dev/projects/friday/target/release/friday
Restart=on-failure

[Install]
WantedBy=default.target
```

```bash
systemctl --user daemon-reload && systemctl --user enable --now friday
```

The scheduler only fires while this service is running. On resume from sleep,
`scheduled_workflow_recovery` reports what was missed and whether a rerun is
safe; FRIDAY will not silently repeat work.

## 2. Public demo profile

A public demo must expose **no personal data, no real credentials and no
unrestricted execution**. Use a dedicated config:

```toml
# .friday/config.toml  — demo
bind = "0.0.0.0:8787"
timezone = "UTC"
mode = "auto"

[permissions]
allow_local_mutation    = true      # needed for memory + briefing demos
allow_network           = true      # research demo
allow_external_side_effect = false
allow_privileged        = false
command_allowlist       = []        # no build/test execution at all
max_file_bytes          = 65536
max_tool_output_bytes   = 32768

[[workspaces]]
id   = "demo"
path = "./examples/demo-workspace"  # a scratch tree, never $HOME

[briefing]
email_provider    = "synthetic"     # clearly-labelled demo data
calendar_provider = "synthetic"
```

Checklist before exposing it:

- [ ] `command_allowlist = []`, so `run_build` / `run_project_tests` refuse.
- [ ] Exactly one workspace, pointing at a throwaway directory.
- [ ] `email_provider = "synthetic"`; never a real mailbox.
- [ ] `.env` holds only the Nebius demo key. No personal tokens.
- [ ] A reverse proxy in front terminating TLS and rate-limiting `/api/chat`.
- [ ] The database is disposable — reset it between sessions.
- [ ] `GET /api/config` returns no secrets (it cannot: no secret is a config field).

Verify the posture from outside:

```bash
curl -s $HOST/api/config | grep -ci 'api_key'          # expect 0
curl -s $HOST/api/status | jq '.counts'                 # expect a near-empty database
```

### Docker

```dockerfile
FROM rust:1-slim AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked

FROM node:20-slim AS ui
WORKDIR /ui
COPY frontend/package*.json ./
RUN npm ci
COPY frontend .
RUN npm run build

FROM debian:stable-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /src/target/release/friday /usr/local/bin/friday
COPY --from=build /src/examples /app/examples
COPY --from=ui /ui/dist /app/frontend/dist
ENV FRIDAY_BIND=0.0.0.0:8787 FRIDAY_DB=sqlite:///data/friday.db?mode=rwc
VOLUME /data
EXPOSE 8787
CMD ["friday"]
```

```bash
docker build -t friday . && docker run -p 8787:8787 -v friday-data:/data --env-file .env friday
```

Mount `/data` so memory and research survive a container replacement.

## 3. Guaranteed scheduling (optional)

The local scheduler cannot fire while the machine is asleep — it says so rather
than pretending otherwise. For delivery that does not depend on your laptop,
run FRIDAY on a host that stays up and have an external scheduler poke it:

```bash
# e.g. a Nebius Serverless Job, daily at 07:30
curl -fsS -X POST https://your-host/api/jobs/morning-briefing/run
```

The endpoint is idempotent per local date, so a retrying scheduler cannot
produce two briefings for one day.

## Operations

| Task | Command |
| --- | --- |
| Health | `curl localhost:8787/api/health` |
| Capability check | `friday status` |
| Validate config | `friday config validate` |
| Logs | `RUST_LOG=friday=debug` (secrets are redacted before logging) |
| Backup | stop the service, copy `data/friday.db*` (all three WAL files) |
| Export knowledge | `export_knowledge` tool, or `GET /api/research` |
| Reset the demo | stop, delete `data/friday.db*`, restart (migrations re-run) |

Schema changes are additive and tracked in `_migrations`, so an upgrade is a
binary swap plus a restart. There is no downgrade path — back up before
upgrading.

## Credentials

| Secret | Where it lives | Rotate |
| --- | --- | --- |
| `NEBIUS_API_KEY` | process environment / `.env` (gitignored) | replace and restart; revoke in the Nebius console |
| `TAVILY_API_KEY` | same | same |

Never put a credential in `config.toml`: that file is written by
`PUT /api/config` and is meant to be readable and shareable. Child processes
started by `run_build` / `run_project_tests` have both keys stripped from their
environment.
