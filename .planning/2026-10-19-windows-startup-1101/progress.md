# Progress
- 2026-10-19: oriented; plan + failure modes written.
- Instrumentation 562e6d1a; E2E 70509b4a; fix ac01c655; docs 9f10d3ca.
- macOS before: natural overlap 2/48 failures (busy); after 48/48. Windows before: holder busy -> corrupt; after 4/4 pass.
- Windows natural race not hit (process start ~0.7 s). Live-instance overlap hang observed (out of scope).
- VM: resumed from suspended, C:\t0337 removed, suspended again.
- Upgrade-matrix overlay: a13 anchor resolves new dev-deps to existing diesel 2.3.9 / libsqlite3-sys 0.35.0 (edges only).
- Live-instance overlap hang reproduced on pre-fix build too (pre-existing).
- Final HEAD checks: metadata/check/fmt/style/repo/log-macros/diff-check exit 0; nextest observability 178/178.
