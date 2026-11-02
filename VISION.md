# UniClipboard Engine Vision

> UniClipboard Engine should be nearly invisible when idle, efficient when active, and predictable under sustained workloads.

## Purpose

UniClipboard Engine runs in the background across macOS, Windows, Linux, iOS, and Android.
A user can copy content dozens of times each day while the Engine remains active for hours or weeks.
Low memory use, low idle CPU use, and stable resource use are core design principles.
Rust memory safety alone does not establish these properties.

The Engine balances resource use, synchronization latency, and reliability.
Resource use must be measurable and predictable.
Performance is a continuously measured property of the Engine, not an occasional optimization activity.
An optimization must preserve data security, synchronization correctness, and recovery guarantees.
A lower RSS does not justify an unexplained increase in CPU use, disk I/O, energy use, or latency.

This document defines design goals. It does not report current performance or certify the current implementation.
Development requirements belong in [AGENTS.md](AGENTS.md).
The current architecture belongs in [ARCHITECTURE.md](ARCHITECTURE.md).

## Resource goals

The initial direction targets approximately 20 to 30 MB of idle memory while preserving latency and reliability.
The following budgets are challenges for a standalone Engine or daemon process.
They exclude a host WebView and tray GUI.
They are not release gates or claims about the current dependency set.

| Scenario | Initial challenge |
| --- | --- |
| Initialization complete | RSS at most 20 MB |
| Stable idle state | RSS at most 25 MB |
| Multiple devices online without synchronization work | RSS at most 35 MB |
| Ordinary text synchronization | RSS at most 40 MB |
| Large image or file synchronization | Peak RSS increase at most 16 MB above the same connected idle baseline |
| Seven days of operation | No sustained upward resource trend under a bounded, repeatable workload |

Use decimal MB for these budgets. Record exact byte counts in measurement artifacts.
The image and file challenge includes the complete Engine pipeline, including image decoding when required.
A codec exception needs measured evidence and a documented decision. It does not count as meeting the challenge.
The workload must define content sizes, device counts, concurrency, and history size before a comparison is valid.

Establish separate baselines for each platform and process model.
Resident Set Size (RSS), shared pages, and private memory have different accounting rules across platforms.
For an embedded Engine, measure the host baseline and the added Engine cost separately.
Do not apply standalone budgets directly to the whole host process.
Do not compare different memory metrics as if they were equivalent.

Track more than RSS:

| Metric | Purpose |
| --- | --- |
| Idle CPU time and wakeups | Detect work that continues without a user action |
| Proportional Set Size (PSS), where available, and private memory | Separate shared memory from private process cost |
| Heap size, allocation rate, and retained capacity | Locate allocations and explain memory retention |
| Thread and task counts | Detect accumulation and excess background work |
| Open handles, file descriptors, and connections | Detect resources that do not close |
| Peak memory | Bound transient costs during large transfers and conversions |
| Disk I/O, energy use, and synchronization latency | Detect costs shifted away from memory |
| Binary size | Track distribution size and dependency cost |

This vision does not yet set a numeric idle CPU budget.
Define the sampling interval, normalization, and acceptable wakeup rate from platform measurements before setting a gate.

## Architecture direction

Use a minimal core, services that activate when needed, and bounded resources.
Keep configuration, lifecycle control, and essential event dispatch resident.
Activate expensive capabilities when work requires them.
Release temporary buffers and task state after completion, failure, or cancellation.
Query history in pages instead of retaining complete history in memory.

Availability does not require a permanent worker or a large resident buffer for every capability.
However, required connections and platform listeners must preserve discovery, synchronization latency, and platform lifecycle rules.
Do not close connections solely to improve an idle measurement.

Bound total retained resources, not only individual operations.
A resource bound includes active work, waiting work, payload bytes, retries, and the number of owners.
A per-device limit alone does not bound the Engine when the device count grows.
Memory use must not grow without a defined limit as time, history, or synchronization counts increase.

Large payloads should pass through bounded buffers from source to destination.
Compression, authenticated encryption, queues, and receiving writes must preserve that bound.
Chunking requires a protocol design that authenticates order, completion, and integrity.
Image conversion can require a complete decoded image.
Bound decoded pixels and conversion concurrency separately from transfer buffers.
Temporary files must obey the existing encrypted-persistence rules.

Choose the runtime, worker counts, dependencies, and allocator behavior from measurements.
Keep CPU work off latency-sensitive asynchronous and platform threads.
A container type, shared pointer, runtime flavor, database replacement, or release profile is not a performance result.
Compilation options can change binary size without reducing retained heap memory.
Any change to panic behavior must preserve the agreed failure and recovery contract.
Retain separate debug symbols when stripping release artifacts.

## Repeatable resource acceptance

Use end-to-end workloads to establish baselines and detect regressions.
The initial scenario set is a validation target, not an existing test suite:

| Scenario | Required observations |
| --- | --- |
| Idle for 24 hours | CPU, wakeups, memory, threads, tasks, and handles remain stable |
| 10,000 text synchronizations | Tasks, queues, and retained memory do not accumulate |
| 1,000 image synchronizations | Conversion buffers and workers release resources after each batch |
| 100 device reconnections | Old connections and their tasks terminate |
| A 1 GB file transfer | Peak memory stays within a declared bound and received content matches the source |
| Seven days of sustained operation | Resource trends remain bounded across repeated work and idle recovery periods |

Include cancellation, failure, retry, and shutdown in workload variants.
Define payloads, topology, rates, warm-up, sampling, and cooldown before running each scenario.
Inspect the time series, peaks, and recovery after work.
A start/end RSS comparison alone cannot establish stability.
Distinguish live objects, bounded cache retention, allocator retention, and operating-system accounting before claiming a leak.

Each run must produce a verifiable and repeatable artifact.
Record the source revision, dirty state, build profile, enabled features, platform, hardware, and dependency lock identity.
Include the workload command, synthetic fixtures or fixture-generation seed, measurement tools, and sampling rules.
Retain timestamped samples, peak values, trend analysis, correctness checks, and the final result.
Keep credentials and user content out of artifacts.
Record unexecuted scenarios as skipped.

Establish baselines before enforcing numeric gates in CI.
Compare the baseline and candidate under the same conditions.
Use repeat runs to distinguish a regression from measurement noise.
Define platform-specific tolerances and the responsible owner before enabling a gate.
Run short resource scenarios in CI when practical.
Run long scenarios in a dedicated environment with retained artifacts.
Neither documentation nor a benchmark command proves that these checks already exist.

## Delivery priorities

Establish measurement and evidence collection from the first resource change.
Then prioritize implementation work in this order:

1. Define ownership, lifetimes, and aggregate bounds for caches, queues, transfers, connections, and workers.
2. Reduce idle runtime cost, thread count, keepalive work, and unnecessary wakeups.
3. Remove measured duplicate allocations for images, files, compression, and encryption.
4. Review dependency features and release configuration with measured comparisons.
5. Expand baseline coverage, CI regression gates, and long-duration testing.

Prefer a measured improvement that preserves the full synchronization contract.
Do not select an arbitrary memory number at the expense of reliable background operation.
