# Product verification

Build the bundle, then run strict product checks from the repository root:

```sh
python3 scripts/dev.py build --offline
python3 scripts/dev.py check --offline
```

Offline mode requires cached dependencies. The check verifies pinned vendor
contents, Rust formatting, Python suites and all Rust targets. It requires actual
semantic backends: a missing analyzer fails rather than counting a skipped fixture
as successful language coverage. Host requirements are in the [README](../README.md).

The latest validated implementation passes **376 product tests**: 123 Python and
253 Rust. Counts describe mechanism coverage, not an independent coding study or
proof of better architecture, fewer agent mistakes or lower development cost.

## Context and semantic analysis

- Scoped source discovery, Git ignore rules, path validation, UTF-8 byte budgets,
  exact numbered excerpts, omissions and stale continuation rejection.
- Exact-identifier/path ranking, retained late evidence, local examples and related
  tests under tight budgets without silently widening scope.
- Real symbol/call/type relationships for C, C++, C#, Rust, Python, TypeScript and
  JavaScript, including overload identity, aliases and source coordinates.
- Authoritative source overlays, diagnostics, explicit unresolved relations,
  cache invalidation, metadata/dependency changes and runtime provenance.
- Analysis does not execute target modules, build scripts, generators, plugins or
  inherited Python startup hooks; marker tests exercise these boundaries.

[Semantic QA](SEMANTIC-QA.md) covers C/C++/C# and pagination/source-size probes.
[Additional-language QA](POPULAR-LANGUAGES-QA.md) covers Rust/Python/TS/JS.

## State, changes and execution

- Snapshot/file compare-and-swap, scoped claims, complete edit prevalidation and
  rollback for ordinary IO failures.
- Provider-before-consumer dependencies, read/write conflicts, adaptive single/team
  routing and independently persisted attempts/proposals.
- Interrupted apply recovery, retained sibling work, addressed repair and bounded
  repeated-failure handling.
- Original-revision binding and immutable-answer replay; model references resolve
  against saved source catalogs and current semantic contracts.
- Actual configured checks, bounded output/time, fresh proofs, stale assessment
  rejection and combined-source completion gates.
- Architectural advisories versus blockers, responsible-task repair and independent
  review in team mode; single QA avoids a mandatory extra reviewer.
- Native worker cancellation, concurrent-runner exclusion and durable usage limits.

Native integration tests use deterministic model responses with real Rust state,
source transactions and configured checks. They establish protocol/controller
behavior, not the quality of an unconstrained model's decisions.

## Limits

Snapshots cover indexed text and configuration. Omitted inputs and external
changes remain outside that guarantee. Checks run with ordinary user permissions.
Filesystem power-loss atomicity and autonomous worktree execution are outside the
MVP. A fresh graph is still static and may be partial; a fresh note can contain
incorrect reasoning. Passing tool tests cannot establish universal correctness.

[Architecture](ARCHITECTURE.md) · [Targeted verification](TARGETED-VERIFICATION.md) ·
[Evaluation method](EVALUATION.md).
