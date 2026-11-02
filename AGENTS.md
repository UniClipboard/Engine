# UniClipboard Engine Maintenance Map

This file defines repository constraints and navigation. Long-term knowledge belongs in `docs/`.
Read [ARCHITECTURE.md](ARCHITECTURE.md) for the current architecture and [docs/README.md](docs/README.md) for the documentation index.

## Read first

- [Resource vision](VISION.md)
- [Core beliefs](docs/design-docs/core-beliefs.md)
- [Engineering and module design principles](docs/design-docs/engineering-principles.md)
- [Rust style](docs/design-docs/rust-style.md)
- [Error handling and conversion](docs/design-docs/error-handling.md)
- [Runtime observability](docs/design-docs/observability.md)
  - Before changing logs, tracing, sampling, or observability acceptance, read the business-record organization standard in that document.
- [Security architecture](docs/SECURITY.md)
- [Execution plans and technical debt](docs/PLANS.md)

Read the nearest local `AGENTS.md` when entering a directory. Local files provide scope maps and link to rules in `docs/`.

## Invariants

- Encrypt user content with MasterKey AEAD before writing it to SQLite, disk caches, search indexes, or configuration files.
  Do not write plaintext user content to a database.
  User content includes clipboard content, synchronized content, derived data, clipboard filenames, and paths.
  The only plaintext exceptions are the approved file bodies and managed-cache basenames described below.
- Store credentials and key material only in system secure storage.
  This includes MasterKey, passphrases, KEKs, relay access tokens, and authentication tokens.
  Do not write them to configuration, logs, or ordinary files.
  The existing Engine exception is runtime key material in `vault/profile-secrets-v1`. Do not expand this exception.
  Host repositories define their own existing exceptions.
- Configuration can remain plaintext without approval for each field.
  Configuration includes device names, save directories, relay addresses, and LAN addresses.
  Configuration must not contain user content or credentials. Logging and observability restrictions still apply.
- Content-type enums, file bodies, and sanitized original filenames used only as actual basenames in the managed inbound cache can remain plaintext.
  Original paths, database and search fields, logs, and correlation metadata must remain encrypted or redacted.
- Except for configuration, treat each new persistent field or file as sensitive by default.
  Describe any proposed plaintext exception in the PR and obtain explicit approval.
- Logs, public errors, and observability events must not contain clipboard content, passwords, keys, complete tokens, invitations, device names, addresses, filenames, or paths.
- Fix core problems in this repository. Product repositories must not maintain patch copies.
- `uc-engine` is the only stable Rust entry point. iOS, Android, and HarmonyOS bindings depend only on `uc-engine` and use the same version.
- P2P is the default capability. Enable LAN compatibility only after explicit user selection.
  Do not switch to LAN automatically after a P2P failure.
- Do not publish internal crates or bindings to crates.io. Deliver them only through GitHub Releases with verification metadata.

## Directory map

- `crates/`: Core rules, Application workflows, Infra implementations, and the stable Engine entry point.
- `bindings/`: Thin iOS, Android, and HarmonyOS bindings.
- `compatibility/`: The independently versioned and released LAN compatibility line.
- `tests/hosts/`: Mobile acceptance hosts. Do not add product features here.
- `scripts/architecture/`: Ownership, dependency direction, and release provenance checks.
- `scripts/release/`: Artifact collection, manifests, and pre-release verification.
- `docs/design-docs/`: Long-term design, stable contracts, and ADRs.
- `docs/exec-plans/`: Active plans, completed plans, and technical debt.
- `docs/product-specs/`: Product requirements and acceptance criteria.
- `docs/generated/`: Reproducible schemas and diagram snapshots.
- `docs/references/`: Domain glossary and source mappings.

## Change requirements

- Write project documentation in English. Keep code comments in Chinese. Use English code identifiers and commit messages.
- Keep one source of truth. Do not retain parallel old and new implementations or documentation entry points indefinitely.
- Use relative repository paths in documentation.
- When using planning-with-files, put `task_plan.md`, `findings.md`, and `progress.md` under `.planning/<YYYY-MM-DD>-<task-name>/`.
  Do not create these files at the repository root. Reuse the original directory when continuing a task.
- Run Rust commands from the repository root through `just cargo`.
  Before running Cargo directly, source `scripts/build-cache/env.sh`.
- Reuse the repository `target` by default. If that path is unavailable, stop and repair it first.
  Do not move a task-specific `CARGO_TARGET_DIR` to `/tmp` or `/private/tmp` to continue a build.
- Agents can read in parallel and edit separate files. One owner must run Cargo validation serially through the shared `target`.
  Do not create a complete build directory for each agent.
- Preserve the shared compilation cache. Do not clear `RUSTC_WRAPPER` unless the task diagnoses the cache itself.
- If build isolation is necessary, use an explicitly selected external directory for reproducible outputs.
  Before task completion, check for active processes and reclaim that directory. Do not leave cleanup to a later session.
- Do not use `unwrap()`, `expect()`, `println!()`, or `eprintln!()` in production code.
- Import Rust names at the module entry point by default.
  Do not repeat fully qualified `crate::` paths in signatures or bodies. Follow the exceptions and exemption format in the Rust style guide.
- Preserve the complete source chain for lower-layer failures in Application. Do not stringify or discard errors.
  Follow the error-handling document.
- Give each cross-layer behavior one complete owner. Core holds rules, Application owns workflows, Infra supplies capabilities, and Engine assembles them.
- One workflow owner controls order, results, retry, and restart recovery. This does not require one file or one type.
  Separate implementation, workflow-specific ports, models, errors, and tests by responsibility within the module.
  Keep `mod.rs` focused on module declarations and necessary exports.
  File separation must not expand public interfaces or require callers to assemble internal steps.
