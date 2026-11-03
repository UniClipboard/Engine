# macOS Engine benchmark harness

Status: CPU normalization correction and repository validation complete; the first full macOS headless reference suite completed and verified. Reference approval and CI gates remain manual and unestablished. Owner: benchmark controller. Issue: #374.

The caller runs one suite command. Success requires real two-process clipboard synchronization, persisted content verification after restart, complete OS samples, and successful cleanup. Failures remain in immutable artifacts. The controller owns deadlines and cleanup; Engine owns business retries and durable recovery. Runs never retry failed samples silently.

Use the existing connectivity host, JSON pipe protocol, local rendezvous fixture and uc-testkit. Preserve the release profile and shared build cache. The first slice is fresh startup and one real text transfer, followed by timeout and crash cleanup acceptance. Expand to four scenarios and six runs per revision only after that slice passes.

Failure inventory before implementation: spawn failure, early exit, malformed/oversized response, blocked pipe, deadline expiry, wrong content, missing/duplicate history, restart loss, collector exit/API failure, controller interruption, artifact I/O failure, resource cleanup failure, incompatible binary provenance, competing benchmark process, and missing platform support.

Acceptance: process-level E2E cases produce JSON evidence; no unit tests are added after implementation. Required repository checks and connectivity regression checks must run. macOS is the only accepted platform in this phase. Linux, Windows, devices and CI scheduling remain skipped.

## Verified design correction

The first real text E2E proved that `CaptureCurrentClipboard` persists a snapshot but does not alone enter the automatic outbound listener flow. The benchmark adapter now emits the existing public `HostClipboardChangeStream` notification. It retains a single pending notification and does not change production APIs. The failed runs remain in local evidence.

## Implementation and acceptance receipt (2026-11-03)

The stable Engine interface is unchanged. `uc-benchmark` owns supervision, sampling, correctness checks and comparison. `uc-testkit` supplies the shared rendezvous fixture, a bounded asynchronous pipe client, profile leases and cleanup reports. Existing synchronous connectivity tests retain their assertion-based process driver; rewriting those tests is not required to introduce the bounded asynchronous owner.

The host storage guard rejected cross-worktree target reuse. The build driver therefore requires an explicit external isolation root for a detached baseline worktree and its own target. Builds remain serial and share MBX. The isolated worktree and target were removed after binary collection; the candidate target and shared caches were preserved.

The fixed Main snapshot and candidate Engine source both use `aa1b409fda38053d2d1e61e7801f680e2261ea26`. Production Engine sources are unchanged. Both use the same adapted test-host source. Build provenance distinguishes a modified overall tree from unchanged Engine sources. The baseline manifest's initial dirty-field ambiguity was corrected before measurement, with the original manifest retained in local build evidence.

Executed acceptance:

- Ten process E2E cases passed: short text, startup, disconnected idle, connected idle, 200-text synchronization, timeout, host crash, collector crash, controller interruption, and supervisor kill followed by stale-lease recovery.
- Six artifact/control E2E cases passed: A/A comparison, changed sample, missing sample, missing cleanup receipt, malformed response and oversized response.
- The final baseline and candidate release executables each synchronized 200 texts and verified the persisted history after receiver restart.
- The startup ABBA suite produced two warmups and six measured acceptance runs per executable. Per-run manifests and environment equality are verified. Reports explicitly mark these runs as acceptance-only.
- Existing active-clipboard connectivity regression: seven passed, one existing old-host interoperability case ignored.
- Required metadata, workspace/all-target check, formatting, Rust style, repository architecture and diff checks passed.

The optional release-profile invocation of the existing Rust integration-test binary failed while compiling `iroh-util`: rustc reported distinct identities for the same `n0-error 1.0.1` types. Cargo's dependency tree contains one version. No cache was cleared and no production dependency was changed. The default-profile integration regression passed. This does not invalidate the separately built release hosts' process E2E results.

Local evidence is under `target/benchmark-acceptance-02`, `target/benchmark-report-acceptance`, `target/benchmark-final-baseline-text`, `target/benchmark-final-candidate-text`, `target/benchmark-startup-repeatability-final`, and `target/benchmark-binaries-03`. Build/check logs remain under `target/benchmark-*.log`. These local artifacts are not committed.

## Remaining acceptance

The machine had an active Simulator and concurrent builds. They were not stopped because they belong to other work. Strict runs reject this environment. The first controlled reference baseline, all-scenario repeated performance comparison, Linux/Windows/device acceptance, and CI scheduling remain unexecuted. No numeric budget or optimization claim is certified by the acceptance-only data.

