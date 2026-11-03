# Engine process benchmarks (macOS)

This harness measures the existing connectivity test host, including its four-worker Tokio runtime, `dev-tools` and local observability runtime. It does not measure a GUI, native clipboard integration, or an isolated Core runtime. Only macOS resource measurement is implemented.

## Build and run

Use Python 3.11 or later. Run from the repository root. Initialize the pinned network submodule first. Keep the repository `target` and existing MBX/sccache configuration.

```sh
python3 scripts/testing/build-engine-benchmark.py --baseline origin/main --isolation-root /path/to/external/benchmark-builds --output target/benchmark-binaries
python3 tests/benchmarks/tests/acceptance.py --binary target/benchmark-binaries/uc-benchmark --host target/benchmark-binaries/candidate-host --output target/benchmark-acceptance
python3 scripts/testing/run-engine-benchmark.py --harness target/benchmark-binaries/uc-benchmark --baseline-host target/benchmark-binaries/baseline-host --candidate-host target/benchmark-binaries/candidate-host --output target/benchmark-suite
```

Additional artifact/control acceptance:

```sh
python3 tests/benchmarks/tests/report_acceptance.py --binary target/benchmark-binaries/uc-benchmark --acceptance target/benchmark-acceptance --output target/benchmark-report-acceptance
```

Every output directory must be new. A baseline build uses an immutable commit and the same current host source as the candidate. The build driver creates a detached baseline worktree under the explicitly selected external isolation root. It builds serially through `just cargo`, shares MBX, copies executable/provenance, and reclaims that worktree and its dedicated target in `finally`. The candidate uses its existing target. This respects host storage guards that forbid sharing one target between worktrees. It never changes the release profile or disables the compiler cache.

A developer acceptance run does not require a binary manifest. It records `acceptance_only: true` and permits a busy machine; it cannot establish a reference baseline:

```sh
target/benchmark-binaries/uc-benchmark self-test --host target/benchmark-binaries/candidate-host --output target/benchmark-smoke --case smoke
```

Formal suite runs require binary provenance manifests. Dirty candidate sources are recorded and must not be represented as an approved reference baseline.

## Workload and measurement

Workload version 2 covers startup, disconnected idle, connected idle and text synchronization. It runs one warmup per revision followed by three ABBA blocks (six measured runs per revision). Every run uses fresh independent profiles, in-memory secure storage and synthetic clipboard adapters. A local rendezvous fixture mediates pairing; Engine uses its real Iroh data path. Public relay fallback is disabled.

Startup is process creation through the host `ready` response. That response precedes Space creation/unlock. This is a fresh-profile process startup, not an OS cache-cold startup. Sampling begins after spawn; very early allocation spikes can be missed.

Idle scenarios warm up for 90 seconds after setup and measure for 60 seconds. Text synchronization also uses the 90-second warmup. Every initialization and warmup sample remains in the raw stream. `warmup_start` and `work_start` separate initialization retention from steady work. Text uses 100 unique 100-byte strings and 100 unique 10-KiB strings. Dataset v1 is a deterministic ASCII generator with prefix `benchmark-374-v1-`, a six-digit sequence and `x` padding. Seed 374 identifies this fixed generator. Its high compressibility is part of the workload, not representative of every clipboard payload.

Only one text is outstanding. Injection emits a bounded `HostClipboardChangeStream` notification; the normal listener performs capture and automatic synchronization must produce readable matching content at the peer before the next notification. Final paginated content verification checks the exact multiset and count. The receiver restarts and reads the same persisted history before success. This restart is outside resource sampling. Text runs include a 30-second recovery window.

The collector is a separate process. It uses macOS `proc_pid_rusage(RUSAGE_INFO_V2)`. Resident bytes, physical footprint, user time and system time retain their OS field names. Raw CPU counters use Mach absolute-time units. The collector records `mach_timebase_info` and converts their sum to `cpu_time_ns` with a 128-bit intermediate. Sampling is 1 Hz for idle and 20 Hz for startup/text. Monotonic timestamps permit phase attribution. The sampled maximum is an observed maximum, not a proven absolute peak. No portable interpretation of private memory is claimed.

Apple's [XNU rusage implementation](https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/kern/bsd_kern.c) copies task CPU counters without nanosecond conversion. Collectors preceding `cpu_time_format: mach-timebase-ns-v1` mislabeled these counters. Their CPU summaries are invalid. Keep their raw artifacts unchanged and derive corrected analysis from `ri_user_time`, `ri_system_time` and separately recorded host timebase evidence. Do not compare their legacy CPU columns. Validate the current collector against an independent child CPU clock:

```sh
python3 tests/benchmarks/tests/cpu_time_acceptance.py --binary target/benchmark-binaries/uc-benchmark --output target/cpu-time-acceptance
```

