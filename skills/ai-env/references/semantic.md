# Compiler context

Semantic graphs support **C, C++, C#, Rust, Python, TypeScript and JavaScript**. Other languages use lexical `context`; there is no inferred cross-language graph. Commands below use `ae` for `python3 <skill-directory>/scripts/ae.py --root <repository>`.

## Build with the real configuration

C/C++ require a compatible host libclang, Python 3 and the target's headers/toolchain. Prefer its existing compilation database:

```sh
ae graph build --language cpp --compile-commands build/compile_commands.json
```

A database may live under a directory omitted from source snapshots; it is hashed separately. Its commands are parsed, not executed. For a small standalone project, supply compiler arguments individually:

```sh
ae graph build --language c --flag -std=c11 --flag -Iinclude
ae graph build --language cpp --flag -std=c++20 --flag -Iinclude
```

`--file src/app.cpp` can be repeated to select translation units. Supplied headers still participate in those units. Repeating `--flag` preserves argument order. `--libclang /absolute/path/to/libclang.dylib` selects a library explicitly. Missing settings produce visible partial analysis; do not treat guessed defaults as the project's build. Response files, plugins, PCH/modules and unsupported compiler flags are rejected rather than executed or silently ignored. Use a supported, explicit argument set when the database needs adjustment.

C# requires the bundled Roslyn helper and a .NET 8 runtime. Compile all indexed `.cs` sources, or select files, with explicit project defines and metadata assemblies as needed:

```sh
ae graph build --language csharp --define DEBUG --reference /absolute/path/to/Dependency.dll
```

This is a static Roslyn compilation against host .NET metadata and supplied references. It does not evaluate `.csproj`/MSBuild, run generators/analyzers, or infer solution/project boundaries. For multiple projects or target frameworks, select the relevant files and supply their correct references/defines; separate repository roots may be appropriate. Unity/framework-specific or generated types need their real metadata/source. Inspect compiler diagnostics before relying on the graph.

## Rust

Requires `rust-analyzer` and a Rust sysroot with `rust-src`. Build configuration is read statically; source is analyzed in an isolated `rust-project.json` workspace:

```sh
ae graph build --language rust --project Cargo.toml
```

A supplied `rust-project.json` is also supported. Local path crates participate when their sources/manifests are indexed. Registry dependencies, build-generated cfg/source, proc-macros and Cargo build scripts are not built or downloaded. Missing types/macros/dependencies remain partial or unresolved. The graph uses rust-analyzer declarations and syntax/definition results; it is not a passing `cargo check` proof. Trait-object calls preserve the static trait target rather than all runtime implementations.

## Python

Uses bundled Jedi/Parso plus Python AST source extents. No target module, `setup.py`, custom import hook or native project extension is executed:

```sh
ae graph build --language python
```

Indexed modules and the conventional `src` root are supplied as an isolated snapshot. Aliased imports, inferred receivers, annotations, fields and inheritance can produce static dependencies. `--project pyproject.toml` tracks an explicit project file but does not evaluate packaging configuration, install dependencies or infer arbitrary source roots. Installed-interpreter standard-library files and bundled static stubs can participate; third-party runtime environments are not reconstructed.

Python graphs always show `PARTIAL`: static inferred call targets are candidates, and monkey-patching, decorators, reflection, dynamic imports and dispatch can change runtime behavior. Read the printed unresolved entries/limitations. A type-shaped string is not treated as proof that a runtime object has that type.

## TypeScript and JavaScript

Require Node.js and the bundled TypeScript Compiler API (5.9.3). These are two query languages over compatible TS/JS sources; imports between those source formats can resolve within the selected graph:

```sh
ae graph build --language typescript --project tsconfig.json
ae graph build --language javascript --project jsconfig.json
```

`tsconfig.json`/`jsconfig.json`, static `extends`, paths and normal module resolution are read without running Node project modules, npm scripts, plugins or custom transforms. Explicit source snapshots override stale disk sources. JavaScript uses TypeScript's JSDoc/static inference; `any`, ambiguous union/callback targets and dynamic properties remain unresolved where a unique target is unavailable. Project references are not built. Missing package declarations or generated types require the appropriate real files; analysis never installs them.

## Analyzer readiness and distribution