To finish the controlled baseline, use the README build and suite commands on an idle dedicated macOS host, without `--acceptance-only`. Preserve the complete raw artifact set. Do not reset an approved reference or raise budgets automatically.

## Controlled run launch (2026-11-03)

The user authorized stopping the Android emulator and starting the baseline. The verified Android emulator process exited after SIGTERM. No iOS simulator devices were booted; only an idle Simulator application window remained.

Preflight now checks booted iOS simulator devices and Android emulator processes. It no longer treats an idle Simulator window as an active device. A process E2E check proved that a booted device blocks admission before host startup. Required repository checks passed after this change.

The first formal attempt was rejected before its first warmup because an unrelated Cargo build started. Its failed receipt is retained under `target/benchmark-baseline-2026-11-03`. A separate attempt under `target/benchmark-baseline-2026-11-03-attempt-02` waits for 30 seconds without blocking builds or active simulator devices, then runs all four scenarios without `--acceptance-only`. Each scenario includes one warmup and six measured runs per executable. The driver records completion independently of the interactive session. Prior failures are retained rather than replaced.

Starting a run does not approve a reference baseline. Read its completion receipt, correctness, cleanup, environment and sample artifacts before accepting the measurements.

The second formal attempt completed four startup samples, then another external Cargo build caused the next admission to fail. No failed samples were replaced. Both attempts remain intact. The current background driver is under `target/benchmark-baseline-2026-11-03-scheduled`; it requires 300 quiet seconds and retains separate full-suite attempts. It retries only environmental refusals that occur before host startup and produce no sample directory. A real workload failure stops the job. The environment wait is bounded to two hours and the number of full-suite attempts is bounded to three. Read `state.json` for current status and `completion.json` for the eventual result.

## Controlled run result (2026-11-03)

The scheduled driver exhausted all three suite attempts and exited at 03:10 Asia/Shanghai. Each attempt stopped on a later environmental admission refusal, before the refused invocation created a sample directory. There is no complete four-scenario baseline.

Attempt 2 completed startup and disconnected idle: six measured runs per executable and scenario. All 24 measured run artifacts passed manifest, correctness and cleanup verification. Connected idle completed only its first warmup; text synchronization was not reached. Per-process idle RSS varied from approximately 29 to 221 MiB across runs, so these partial observations do not establish a stable memory reference or a code regression. Both Engine source commits are identical.

The sidecar partial report is retained at `target/benchmark-baseline-2026-11-03-scheduled/partial-report.md` and `.json`. Original attempts and raw samples remain unchanged. No benchmark processes remain active. A full host reservation and investigation of RSS variation remain necessary before approving a reference baseline.

## Idle-retention diagnosis and continuation design

Before further implementation, the replay diagnostic reproduced a 192 MiB drop inside the existing idle window. A live disconnected Engine run reproduced the same symptom. Sanitized `vmmap -summary` evidence showed 192 MiB in `Malloc Large (empty)`, while the live allocated heap remained about 5.3 MiB. After reclamation, footprint fell from approximately 205 MiB to 12 MiB in the same process. The production KDF defaults use 128 MiB for KEK derivation and 64 MiB for OPAQUE admission. This identifies freed initialization allocations retained temporarily by the macOS allocator; no Engine optimization or KDF weakening is required.

The controller remains the complete owner. Change the workload version to 2 and use a fixed 90-second post-setup warmup before idle/text measurement. Retain every initialization and warmup sample, label the warmup boundary, and report full-process peaks separately from steady-work metrics. The live replay diagnostic must become stable in the fixed work window while still retaining the initialization peak. This is a measurement-method correction, not a resource optimization.

Add bounded waiting between formal runs when environmental preflight is busy. Preserve successful immutable runs and the ABBA order. A refused invocation with no run directory and a confirmed busy environment is an admission deferral, recorded separately. An invocation that starts a host or fails for another reason remains a failed run and stops the suite. Bound aggregate environment waiting to one hour. Checkpoint progress and partial summaries on failure.

The first complete reference measurement can select one fixed Engine executable because baseline and candidate currently share the same production commit. The existing paired comparison remains available. The reference-only mode performs one warmup plus six measured runs for each scenario and does not manufacture a comparison column.

Pre-implementation failure inventory: an unstable idle window; initialization samples being dropped; comparison of different workload versions; an environment becoming busy between probe and launch; exhausted environment wait; a real host failure incorrectly retried; partial results lost after a later refusal; invalid/mutated binary provenance; and misleading comparison output in reference-only mode. Validate these through real process E2E and retained artifacts, without adding unit tests.