Controller completion time includes bounded status queries and JSON pipe traffic. It is not a pure Engine synchronization latency. Do not derive business P95 latency from it. Resource summaries of the entire sample stream include initialization; use phase windows to distinguish steady idle from setup.

## Failure ownership and resource bounds

The supervisor owns a dedicated worker process group, the profile lease, exclusive run lock and final artifact. The worker owns at most two Engine processes and two collectors. Each protocol reader retains at most one 64-KiB response. Text queries return at most 32 entries; the dataset retains at most 200 digest records. Samples stream to disk. A run has a 15-minute deadline; requests have a 30-second deadline.

On normal completion the worker closes collectors, shuts down Engine and waits for process exit. On failure or interruption the supervisor terminates its own group, escalates to kill, verifies disappearance and deletes the profile lease. Cleanup failure is a failed result. If the supervisor is killed, pipe EOF makes the admitted worker terminate its private group. A private cleanup lease allows the next supervisor to reclaim the generated profile after checking its ownership marker and confirming that the recorded process group no longer exists. A live or ambiguous group blocks recovery. System crashes and power loss cannot guarantee immediate cleanup; do not infer successful cleanup without a receipt.

Failures are retained, never replaced by reruns. A workload failure stops later runs. Formal runs wait between measurements when builds or simulator devices are active. Aggregate environment waiting is bounded to one hour by default. A preflight refusal with no run directory and a confirmed busy environment is recorded as an admission deferral; no measured sample exists to replace. Successful runs and their order are preserved. Sampling failures invalidate performance comparison. Raw control responses, keys, invitations, profiles, logs and clipboard plaintext are not copied to artifacts. Secure storage crosses private pipes only in memory during the persistence check.

## Evidence

Each run contains environment metadata, two JSONL sample streams, work correctness evidence, a final result, uc-testkit cleanup evidence and SHA-256 manifest. `uc-benchmark verify --output RUN` validates the manifest and successful result. The suite checkpoints machine-readable progress and a Markdown report after every completed run. Failures retain partial summaries. Performance deltas never act as numeric gates.

Keep the machine free of builds, simulators and unrelated load for reference measurements. A per-run advisory lock prevents concurrent harness runs. Preflight rejects detected Cargo/rustc/Xcode builds, Android emulator processes and booted iOS simulator devices. An idle Simulator window does not block a run. It cannot reserve the whole host against unrelated tools starting later. Record power and background-load snapshots and review them before approving a reference.

Linux, Windows, device integration, heap profiling, long soak tests and CI scheduling are not accepted by these local macOS results.

Use `run-engine-benchmark.py --acceptance-only` only to validate suite mechanics on a busy host. These results retain the same real Engine workloads and ABBA repeats, but are explicitly ineligible as a reference baseline.

## Reference-only measurement and idle retention

When only one immutable Engine revision needs its first baseline, omit `--candidate-host` and select `--reference-only`:

```sh
python3 scripts/testing/run-engine-benchmark.py --harness target/benchmark-binaries/uc-benchmark --baseline-host target/benchmark-binaries/baseline-host --reference-only --output target/benchmark-reference
```

This runs one warmup plus six measured runs per scenario. It emits one reference column, not a synthetic version comparison. The paired mode retains ABBA ordering.

Version 1's 10-second warmup captured freed KDF allocations temporarily retained by the macOS allocator. A live process showed 192 MiB in `Malloc Large (empty)`; its live allocated heap was approximately 5.3 MiB. The two initialization KDFs use 128 MiB and 64 MiB. When macOS reclaimed these empty regions, footprint fell from approximately 205 MiB to 12 MiB. Across the captured runs, reclamation occurred around 30–61 seconds after setup began. Version 2 uses a fixed 90-second warmup. Full-process peaks continue to include initialization. A fixed warmup does not force allocator reclamation; report retained pages and between-run dispersion rather than assuming every work window reaches the lower footprint. This changes the measurement window, not Engine allocation or cryptographic settings.

Run the diagnostic and admission E2E with the built executable:

```sh
python3 tests/benchmarks/tests/idle_retention_diagnostic.py --binary target/benchmark-binaries/uc-benchmark --host target/benchmark-binaries/baseline-host --output target/idle-diagnostic
python3 tests/benchmarks/tests/suite_admission_acceptance.py --binary target/benchmark-binaries/uc-benchmark --host target/benchmark-binaries/baseline-host --output target/suite-admission-acceptance
```

The idle diagnostic's 64-MiB within-window span identifies the observed symptom. It is not a resource budget or a CI gate. Diagnose allocator retention with the complete time series and OS footprint/heap evidence before claiming a leak or optimization. A stable work window does not erase the initialization peak.