- Implement cross-layer duration measurement and result classification only through domain port decorators in the Engine composition layer.
- Use `uc_*!` logging macros. The field catalog constrains field names and value categories at compile time.
  Follow [ADR-030](docs/design-docs/decisions/030-typed-log-events-and-enforcement.md).
  `scripts/architecture/check-direct-log-macros.mjs` rejects direct `tracing` logging macros.
- Each independent business record must explain the trigger, complete action, and final result.
  Do not promote lower-layer calls or normal cleanup into business entry points.
  Separate business actions from runtime diagnostics and test records from product records.
  Keep correlation, completion, and acceptance rules in the business-record organization standard in `docs/design-docs/observability.md`.
- Do not expand Engine interfaces for observability by exposing internal Application or Core stages, state objects, business identifiers, or step queries.
  Do not expand facades, ports, or result interfaces for that purpose.
  Engine can decorate only existing complete capabilities through their inputs and outputs.
  The complete workflow owner must supply opaque context for cross-step correlation.
  Engine must not use that context to orchestrate business steps.
- Before implementing a feature, define the complete owner, the caller's single action, success and failure results, and retry and restart responsibilities.
- Before changing a persistent format, follow the format-evolution rules in `docs/design-docs/engineering-principles.md`.
  An unreleased feature branch normally adds only one final format version.
  Development slices and local commits do not freeze a format.
- When behavior, interfaces, invariants, or important tradeoffs change, update the affected Product Spec, Design Doc, ADR, or execution plan.
  A change without lasting information does not require a new documentation record.
- Never write unit tests after writing implementation code.
  Strongly prefer end-to-end tests as the sole test mechanism.
  Produce a verifiable and repeatable artifact at the end of each end-to-end test.
  If isolated testing is necessary, enumerate the system's failure modes before writing code.
- Select the simplest implementation that meets the current requirement.
  Establish a working end-to-end slice before adding complexity.
  Check existing dependencies and maintained solutions before adding a library or abstraction.

## Resource design requirements

Follow [VISION.md](VISION.md) for the resource goals, challenge budgets, and acceptance scenarios.
The numeric challenges are not enforced limits until platform baselines and explicit gates exist.
Treat measurable resource use and predictable resource bounds as mandatory design properties.
Do not claim that the current Engine is lightweight or resource-heavy without measurements.

- Before adding a long-lived allocation, shared owner, cache, queue, task, thread, connection, or worker, identify its complete owner.
  Define its lifetime, aggregate resource bound, overload result, and release path.
  Include success, failure, cancellation, shutdown, retry, and restart behavior.
- Use bounded queues by default. Bound waiting tasks and retained payload bytes as well as active concurrency.
  A semaphore alone does not bound queued work. Document any exception with a measured or proven bound.
- Give every long-lived cache a capacity bound or an expiry policy with bounded retention under the maximum admitted workload.
  Include retained buffer capacity and per-device multiplication in the total budget.
- Prefer bounded streaming and paginated reads for large payloads and history.
  Avoid unnecessary full-payload copies. Bound decoded pixels and image conversion concurrency separately.
  Do not invent an ad hoc chunked-encryption scheme or weaken the existing persistence and protocol contracts.
- Activate expensive capabilities when needed. Release temporary state when work ends.
  Preserve required listeners, connections, discovery, and synchronization latency.
  Prefer event-driven waits to polling. If the platform requires polling, bound its frequency and apply backoff.
- Keep CPU-intensive and blocking work off asynchronous executor and platform-affine threads.
  Bound worker admission and blocking work. Measure runtime and thread changes before selecting a configuration.
- Reuse existing dependencies before adding a crate. Review default features, duplicate runtimes, background threads, caches, and buffers.
  Do not replace a maintained library solely because its API or package appears smaller.
- Prefer borrowing when ownership permits. Justify long-lived shared ownership and buffer retention.
  Do not ban cloning or require `Arc`, `Bytes`, `Cow`, or a small-container library without evidence.
- Every performance-sensitive change must be evaluated against a reproducible baseline under equivalent conditions.
  Performance claims must be supported by measured evidence.
  Compare the baseline and candidate with a repeatable end-to-end workload.
  Report memory, CPU, or latency results for the affected resource and check for costs shifted to other resources.
  Include peaks, post-work recovery, and a verifiable artifact as specified in `VISION.md`.
- In the PR, state the resource bound, comparison conditions, results, and tradeoffs.
  Record missing measurements as skipped. Do not describe an unmeasured change as a proven optimization.
  If a required check cannot run, state the blocker and leave acceptance incomplete.
- Preserve data security, synchronization correctness, and recovery guarantees during every optimization.
  Do not disable the shared build cache or alter release panic behavior merely to improve a benchmark.
- Add numeric CI gates only after establishing platform baselines, measurement noise, tolerances, and an owner.
  Do not claim that a proposed benchmark or gate already runs in CI.

## Delivery checks

Even when a change does not alter behavior, run at least:

```bash
just cargo metadata --locked --format-version 1
just cargo check --workspace --all-targets --locked
just cargo fmt --all -- --check
node scripts/architecture/check-rust-style.mjs
node scripts/architecture/check-engine-repository.mjs
git diff --check
```

For release changes, also run:

```bash
node scripts/release/verify-release-bundle.mjs <artifact-directory>
```

Record unexecuted device-matrix items as skipped. Do not mark them as passed.
