# Why I Chose micropool for Chunk Worker Jobs

**Document type:** Architecture Decision Record / Devlog Entry  
**Status:** Accepted  
**Date:** 2026-11-15  
**Project:** Valescent Engine  
**Owner:** Project Owner  
**Supersedes:** None  
**Scope:** The worker pool used for chunk generation, lighting, and meshing from v0.0.5 onward. The `pool_sim` benchmark methodology, queue priorities, upload budgets, and GPU work are described by the milestone specs and are not re-decided here.

---

## Summary

This document records why the engine uses micropool for chunk worker jobs instead of rayon. It is both an official decision record and a devlog entry: the decision is final for the current scope, and I am publishing the reasoning so future decisions can be traced back to it.

The short version: `pool_sim` ran both backends on the same machine, with identical workload checksums and every scenario under the 60 FPS budget. Micropool was slightly better on most default frame percentiles and, more importantly, had a much better saturated foreground-join and frame tail. The result is close enough that the choice is about tail behaviour, not raw throughput. The tradeoff is that micropool is a smaller, less widely used crate than rayon and its advantage is machine- and run-specific.

---

## 1. Context

### 1.1 The project

v0.0.5 moves chunk generation, lighting, and meshing onto workers:

- Jobs are coarse: one chunk generation, one light job, or one chunk mesh.
- Workers receive immutable snapshots and return data through channels.
- The main thread applies world mutations and GPU uploads.
- Edits are urgent; background LOD and streaming work is lower priority.
- The pending queue is bounded and background work can be dropped.
- The main thread must not wait for a routine chunk mesh.
- The pool must support thousands of coarse jobs without starving the frame loop.

The benchmark also simulates a foreground `join` for an urgent edit. That join is a worst-case
stress path, not the normal no-wait design, but its tail is the clearest proxy for scheduler
contention during a hitch-sensitive frame.

### 1.2 The constraints

My choice is constrained by the following:

- Coarse jobs, not fine-grained parallel iterators.
- Bounded queues and result channels.
- Urgent foreground work must not be starved by background work.
- Frame-tail latency matters more than average throughput.
- The pool must be small enough to wrap and pin.
- It must support owned jobs with `'static` data.
- It must not touch the GPU or mutable world state.
- The dependency must be replaceable behind a narrow engine-side interface.

---

## 2. Decision

**micropool is the worker pool for chunk generation, lighting, and meshing.**

When v0.0.5 is implemented, `micropool` moves from a development dependency to a normal
dependency. `rayon` remains a development dependency for `pool_sim`, so the comparison can be
repeated.

The engine keeps a narrow pool interface around submit/result channels. The concrete pool is not
exposed to the mesher, lighting, or generation code. If the assumptions below stop holding, a
later ADR can replace micropool without changing job payloads.

The default worker count is whatever micropool selects for the host; on the benchmark machine
that was 11 workers on a 12-thread CPU. The main thread keeps its own capacity; the pool is not
forced to occupy every logical CPU.

I accept this decision for the current scope. I will revisit it only if the assumptions in this
document stop holding.

---

## 3. Why I Chose micropool

### 3.1 It wins the saturated tail

The default run used rayon with 12 workers and micropool with 11. In the saturated scenario,
micropool's frame p99 was 2,662 µs versus rayon's 2,753 µs, and its join maximum was 1,508 µs
versus 1,624 µs. A second default run showed micropool's saturated p95/p99 at 2,452/2,519 µs
versus rayon's 2,701/2,728 µs.

The matched 12-worker run made the tail difference clearer: rayon's saturated frame
p95/p99/join-p99 were 6,827/8,259/6,995 µs; micropool's were 3,935/4,666/2,875 µs. That is the
kind of tail that becomes a visible hitch.

### 3.2 One fewer worker still competes

Micropool's default pool was 11 workers, one fewer than rayon's 12, yet it was competitive or
better on most default frame percentiles. That leaves the main thread more room and avoids
assuming that every logical CPU belongs to the pool.

### 3.3 Coarse jobs do not need work-stealing

