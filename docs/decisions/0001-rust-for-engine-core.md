# Why I Chose Rust for the Voxel Engine

**Document type:** Architecture Decision Record / Devlog Entry  
**Status:** Accepted  
**Date:** 2026-11-13  
**Project:** Voxel Engine  
**Owner:** Project Owner  
**Supersedes:** None  
**Scope:** Primary implementation language for the engine core. Tooling and web-facing components are out of scope and may use other languages.

---

## Summary

This document records why I am writing the voxel engine primarily in Rust. It is both an official decision record and a devlog entry: the decision is final for the current scope, and I am publishing the reasoning so future decisions can be traced back to it.

The short version: a voxel engine needs low-level control, predictable performance, and strong compile-time safety. Rust provides all three. TypeScript is not a low-level language and cannot serve as the engine core. Go is unfamiliar to me and is not my current target. I reject C-family languages and Java for their complexity and learning curve. I reject Python for its syntax and lack of low-level suitability.

This decision also reflects personal preference. I prefer bracket-based syntax, strict typing, and the discipline of borrow checking. I expect Rust’s strictness to frustrate me at times; I accept that cost because it enforces code health and catches problems before runtime.

---

## 1. Context

### 1.1 The project

The voxel engine is a large, performance-sensitive project. Voxel engines are demanding in ways that ordinary applications are not:

- **Large, dense data structures.** Voxel worlds are stored in chunks, and those chunks must be laid out in memory for fast iteration and cache efficiency.
- **Hot loops.** Meshing, lighting, and chunk generation run over millions of voxels. Small inefficiencies multiply.
- **Multithreading.** Chunk generation and meshing are typically parallelized. Shared state must be handled safely and predictably.
- **GPU interaction.** Vertex and index buffers must be built and uploaded efficiently, with tight control over memory layout.
- **Long-running simulation.** The engine must remain stable and predictable over time, not just correct on a small test case.

These requirements point toward a systems language with explicit memory control, strong concurrency safety, and minimal runtime overhead.

### 1.2 The constraints

My language choice is constrained by the following:

- It must support low-level, performance-critical work.
- It must provide strong compile-time safety and good tooling (IntelliSense, type checking).
- It must be a language I am willing and able to work in for a large, long-term project.
- It must not be a language I have already rejected for complexity or unfamiliarity.

---

## 2. Decision

**Rust is the primary implementation language for the voxel engine core.**

This includes chunk storage, meshing, world generation, simulation, and any other performance-critical or low-level systems. TypeScript remains acceptable for tooling, web-facing components, and other high-level areas where it is the better fit. I am not selecting Go. I am not selecting C, C++, Java, or Python for the engine core.

I accept this decision for the current scope. I will revisit it only if the assumptions in this document stop holding.

---

## 3. Why I Chose Rust

### 3.1 Low-level performance and memory control

A voxel engine lives or dies on memory layout and throughput. Rust gives me explicit control over data layout, ownership, and allocation without a garbage collector. I can store chunk data in contiguous arrays, iterate efficiently, and pass it to worker threads with predictable behavior. This is my core technical reason for the decision.

### 3.2 Concurrency safety for chunk work

Chunk generation and meshing are naturally parallel. Rust’s ownership model and `Send`/`Sync` traits make data races a compile-time error rather than a debugging nightmare. This matters directly for a voxel engine, where threading bugs are easy to introduce and expensive to diagnose.

### 3.3 Strict typing and code health

I value strict typing and already enforce it in TypeScript. Rust’s type system is mandatory rather than optional, which improves code health, catches entire classes of bugs at compile time, and supports strong IntelliSense and type safety. For a project of this size, I consider that discipline worth the cost.

### 3.4 Borrow checking as enforced discipline

I expect to like borrow checking because it keeps my code in check. It forces me to reason about ownership and lifetimes instead of papering over them. I also expect to get mad and exhausted by the strictness at times. I accept that tradeoff deliberately: short-term friction in exchange for safer, healthier code.

### 3.5 Syntax preference

I prefer bracket-based, block-style syntax over Python’s indentation-based structure. Rust’s syntax is closer to JavaScript and TypeScript, which I find easier to read. This is a personal preference, not a technical requirement, but it is a legitimate factor in a long-term project.

### 3.6 Avoidance of C-family and Java complexity

I do not prefer C-family languages or Java because of their complexity and learning curve. I am selecting Rust as my preferred low-level alternative despite having its own learning curve. Rust replaces some of the dangers of C and C++ with compile-time safety rather than runtime failure.

### 3.7 Why not TypeScript or Go?

- **TypeScript** is excellent for web and high-level application work, but it is not a low-level language. It cannot serve as the core of a voxel engine that needs explicit memory layout and predictable performance.
- **Go** is not selected because I am unfamiliar with it and it is not my current target. It may be simpler in some respects, but that is not sufficient reason for me to adopt it here.

---

## 4. Alternatives Considered

