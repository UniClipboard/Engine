# Dependency hardening (upstream: t-0095 assessment; parallel Desktop t-0099)

Worktrees: Engine = this tree (HEAD 28550b4f); Desktop = /Users/mark/.herdr/worktrees/desktop/hp-uni-t-0101-desktop-dependency-hardening
(branch fix/dependency-hardening, base origin/main 687ce81da). No commit/push/PR. Remote hosts offline; no heavy Mac rebuilds.

## Failure modes to guard against (listed before implementation)

1. swarm-discovery: Engine's third_party copy is 0.6.1, Desktop's release lock resolves 0.6.3.
   - Patching Desktop with the 0.6.1 copy would silently DOWNGRADE 0.6.3 multicast fixes (per-interface join, set_multicast_if).
   - Patch must be rebased on 0.6.3 (Engine single source), Desktop consumes it by immutable Engine rev; lock must show patched source.
   - Evidence: closed-mailbox test on the patched crate fails against unpatched 0.6.3 (RED) and passes patched (GREEN).
2. Audit: today's advisory DB has more findings than the 3 in t-0095; a hard gate would fail on unfixable ones (libcrux-aesgcm has no patch).
   - Must not hide with a blanket ignore; per-advisory exception only with evidence, expiry, and a guard.
   - Minimal update: `cargo update -p` only; no mass upgrade; Desktop lock must not regress.
3. from_toml: `pub use` from uc-core; uc-core is internal (not published), uc-engine does not re-export. Check Desktop/downstream imports.
4. image features: removing a codec silently breaks clipboard input at runtime. Needs 3-platform E2E; this host is macOS-only and remotes are offline -> item stays unverified/blocked, not done.
5. tokio test-util: uc-desktop + uc-webserver must move together, else feature unification splits GUI/daemon graphs.
6. serde_with base64: feature might be used through `serde_with::base64::Base64`; grep before removal; serialization golden test.

## Phases
- [ ] A. Audit facts (done: advisory DB 2026-09-28 cloned to scratchpad)
- [ ] B. Item 1 swarm-discovery
- [ ] C. Item 2 Engine lock + cargo-audit CI
- [ ] D. Items 3, 6 Engine
- [ ] E. Item 5 Desktop
- [ ] F. Item 4 image inventory (+ E2E plan for coordinator)
- [ ] G. Report
