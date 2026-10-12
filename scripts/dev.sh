#!/usr/bin/env bash
# Runs the backend and the Vite dev server together, and cleans both up on exit.
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ ! -f .env ]]; then
  echo "note: no .env found. Copy .env.example and add NEBIUS_API_KEY for the hosted model." >&2
fi

cargo build --quiet
./target/debug/friday &
backend=$!
trap 'kill $backend 2>/dev/null || true' EXIT

if [[ -d frontend/node_modules ]]; then
  (cd frontend && npm run dev)
else
  echo "frontend deps missing - run: cd frontend && npm install" >&2
  wait $backend
fi
