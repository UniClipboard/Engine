# Findings
- Desktop repo: /Users/mark/MyProjects/uni/desktop, `Cargo.toml` pins `uc-engine` git rev. Published 1.0.0 releases:
  alpha.2-5, alpha.8-17 (alpha.14/15 share rev). Unpublished: alpha.1/6/7 and fix/codex/engine variants.
- All 13 revs exist in Engine repo; alpha.9 rev 9149a875 is off the main line.
- All revs expose EngineConfig/HostCapabilities/HostDirectories/HostSecureStorage and a dev-tools feature.
- Precedent: scripts/testing/run-connection-recovery-e2e.sh builds rc15 (f6f305d9 = Desktop alpha.10) host by
  git archive + overlay current tests/hosts/connectivity + scripts/testing/rc15-test-host.patch (10 lines).
- uc-connectivity-host already supports secure storage export/import via start field and `secure_storage` command.
- Engine GitHub releases: 27 (v0.20.0-rc.11 onward). DB migrations: 48 (v1.0.0-rc.1) -> 61 (rc.16+).