| Alternative | Assessment | Outcome |
|---|---|---|
| **Rust** | Low-level, fast, strict, strong compile-time safety, concurrency safety, bracket-based syntax, good tooling. | **Selected** |
| **TypeScript** | Excellent for web and high-level work; strictness is optional and can be bypassed with `any`; not low-level. | Retained for tooling and web-facing components; not selected for engine core |
| **Go** | Simpler in some respects, but unfamiliar to me and not my current target. | Not selected |
| **Python** | Readable to many, but I dislike indentation-based blocks and it is not suitable for low-level engine work. | Not selected |
| **C / C++** | Low-level and performant, but I consider the complexity and learning curve unacceptable. | Not selected |
| **Java** | Mature and garbage-collected, but I consider the complexity and learning curve unacceptable. | Not selected |

---

## 5. Counterarguments and Disposition

This decision has real weaknesses. I am recording them here rather than omitting them, so the record stays honest.

| Counterargument | Disposition |
|---|---|
| **“Rust looks like JavaScript” is only skin-deep.** Rust shares curly braces, `let`, and `async/await`, but ownership, lifetimes, traits, generics, and `Result`/`Option` make it much more demanding than the syntax suggests. | I accept this. I acknowledge the resemblance is superficial. My decision does not rest on syntax alone; it rests on low-level control, strict typing, tooling, and concurrency safety. |
| **Loving borrow checking in theory is not the same as enjoying it in practice.** Real projects involve lifetime annotations, async lifetimes, shared state, and code restructuring. | I accept this with qualification. I expect to like borrow checking because it enforces discipline, while also expecting frustration. I accept the tradeoff for code health and type safety. |
| **Rust’s strict typing will demand more than TypeScript.** TypeScript strictness is optional and escapable; Rust’s strictness is mandatory and includes ownership and lifetimes. | I accept this. I prefer compiler friction upfront to runtime bugs. I will monitor productivity cost during validation. |
| **Disliking C/Java complexity sits awkwardly with choosing Rust.** Rust replaces some C/C++ problems with a complex type system and borrow checker. | I accept this. I consider Rust’s complexity safety-oriented rather than undefined-behavior-oriented. I am not assuming Rust is the “easy” low-level option. |
| **Performance may not always be the real constraint.** Many projects are not bottlenecked by language performance. | I accept this. I am selecting Rust with performance in mind, not as a universal language for every task. TypeScript remains appropriate for high-level and tooling work. |
| **Readability is subjective, and Rust can be noisy.** Lifetimes, generic bounds, macros, and sigils can reduce readability. | I accept this. I still prefer bracket syntax personally, but I will judge readability on real Rust code rather than small examples. |

---

## 6. Consequences

### 6.1 Positive

- My engine core will have predictable performance and explicit memory layout.
- Data races in chunk generation and meshing become compile-time errors.
- Strict typing and strong tooling improve long-term maintainability.
- Borrow checking enforces discipline and reduces entire classes of runtime bugs.
- The language aligns with my personal preference, which matters for a long-term project.

### 6.2 Negative

- Rust’s strictness will sometimes cause me frustration and slow my iteration.
- The borrow checker will sometimes require me to restructure code rather than apply quick fixes.
- Compile times and development overhead may be higher than with TypeScript or Go for some tasks.
- Rust’s JavaScript-like syntax may create false expectations of JavaScript-like ease.
- Lifetime-heavy or generic-heavy code can become visually dense.

### 6.3 Neutral

- TypeScript remains in use for tooling and web-facing components.
- Go remains out of scope but is not permanently excluded if my requirements change.

---

## 7. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| Borrow-checker fatigue slows my progress. | I will build incrementally; learn ownership patterns early; prefer clear ownership models over premature complexity; and review code for unnecessary lifetime complexity. |
| My development speed is unacceptable compared with TypeScript. | I will prototype the engine core early; track development time; and keep TypeScript for tooling and high-level components. |
| Compile times impede my iteration. | I will use incremental builds, workspace organization, and targeted test builds. |
| I overuse Rust where performance is not required. | I will restrict Rust to low-level, performance-critical, or safety-critical components; and use TypeScript for high-level work where appropriate. |
| Lifetime-heavy code becomes hard for me to read or maintain. | I will simplify ownership where possible; document non-obvious lifetime decisions; prefer owned data early in development; and refactor when borrow-checker friction reveals design issues. |
| Go or another language later proves simpler for some needs. | I will keep Go out of current scope. I will re-evaluate only if Rust fails my requirements or a concrete requirement favors Go. |

---

## 8. Conclusion

Rust is my deliberate, preference-driven, performance-aware choice for the voxel engine. I am selecting it because the engine needs low-level control, predictable performance, concurrency safety, strict typing, and strong tooling. I expect to like borrow checking because it enforces discipline, while also expecting the strictness to be frustrating at times. TypeScript is not low-level enough for my engine core, and I am not selecting Go because it is unfamiliar to me and not my current target.

The strongest counterargument is that Rust’s JavaScript-like syntax can mask its true semantic complexity. I acknowledge that risk and manage it through the risks and mitigations above. I therefore accept this decision as my primary language choice for the voxel engine, with TypeScript retained where appropriate and with its limits recorded above.