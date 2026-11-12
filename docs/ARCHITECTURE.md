# AI Env architecture

The project ships a small skill at `skills/ai-env`, a native Rust executable and compiler adapter helpers. A human continues using their existing editor. Models make planning and semantic decisions; Rust owns scheduling, persistent state, edits and check gates. The Python launcher and compiler adapters do not orchestrate agents.

`src/workflow.rs` owns the task graph and individual execution attempts. Task
identity survives retries. Read-context prefixes grant no write permission;
write/write and write/read overlap prevent simultaneous dispatch. Explicit
contract dependencies sequence shared interfaces before consumers. Scopes are
an enforcement boundary, not a proof that the architect understood all runtime
relationships.

`src/orchestrate.rs` is the native Codex transport and control loop. Auto mode
asks a coordinator to choose one executor or a team; explicit single/team modes
are available. A small change avoids separate architect/reviewer calls, but still
gets actual checks and a model assessment after those checks. Team mode uses
architect, scoped developers, independent reviewer and QA. Sessions are read-only;
Rust validates and applies structured proposals through existing snapshot/file
CAS and cooperating task claims. The main project lock is not held during model
inference; a separate workflow lock prevents duplicate dispatchers.

`src/model_edits.rs` binds hashless model edits to a private original-file
manifest captured before dispatch and persisted for replay. It never reads
current hashes to authorize a saved response; the normal scope/CAS transaction
remains authoritative. Direct native edit APIs still require original hashes.

`src/verification.rs` validates a plan's bounded change-specific risk obligations
and QA's source/check evidence. Every acceptance index must be assigned. Fresh
reports bind the source and contract; gaps route to owners and dependents. Static
inspection is distinct from passing execution, and all approved checks remain
mandatory. Models still judge risk relevance and behavioral test coverage.
`src/qa_context.rs` selects obligation/changed/read scopes and compacts style/test
metadata for QA without repeating the global discovery inventory. It preserves
discovery counts, approved commands and explicit omissions. Instruction discovery
uses every requested scope even when the source selection is truncated.

Each attempt records its owner, input snapshot, outcome and proposal/error.
Successful proposals survive sibling failures and process restarts. Applying an
edit is separate from accepting the task: proposal → applied → verified →
completed, with current combined-source proofs required for approved checks.
Repair invalidates affected tasks and dependent consumers while retaining other
applied work. A finite attempt/round budget and unchanged-failure detection stop
unproductive retries. Replanning is an explicit plan-level operation.

Source transactions retain an apply intent so recovery can distinguish an
unapplied proposal from an already committed change. Ambiguous interrupted edits
require inspection instead of replaying a patch. This does not make filesystem
writes power-loss atomic. Other editors can still write outside the controller.
Freshness gates and CAS reject unexpected indexed changes.

Missing check configuration for discovered tests stops before model dispatch.
Per-role prompts/results, usage, original source artifacts and actual checks
remain under `.ai-env/`. Resume does not replay the full conversation or reset
usage. Role composition has a real cost; no efficiency improvement is inferred
from architecture alone. Worktree-based autonomous builds are a future stage;
current executors propose patches in read-only sessions.

`src/profile.rs` builds bounded static evidence for scoped style settings,
nearby source conventions and test inventory (including tests outside source
scopes). Explicit configs retain scope/provenance and outrank inferred samples.
It does not execute configuration, merge every formatter's rules, infer naming
with certainty or certify complete test coverage. The profile sits beside the
compiler graph; the graph supplies program relationships rather than style
policy. Existing configured formatter/linter and unit/integration checks supply
executable enforcement through the same proof mechanism.

`src/project.rs` discovers eligible files, hashes current sources and configured checks, validates paths, writes state atomically and provides a per-project process lock. Git roots use tracked/nonignored candidates; other roots use a bounded walk. Lexical discovery always reads actual sources; the semantic cache described below remains derived state.