The engine submits large chunk jobs with owned inputs and receives large mesh results. It does
not need rayon's parallel iterators or fine-grained work stealing. Micropool's smaller model is
sufficient for the queue shapes the engine actually uses.

### 3.4 Owned jobs fit the pipeline

`spawn_owned` matches the immutable chunk snapshots and owned result payloads the worker pipeline
uses. Jobs do not borrow world state, so the queue can outlive the caller's stack frame.

### 3.5 The workload was identical

`pool_sim` reported identical workload checksums for both backends in every scenario. The
comparison is a scheduling comparison, not one backend quietly doing less work.

### 3.6 All scenarios fit the frame budget

Every scenario in all three runs had zero over-budget frames. Both backends are viable. The
choice is therefore a narrow tail-latency preference, not a correctness or budget failure.

### 3.7 Why not rayon?

Rayon is the more mature choice, has a broader ecosystem, and was better in some light-load
tails. It was not selected because its saturated frame and foreground-join tail was repeatedly
worse in the runs that stress the pool, and the engine does not need rayon's parallel iterator
surface. If the queue model changes to rely on fine-grained parallel work, rayon becomes worth
revisiting.

---

## 4. Benchmark

**Machine:** AMD Ryzen 5 7600, 6 cores / 12 logical processors, Windows.  
**Toolchain:** Rust 1.99.1.  
**Command:** `cargo bench --bench pool_sim -- both` (default worker counts; micropool 11,
rayon 12).  
**Budget:** 16,667 µs per simulated frame. Times are frame work time in µs. All scenarios had
`over = 0` and identical workload checksums.

| Scenario | Backend | p95 | p99 | max | join max |
|---|---|---:|---:|---:|---:|
| idle | rayon | 2,727 | 2,839 | 2,893 | 1,595 |
| idle | micropool | 2,660 | 2,772 | 2,910 | 1,518 |
| light | rayon | 2,809 | 2,895 | 3,077 | 1,602 |
| light | micropool | 2,724 | 2,849 | 2,995 | 1,512 |
| loaded | rayon | 2,808 | 2,911 | 3,318 | 1,582 |
| loaded | micropool | 2,754 | 2,873 | 2,990 | 1,550 |
| saturated | rayon | 2,457 | 2,753 | 2,781 | 1,624 |
| saturated | micropool | 2,471 | 2,662 | 2,830 | 1,508 |

Repeat observations:

- A second default run kept micropool ahead in the saturated scenario: p95/p99 were
  2,452/2,519 µs versus rayon's 2,701/2,728 µs.
- That repeat also produced a micropool light-load outlier: p99/max were 3,264/6,497 µs versus
  rayon's 2,924/3,048 µs. The choice is not a claim that micropool wins every percentile in
  every run.
- The matched 12-worker run confirmed the saturated tail difference: rayon
  p95/p99/join-p99 6,827/8,259/6,995 µs versus micropool 3,935/4,666/2,875 µs. The matched run
  also had a micropool light p99 outlier, so the benchmark remains noisy outside saturation.

`pool_sim` is a decision aid, not a regression gate. It is not run in CI. Re-run it on the target
machine before changing the pool or its worker count. Ongoing results are recorded in
[Benchmark results](../dev/benchmarks.md).

---

## 5. Alternatives Considered

| Alternative | Assessment | Outcome |
|---|---|---|
| **micropool** | Competitive default percentiles, one fewer worker, better saturated frame/join tail, owned jobs, small API. | **Selected** |
| **rayon** | Mature, broad ecosystem, work-stealing and parallel iterators; worse saturated/join tails in the runs, unnecessary fine-grained surface. | Not selected; retained for `pool_sim` |
| **Custom engine pool** | Full control over queues and priorities; more concurrency code to own and test. | Not selected at this scale |
| **One thread per job** | Simple submission; unbounded thread creation and context-switch cost under bursts. | Not selected |
| **Async runtime (Tokio)** | Good for I/O and timers; the jobs are CPU-bound and the engine already has a synchronous frame loop. | Not selected |
| **Rayon with a custom thread count** | Matched-thread run still showed the saturated tail; does not remove the pool-model difference. | Not selected |

---

## 6. Counterarguments and Disposition