All adapters require Python 3.11+. The complete skill carries Jedi/Parso and the pinned TypeScript API with their licenses. Node.js, rust-analyzer/rust-src, libclang and .NET remain host runtimes. Analysis never installs dependencies on a query. `scripts/dev.py build` records detected trusted host tool paths in the tool bundle's `scripts/bin/runtimes.json`; it does not read executable overrides from target-project configuration. This profile is machine-specific: after copying to another machine, use PATH or explicit `AI_ENV_NODE`, `AI_ENV_RUST_ANALYZER` and `AI_ENV_RUST_SYSROOT` (the Rust toolchain sysroot, not its `library` subdirectory), or rebuild the package there.

The project's `scripts/dev.py check` exercises all shipped adapters against real analyzers. Missing runtime artifacts are failures in this product check. Static analysis does not replace the target's actual named build/test checks.

## Select facts, then implementations

```sh
ae graph status --language cpp
ae graph find refund --language cpp --budget 6000
ae context --symbol cpp:COMPACT_ID --relations calls,callers,types --depth 1 --budget 6000
ae context --symbol cpp:COMPACT_ID --relations types --body --budget 6000
ae graph impact cpp:COMPACT_ID --depth 2 --budget 6000
```

Use an actual compact ID from `graph find`. Overload declarations and template specializations retain distinct contracts; reference links connect them to implementations or primary templates. Use `--relations references` to inspect those links, then select the implementation ID for its body. Impact at depth 2 follows these intermediate contracts to callers. Overloads remain distinct; an ambiguous name reports alternatives instead of picking one. Signatures are static analyzer contracts; cards include source locations and current file hashes. Related symbol cards and source files are deduplicated. `--body` returns only the selected compiler declaration extent, which may include a full type for a type symbol. External or synthesized symbols have no user-written body.

Relations: `calls`, `callers`, `types` (including fields/aliases/bases), `used-by`, `references` (incoming and outgoing), `inheritance`. Depth is 1–3. Impact traverses incoming indexed relations; it is a review candidate set, not a proof of runtime reachability or an exhaustive test plan. Virtual/interface calls identify the compiler's static target, not every possible runtime implementation. Function pointers, delegates, dynamic/reflection, templates requiring instantiation and generated code may remain unresolved; unknown relations are explicit. A complete static analysis is still subject to the printed limitations.

Budgets are **UTF-8 bytes**, not model tokens. Continue a page with:

```sh
ae context --cursor g1-TOKEN --budget 6000
```

This resumes find, context or impact using the encoded query. Read through `end` before claiming all selected facts were returned; choose a narrower query if the result is too large.

## Freshness

The local graph is derived state under `.ai-env/semantic/`. Language sources, build options/database, analyzer implementations, pinned vendor libraries, runtime profiles and read external headers/metadata/standard-library sources are hashed. Changes to those recorded inputs reject queries with a refresh instruction. Provenance records read files; a newly installed dependency or an external header that starts shadowing another include may require an explicit refresh even if no recorded file changed. Toolchain/environment changes are not a reproducible-build guarantee. Refresh explicitly after such changes:

```sh
ae graph refresh --language cpp
```

Refresh retains the last build options; run `graph build` again to change them. Unrelated documentation edits do not invalidate the graph, but any indexed source/config edit invalidates an existing continuation cursor. Compiler failure never replaces the last graph with fabricated lexical edges. Partial graphs expose diagnostics/unresolved counts and a `PARTIAL` marker; repair configuration or use explicit source inspection where coverage is missing.

For a large `brief --impact QUERY`, an existing graph may refresh on demand
after **source-only** edits. The saved file-hash manifest must match its recorded
source key and build options; current analyzer/runtime and external/explicit
inputs must still match. Adding, removing or changing indexed language source
is eligible. Configuration, lockfile, project-root or tool provenance changes,
corrupt caches and old stale caches without this manifest require an explicit
refresh. No missing language graph is built, no configuration is discovered,
and no runtime is installed.

All languages share a 20-second refresh deadline and at most two analysis
attempts per packet. Hashing cooperatively checks that deadline between reads;
helper processes and their descendants are terminated on timeout, with bounded
cleanup. This is not a hard wall-clock limit for filesystem calls or the entire
brief. Rebuilt facts pass the same validation and freshness checks before atomic
publication. Failures preserve the old cache but use lexical selection with a
reason; stale relationships are never returned. A changed source snapshot
rejects the packet. Small packets and briefs without `--impact` never refresh;
the latter can still use fresh cached graphs.
