# Demo workspace

A throwaway file tree for the public demo profile in
[docs/deployment.md](../../docs/deployment.md). Nothing here is real: it exists
so `search_local_files`, `read_local_file` and `summarize_document` have
something safe to read when FRIDAY is exposed to people who are not you.

Point a demo instance at this directory and nothing else:

```toml
[[workspaces]]
id   = "demo"
path = "./examples/demo-workspace"
```