| Counterargument | Disposition |
|---|---|
| **The benchmark is noisy.** Light-load tails moved between runs. | Accepted. The decision is based on the repeated saturated/join tail, not a claim that every percentile favours micropool. Re-run before changing the pool. |
| **Micropool is smaller and less widely used than rayon.** | Accepted. It has a narrow API, is pinned, and sits behind an engine-side interface. If maintenance or correctness becomes a problem, a later ADR can switch to rayon without changing job payloads. |
| **The default pool uses one fewer worker.** | Accepted. It still competed with rayon's 12 workers and leaves logical CPU for the main thread. A matched 12-worker run still favoured micropool under saturation. |
| **Rayon was better in some light/loaded tails.** | Accepted. The difference was small and inconsistent. The saturated tail is the hitches that matter more to the player. |
| **The foreground join is a worst-case simulation.** | Accepted. The normal v0.0.5 path avoids main-thread waiting, but urgent edits can still contend for pool capacity; the join tail is a useful stress signal. |
| **Micropool lacks work-stealing and parallel iterators.** | Accepted. The engine submits coarse chunk jobs; it does not need either feature. |
| **The result is machine-specific.** | Accepted. The ADR records the hardware and commands; the decision should be re-measured on a materially different target. |
| **Moving a dev dependency into runtime dependencies adds weight.** | Accepted. It is one small crate, pinned, and the alternative is a larger runtime dependency with a worse tail in the measured workload. |

---

## 7. Consequences

### 7.1 Positive

- Urgent joins and the saturated frame tail are lower in the measured workload.
- The default pool leaves one logical CPU for the main thread.
- The engine avoids rayon's parallel-iterator surface it does not use.
- Jobs use owned snapshots and results, matching the v0.0.5/v0.0.7 pipeline.
- The pool is replaceable behind a narrow interface.
- `rayon` remains available for repeating the benchmark.

### 7.2 Negative

- The engine depends on a smaller, less widely used pool crate.
- The performance advantage is narrow and machine-specific.
- The default worker count may not suit every host; it should be measured.
- `rayon` stays in the development dependency tree for `pool_sim`.
- The team must keep the pool interface isolated if the choice is revisited.

### 7.3 Neutral

- `pool_sim` remains the decision aid, not a CI gate.
- Worker count and queue capacity remain implementation details.
- A future ADR can replace the pool without changing generation, lighting, or mesh payloads.

---

## 8. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| Benchmark results do not reproduce on another host. | Re-run `pool_sim` with both backends on the target machine; treat the ADR as a decision record, not a universal benchmark claim. |
| Micropool becomes unmaintained or has a correctness bug. | Pin the version; keep it behind a small engine interface; keep the rayon comparison and a new ADR as the fallback. |
| The default worker count underuses or oversubscribes the host. | Measure on the target; expose worker count as a later configuration decision if needed. |
| Saturated queues drop background work. | Keep the bounded queue, background-drop policy, and queue-depth metrics from v0.0.5. |
| The main thread still waits on an urgent mesh. | Keep the no-wait design; use revisions and upload budgets; treat the benchmark join as a worst-case stress path. |
| A future parallel workload needs work stealing or parallel iterators. | Revisit the pool with a new ADR; rayon remains the benchmark comparison and a viable fallback. |
| The benchmark result is over-read as a regression gate. | The ADR and conventions state that `pool_sim` is a local decision aid and is not run in CI. |

---

## 9. Conclusion

micropool is my measured, tail-focused choice for chunk worker jobs. I am selecting it because
it matched or beat rayon on the default frame percentiles, because its saturated frame and
foreground-join tails were repeatedly better, and because it supports the owned coarse jobs the
engine actually submits without rayon's wider parallel surface. I acknowledge that the
difference is narrow, that the benchmark is noisy and machine-specific, and that micropool is
the smaller dependency. I manage those through a pinned version, a narrow pool interface,
repeatable local benchmarks, and a documented fallback to rayon. I therefore accept this
decision as the engine's worker pool, with the benchmark evidence and its limits recorded above.

---

*This entry is part of the Valescent Engine decision log. Future entries should reference this document when they depend on the chunk worker pool or revise its assumptions.*
