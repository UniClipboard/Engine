# UniClipboard note for the vendored iroh-mdns-address-lookup

This directory is `iroh-mdns-address-lookup` 0.6.0 from crates.io, unchanged except for `Cargo.toml`:

- the `swarm-discovery` dependency points at `../swarm-discovery` (the UniClipboard-patched copy), and
- the example target and dev-dependencies are removed, and the package is detached from the Engine workspace.

Why it exists: `[patch]` entries are only read from the root manifest of the build, so repositories that depend on Engine
(Desktop) would otherwise have to repeat the `swarm-discovery` patch. With this vendored crate, `uc-infra` depends on both
by path and no downstream `[patch]` is needed. Drop this directory when `swarm-discovery` ships the fix upstream.
Source sources and licence: crates.io `iroh-mdns-address-lookup` 0.6.0, `MIT OR Apache-2.0`.