`src/context.rs` provides file/hash summaries, literal text matches and selected line views. Its cursor carries the query, offset and snapshot. A continuation re-scans and rejects stale snapshots; page generation is bounded and Unicode-safe. Text matches do not imply actual call edges, types or effects. Semantic queries route separately through `src/graph`.

`src/brief.rs` offers an adaptive work packet: exact sources, existing HANDOFF.md
and configured check argv in one UTF-8 byte budget. Explicit source scopes retain
small indexed local check drivers so a model can see existing build/test coverage.
If the eligible source set
fits, it bypasses graph reads/backend fingerprints entirely. Larger packets
rank literal matches and one-hop neighbor paths from validated cached graphs,
then emit source once, with explicit excerpts and omitted counts/paths. Stale
or invalid graphs contribute no facts and are labelled as lexical fallback.
This does not silently rebuild analyzers or promise exhaustive impact. Source
snapshots and used compiler provenance are checked again before publication.

`src/consistency.rs` adds task-specific navigation to `brief --impact`: bounded
query anchors, same-directory/language examples, filename-related tests and
input-bound static edge sites. Selected candidates influence source allocation;
the packet states selection reasons, exact locations, visibility and omissions.
One directory cannot fill every anchor slot. Small packets still skip graph IO.
Explicit paths constrain selection; source or example gaps do not widen write
permission. A snapshot binds the output; `--hashes` additionally includes file
hashes. A nearby file is not a proven equivalent contract, active owner or policy.
No semantic duplicate detector or style certification is claimed.

`src/retrieval.rs` shares navigation evidence between candidate selection and
source ranking. Explicit canonical relative paths and exact structured
identifiers precede broad substring scores. Selected example/test reservations
remain in force. Excerpts reserve strong matching lines before their surrounding
windows, so a late definition or caller can survive early generic matches.
Declaration-shaped text is only a lexical presentation hint. Scope, byte budgets,
source binding and omission reporting remain authoritative.

Native plans record reuse/extension decisions in existing architecture boundaries
and task instructions; executors retain concise decisions in durable summaries.
Review/QA receive bounded historical summaries (including current task status and
omission counts) and read-context paths. They compare actual diffs with applicable
owners/examples inside existing architecture assessments, allowing justified new
abstractions. Summaries are claims, not current source or executed-test proof.
No new role or model call is introduced by this mechanism.

`skills/ai-env/scripts/prepare_task.py` is an optional host dispatch hook. It
attaches the original task, scoped project instruction files and current source
before inference; no model call or target-source execution occurs in preparation.
Global/profile instructions remain owned by the host. The instruction scan has
explicit directory exclusions and a separate 32 KiB limit; failures emit no
partial prompt. This hook removes interactive discovery only when a host uses
it. Attached context and local preparation still have a measured cost.

`src/edit.rs` prevalidates the complete source batch, source/file CAS and task ownership. It stages replacements and backups before commit and reports rollback/cleanup problems explicitly. It protects cooperating writers; a noncooperating editor can still write concurrently. Source patch inputs should be outside indexed files so constructing a request does not invalidate its own snapshot.

`src/verify.rs` runs one named check from the owner's configuration, bounds time/output and records the exact command, source snapshot, exit status and capture result. Passing requires unchanged indexed sources/config and successful bounded execution. Checks use ordinary user permissions; the tool is not a code-execution sandbox. All commands share the project lock, so verification serializes cooperating writes; agents can still reason/read independently outside that critical section. Checks must not recursively call this same project's AI Env CLI.

`src/tasks.rs` tracks explicit task scopes, dependencies, owners and completion proofs. Claims prevent overlapping active scopes. Task completeness is bound to scoped files; verification itself is bound to the full indexed snapshot and configured checks. These claims remain respected by workflow source integration; model dispatch belongs to the native Rust runner or an external host.

`src/notes.rs` stores concise decisions and their file scopes. A changed scope marks the note stale rather than treating it as a fact. A stale note may still contain useful intent; the agent must inspect it alongside current code. `handoff` combines current task/check state and decisions; it does not load conversation history or promise to eliminate model context loss.