## Version 2 validation and reference launch

The live disconnected diagnostic passed with the fixed 90-second warmup. Within the 60-second work window, endpoint RSS spans were 65,536 bytes and 49,152 bytes. Full initialization samples remain available. The independent replay also validates the corrected even-sample median.

The real process admission acceptance passed: environmental waiting preserves completed samples, reference-only mode retains six measured startup runs, and an injected host crash stops the suite with completed cleanup instead of retrying the failed workload. Connected idle and 200-text synchronization passed separately. The text receiver restart verified all persisted entries.

Required locked metadata, workspace/all-target checking, formatting, Rust style, repository architecture and diff checks passed after the version 2 changes. No production Engine source, KDF parameter, release profile or shared cache setting changed.

The complete reference-only suite is running under `target/benchmark-reference-v2-2026-11-03`. Its configuration fixes the Engine commit, executable hashes, workload version, four scenarios and six measured runs per scenario. The runner uses formal admission, not acceptance-only mode. It preserves progress and waits between runs for competing workloads. The host is not reserved against unrelated tools starting later; environmental snapshots require review before reference approval. No reference has been approved automatically.

Version 2 validation evidence: `target/benchmark-idle-retention-fixed`, `target/benchmark-idle-retention-fixed-verified`, `target/benchmark-v2-admission-final`, `target/benchmark-v2-connected`, and `target/benchmark-v2-text`. Check logs are `target/benchmark-v2-*.log`.

## CPU unit correction

Source review found that `ri_user_time` and `ri_system_time` retain Mach absolute-time units. The initial collector mislabeled their sum as nanoseconds. On this Apple M4 host, `mach_timebase_info` reports a 125/3 ratio. The original CPU summaries therefore undercount elapsed CPU time. Memory samples and business correctness evidence are unaffected.

Before implementing the correction, the failure inventory is: a non-1:1 timebase causing silent undercount; integer overflow during conversion; invalid timebase information; raw counters lost during normalization; and an old mislabeled report being reused as a normalized comparison. Validate the collector against an independent child process `resource.getrusage` CPU measurement. Retain a failing run before the fix and a passing run after it. Do not run this CPU workload during the ongoing formal measurement.

Keep original formal run artifacts immutable. Their raw `ri_user_time` and `ri_system_time` counters permit deterministic corrected analysis using the recorded host timebase. The derived receipt must identify this correction and ignore legacy `cpu_time_ns` fields. Preserve original hashes and timebase evidence. The legacy suite's CPU columns are invalid; use the corrected receipt. Future collectors must store the timebase and normalized nanoseconds explicitly.

Primary source: Apple's XNU `fill_task_rusage` in `osfmk/kern/bsd_kern.c` assigns task power/recount counters directly to the rusage CPU fields. `task_power_info` retains Mach absolute-time units. The conversion uses `mach_timebase_info`.

## Continuation checkpoint: partial measurements and storage blocker

The version 2 suite completed six measured startup runs, six measured disconnected idle runs, and one measured connected idle run. Sixteen total artifacts, including three warmups, passed manifest and cleanup verification. The remaining connected idle runs and text suite have not completed. The runner retains all completed runs and waits between workloads; it does not reset the suite when other builds appear.

The corrected partial receipt is `target/benchmark-reference-v2-2026-11-03/partial-verified-report.md`, with machine-readable results and a SHA-256 verification manifest beside it. The partial snapshot is immutable. Startup medians were approximately 262–269 ms across the two endpoints. Disconnected idle work-window RSS medians were approximately 28.9 MiB; observed initialization peaks remained approximately 221 MiB. Some work windows still include allocator page reclamation. These observations do not approve a stable resource gate.

A real one-second child CPU workload reproduced the old collector's 97.6% undercount. Replaying its retained raw counters with the measured 125/3 timebase agreed with the independent `resource.getrusage` CPU clock within approximately 0.0072%. The corrected partial receipt and final analysis use those raw counters, not the legacy mislabeled fields. Original per-run artifacts and legacy reports remain unchanged. The new collector records the conversion ratio and normalized nanoseconds; new comparisons reject legacy CPU formats.

The next release build was blocked after formatting when the local storage guard tried to update `.uni-last-used`. A native stack sample identified `os.utime` inside `setattrlistat`. Separate five-second write probes in the configured target and external-volume root also timed out. Reads succeed; the APFS volume is mounted read/write with approximately 228 GiB free. No alternate target, cache reset, forced unmount, or unrelated process termination was used. Only this task's blocked build/check processes were terminated. Probe subprocesses exited. Both generated probe paths were subsequently confirmed absent.

