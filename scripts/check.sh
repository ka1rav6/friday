#!/usr/bin/env bash
# Everything CI should run. No network required.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "== fmt =="      && cargo fmt --check
echo "== clippy =="   && cargo clippy --all-targets -- -D warnings
echo "== tests =="    && cargo test --quiet
echo "== tools doc ==" && cargo run --quiet -- tools --markdown > /tmp/tools.md \
  && diff -q /tmp/tools.md docs/tools.md \
  || { echo "docs/tools.md is stale: cargo run -- tools --markdown > docs/tools.md" >&2; exit 1; }

if [[ -d frontend/node_modules ]]; then
  echo "== frontend ==" && (cd frontend && npx tsc --noEmit && npm run build --silent)
else
  echo "skipping frontend checks (run npm install in frontend/)"
fi
echo "all checks passed"