All generated state is under `.ai-env/`, ignored by its own Git ignore file. State is local, not a replacement for Git. Source text and project tests remain authoritative. The first release supports POSIX locking/process groups on macOS/Linux. A built skill carries a binary for the build platform and a Python3 launcher; installing it does not change IDE configuration or contact an external service.

The skill layout follows [official OpenAI documentation](https://developers.openai.com/plugins/concepts/skills). Keep its discovery description and main instructions small, and load the command reference only when needed.


`src/graph` provides a shared compiler adapter protocol, cache validation and selected graph queries. The Rust process runner sends exact indexed UTF-8 text and compilation settings to fixed Python helpers over JSON stdin/stdout; JSON is an internal transport, not a programming language or the model-facing context format. No shell command from a target project is executed by analysis. The runner bounds wall time (120 seconds), combined output (16 MiB) and request size (192 MiB), and kills the process group on timeout. Serialized local cache state is capped at 8 MiB; larger projects must select a narrower analysis scope.

C/C++ use ctypes/libclang, unsaved source overlays, canonical USRs and parsed compilation databases. C# uses a shipped Roslyn helper against supplied syntax trees and metadata; its own helper is built offline, while target MSBuild, analyzers and generators are never run. Symbols preserve overload identity; UTF-8 compiler extents refer back to indexed source. Synthesized/external declarations are labelled. The common graph has call/reference/type/alias/inheritance/containment edges, compiler diagnostics, explicit unresolved relations and dependency provenance. More languages need a real adapter for this protocol; lexical search is not presented as semantic fallback.

A cache envelope binds the canonical project root, relevant language-source hashes, build options, backend implementation and read headers/libraries/metadata hashes. Reads and publication recheck freshness; changed inputs reject queries until explicit refresh. Unrelated docs do not require recompilation. Source/config snapshots still bind every returned page, and the graph hash binds the encoded semantic query. Compact IDs hash canonical compiler IDs; collisions are checked. Ambiguous names require selection. Breadth-first queries deduplicate symbol/file cards and page signatures by default; bodies are explicit. Impact follows incoming static edges and cannot guarantee dynamic runtime coverage. Missing settings, compiler errors and unresolved calls produce visible partial analysis rather than invented facts.

The adapter registry now accepts `c`, `cpp`, `csharp`, `rust`, `python`, `typescript` and `javascript`. New analyzers share the same validated graph/cache/query protocol; JSON remains an internal transport. Python uses AST extents and Jedi static inference over a materialized source snapshot. TS/JS use TypeScript Compiler API 5.9.3 with a virtual snapshot host and static configuration parsing. Rust uses rust-analyzer LSP/syntax/definition results over an isolated generated crate graph; Cargo build/check scripts, procedural macros and target programs are disabled. No target modules are imported or run. Each adapter reports unresolved relations and limits; Python is always explicitly partial.

Source keys include language metadata such as Cargo manifests/locks, Python project files and TS/JS JSON configuration/module inputs. `--project` selects a static configuration file for new adapters; it does not authorize executing that configuration as code. Backend identity includes helper files, pinned vendor contents, runtime profile and explicit runtime overrides. Provenance covers the runtime/analyzer binaries and read external source/metadata; Rust also hashes its standard-library source tree. Freshness therefore has a local IO/hash cost and is not an incremental long-lived language server.

The package contains Jedi 0.20.1/Parso 0.8.7 and TypeScript 5.9.3, including upstream licenses. Node.js and rust-analyzer/rust-src remain host tools. Build/check preparation records trusted host tool paths in the bundle's ignored `scripts/bin/runtimes.json`; copied machine-specific paths can be overridden with explicit environment variables or PATH. Analysis never reads executable settings from a target repository or installs packages. This runtime profile is part of derived analyzer identity, not a portable dependency lock or a runtime sandbox.
