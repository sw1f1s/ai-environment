# Semantic graph QA

The independent integration suite uses actual libclang and Roslyn processes on the sources in `tests/fixtures/semantic`. It does not substitute mocked symbol or edge responses. It parses programs without building or running the target applications. The C# fixture deliberately contains an MSBuild target that writes a marker and fails; semantic indexing must succeed without executing it.

| Coverage | Independent assertions |
|---|---|
| C | Cross-translation-unit direct calls, header declaration/definition deduplication, struct versus typedef identity, named alias/type dependencies. |
| C++ | Separate overload IDs and exact selected call targets, namespace decoy exclusion, concrete template call, alias and inheritance edges, virtual compile-time target without invented override calls. |
| C# | Distinct original overload IDs, cross-file calls, alias targets, generic field argument types, class/interface inheritance and virtual/interface compile-time dispatch. |
| Authoritative inputs | Different request text changes compiler call edges while disk sources stay unchanged, for both Clang overlays and Roslyn trees. |
| Source coordinates | UTF-8 byte extents after non-ASCII prefixes and CRLF; paginated selected bodies retain every Unicode character without duplication. |
| Cache and projection | Unrelated documentation preserves the semantic source key but updates the apply snapshot; relevant type changes invalidate old facts; refresh retains canonical declaration IDs; stale cursors are rejected. |
| External dependencies | Compile database directory resolves relative includes containing spaces; actual database/header/compiler/reference bytes are hashed; changed external header invalidates the graph while project source hash stays unchanged. |
| Compiler bootstrap | An invalid shared compiler library reports its cause, exits nonzero and leaves the previous real graph byte-identical. |
| Explicit limits | Missing headers/types and indirect/dynamic calls remain partial with diagnostics/unresolved entries; unsupported language cannot silently become a lexical graph. |

`Ledger` in the C fixture is intentionally both a struct tag and a typedef. The tests distinguish their kinds and require the alias edge, rather than merging two compiler entities because their printed names match. Overload IDs are treated as opaque compiler identities. The tests inspect the versioned cache only for exact node/edge facts; public CLI find/context/impact and cursor behavior are exercised separately. Related contract cards are deduplicated, source bodies are opt-in, and unrelated function bodies are not exposed.

Run from the repository root after the native CLI and managed helper are built:

```sh
AI_ENV_REQUIRE_SEMANTIC_BACKENDS=1 cargo test --locked --offline --test semantic_graph -- --nocapture
```

The strict environment flag makes missing adapter artifacts a failure. Without it, developer environments can explicitly skip unavailable adapters; such a run is insufficient evidence that all three languages passed. `semantic-real` log lines report the actual compiler and graph counts. Python analyzer suites provide additional backend-specific tests:

```sh
python3 -m unittest discover -s tests -p 'test_*_analyzer.py' -v
```

The compiler model is static. A virtual edge names the compiler-selected declaration, not every runtime implementation. Unknown function-pointer/delegate/dynamic targets are unresolved. The C# adapter does not execute or fully reconstruct MSBuild, Unity, project references, source generators or arbitrary project analyzers. There is no claim of universal runtime behavior or formal security proof.

The reproducible byte probe is part of the same suite and can be run alone:

```sh
AI_ENV_REQUIRE_SEMANTIC_BACKENDS=1 cargo test --locked --offline --test semantic_graph semantic_context_size_probe -- --nocapture
```

It emits a `SEMANTIC_SIZE_PROBE` JSON record with source hashes, exact selector, compiler identity, every query's page count and total UTF-8 bytes, including repeated transport headers and continuation cursors. It follows every continuation to `end`. The selected symbol is the `warehouse::refund(Money, Money)` overload. Queries are:

```text
context --symbol ID --relations calls,callers,types --depth 1 --budget B
context --symbol ID --relations calls,callers,types --depth 1 --body --budget B
graph impact ID --depth 2 --budget B
context refund --budget 12000
```

Semantic queries use both the normal 12000-byte budget and an 1800-byte pagination stress budget. The probe asserts the selected overload still calls the integer overload and that `warehouse::run` remains its caller. It repeats the same questions after adding 100 independent namespaces/files, each containing 20 actual arithmetic functions. These are valid unrelated declarations, not padded comments, and the original selected facts must remain identical.

There are three source-size controls: all raw C++ source files, the three relevant original source files, and the actual lexical `refund` response. Raw matching-line bytes are also recorded. The full-project baseline is intentionally a whole-project reading baseline. An ordinary agent using `rg` also avoids unrelated files; this probe does not establish that AI Env outperforms such an agent. Lexical matches and compiler relations answer different questions, so their byte sizes are not an equal-information comparison.

These are byte counts, not tokenizer estimates, token savings, model latency, application performance or improved model decisions. A tiny fixture can cost more to describe semantically than to read in full. A larger whole-project ratio demonstrates selection behavior only; relevant-file and lexical controls must remain visible beside it.

## Interpretation

The probe reports source bytes, complete paginated response bytes and exact graph
facts. It includes whole-project, relevant-source and literal-search controls.
Small page budgets can increase repeated transport overhead. Reading the entire
project is not an equivalent baseline for an agent using ordinary targeted search.
These measurements validate bounded selection and pagination, not token savings,
fewer defects or improved project consistency. [Product verification](VERIFICATION.md).
