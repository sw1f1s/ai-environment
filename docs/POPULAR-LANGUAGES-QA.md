# Additional language adapters

Rust, Python, TypeScript and JavaScript use the existing graph build/find/context/impact protocol alongside C/C++/C#. These checks exercise actual rust-analyzer, Jedi and TypeScript Compiler API results, not mocked semantic edges.

| Language | Backend | Checked behavior | Coverage limits |
|---|---|---|---|
| Rust | rust-analyzer | Cross-module/aliased calls, receiver/trait target, generic/field/type/alias edges, self-calls, local path crates, supplied source/hash, exact CRLF/UTF-8 extents, selected-file extraction | Isolated static Cargo/crate graph; no registry dependency installation, build scripts or proc-macros; dynamic trait implementations and unknown targets remain partial |
| Python | Jedi 0.20.1 + Parso 0.8.7 + AST | Import aliases, inferred receivers, inherited methods, field/generic annotations, recursive calls, decorated declarations, supplied source over stale disk, exact source spans and retained external inputs | Always partial; dynamic runtime mutation, custom source roots, packaging and dependency installation are not reconstructed |
| TypeScript | TypeScript Compiler API 5.9.3 | Resolved overloads, aliases, generics/fields, inheritance/interfaces, recursion, tsconfig extends/paths, source overlays | No plugins or project-reference builds; ambiguous callbacks/dynamic targets unresolved; type traversal limits explicitly partial |
| JavaScript | Same Compiler API | Checked JSDoc types, module/receiver calls, per-file implicit owner for top-level calls, exact source offsets | Static inference does not establish exhaustive runtime behavior |

The independent public-CLI suite in `tests/popular_graph.rs` creates separate four-module applications. For every new language it requires the exact `run -> credit` call through an import alias, a dependency on `Money`, caller/impact selection, opt-in body text, UTF-8/CRLF extents and stable declaration IDs after body-only edits. It verifies that unrelated docs keep the graph fresh, while metadata/source edits reject old facts and refresh restores access.

Additional CLI regressions reject compiler flags from a different language, invalidate context when an explicit runtime override changes, and prove that an inherited project `PYTHONPATH`/`sitecustomize.py` cannot run during analysis. Backend tests also place execution markers in Python modules/setup files, Rust build scripts and TS/JS modules/plugins. Analysis leaves those markers absent.

Independent review found and corrected transient Python snapshot paths retained as dependencies on macOS, silently omitted self-recursion in Rust/TS, an unreported TS type-traversal limit, ignored Rust file selection, and inherited Python startup settings. Cache validation rejects transient/nonexistent provenance rather than publishing an apparently fresh graph.

## Reproduce

The host needs Python, libclang, .NET 8, Node.js and rust-analyzer/rust-src. Build the bundle and make the host analyzers available before running checks. `scripts/dev.py` detects trusted tools, records the machine-specific runtime profile and enables strict real-backend coverage:

```sh
python3 scripts/dev.py check --offline
```

To run the public-CLI checks alone after preparation:

```sh
cargo test --locked --offline --test popular_graph
```

The packaged skill carries pinned Jedi/Parso/TypeScript code and upstream licenses. Node.js, Rust tools/sysroot, libclang and .NET are external host runtimes. `scripts/bin/runtimes.json` is local build output, not a portable dependency lock. After moving the skill to another machine, configure its installed runtime paths or rebuild there; queries do not download or install anything.

These are functional analyzer/tool checks. This stage does not measure model tokens, model repair count, ordinary-agent comparison or application performance. The [semantic byte probes](SEMANTIC-QA.md) verify bounded selection rather than better agent decisions.

Current product coverage and strict verification commands are recorded in
[Verification](VERIFICATION.md). Tool versions used by a particular run are local
provenance; host analyzer version numbers here are not a portable dependency lock.