After the CPU correction, Rust style and Python syntax checks passed, and `git diff --check` passed. Fresh release compilation, the corrected collector's live acceptance, workspace/all-target checking, format-check completion and the repository architecture check's OpenMLS execution remain blocked by external storage. Earlier version 2 checks passed before this CPU correction; do not represent those earlier results as validation of the correction.

The formal runner remains under `target/benchmark-reference-v2-2026-11-03`. It waits for unrelated Cargo builds to finish. The completion watcher runs corrected analysis only after the full suite passes and writes `completion.json`. Approval remains manual. This local suite measures the headless connectivity host, not iOS, Android or a native daemon.

The final locked metadata attempt also exceeded its 30-second deadline. Only its private process group was terminated. The partial manifest was reverified across all 25 listed files. No benchmark host processes remained active while admission was deferred.

## External-storage retry and validated CPU correction

The user requested an external-storage retry. Create, timestamp update and removal all succeeded in the original configured build target. A separate volume-root probe returned permission denied; the benchmark does not require volume-root writes. No storage configuration, target location, release setting or cache wrapper was changed.

The release benchmark executable rebuilt successfully. The live collector acceptance agreed with an independent one-second child CPU clock within approximately 0.018%. Queued admission, six real startup repetitions and fail-without-retry acceptance also passed with the corrected executable. Locked metadata, workspace/all-target checking, formatting, Rust style, repository architecture including OpenMLS execution, and diff checks all passed on retry.

The earlier formal suite subsequently ended after exhausting its aggregate one-hour environment wait. It retained six measured startup runs, six measured disconnected idle runs and two measured connected idle runs. There were no started-workload failures; text was not reached. The original suite and its failure remain unchanged.

A fresh suite under `target/benchmark-reference-v2-cpu-fixed-2026-11-03` uses a frozen copy of the validated executable and runner. It measures all four scenarios with one warmup and six measured runs each. Every run now records the normalized CPU format and Mach timebase. The retained CPU acceptance, binary provenance and suite configuration identify these inputs. `completion.json` records eventual suite and analysis completion; `verified-report.md` is generated only after all 28 run manifests, cleanup receipts and CPU conversions pass. This is a running measurement, not a completed or approved baseline.

Validation receipts: `target/benchmark-cpu-units-fixed-retry`, `target/benchmark-cpu-fixed-admission-retry`, and `target/benchmark-cpu-fixed-*-retry.log` / `.json`.

## Complete normalized-CPU reference result

The fresh suite completed on 2026-11-03 at 11:53 Asia/Shanghai, after approximately 53 minutes. All four scenarios completed six measured runs plus one warmup each. All 28 manifests, normalized CPU conversions, correctness results and cleanup receipts passed. Each of the six measured text runs synchronized 200 entries and verified persistent history after receiver restart. One admission deferral waited approximately 31 seconds for competing builds; no failed samples were filtered.

Evidence: `target/benchmark-reference-v2-cpu-fixed-2026-11-03/completion.json`, `verified-report.md`, `verified-summary.json`, `verification-manifest.json`, and the immutable per-run artifacts under `results/`. The final verification manifest and all cleanup/text results were rechecked before reporting completion.

Per-endpoint startup medians were approximately 263 and 272 ms. Disconnected idle RSS medians were approximately 29 MiB; connected idle approximately 33–35 MiB; text work approximately 34–36 MiB. Observed full-process peaks reached approximately 228 MiB, including initialization. These are headless connectivity-host measurements with the fixed Engine commit and test-host overlay recorded in provenance. They are not native-daemon or device measurements. No approved reference, numeric gate or optimization claim is implied.

## PR delivery verification

The final executable repeated the real two-text synchronization and receiver-restart acceptance before delivery. The six artifact/control cases passed again: A/A acceptance comparison, changed sample, missing sample, missing cleanup receipt, malformed response and oversized response. Evidence is under `target/benchmark-pr-acceptance` and `target/benchmark-pr-report-acceptance`. All 37 files in the complete reference verification manifest were rechecked.

The measured Engine revision remains `aa1b409fda38053d2d1e61e7801f680e2261ea26`. Later main-branch changes are outside that measurement. Local raw artifacts are retained, not committed or uploaded by CI. The first reference does not prove performance equivalence for subsequent Engine revisions.
