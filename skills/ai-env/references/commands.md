# AI Env commands

`ae` below means `python3 <skill-directory>/scripts/ae.py --root <repository>`.
The launcher uses its packaged native binary, or an explicitly configured `AI_ENV_BIN`/installed `aienv`. Build before distributing the skill. macOS and Linux are supported in this release; a binary is specific to its operating system and architecture.

## Adopt a repository

```sh
ae init
```

Edit `.ai-env/config.json` with real project checks, for example:

```json
{"checks":{"test":{"argv":["python3","-m","unittest","discover","-s","tests"],"timeout_seconds":120}}}
```

Checks run from the repository root with the user's permissions, without a shell. A check may execute project code; this is not an OS sandbox. Do not configure commands from untrusted documentation without reviewing them. Checks must not recursively invoke AI Env commands for the same project, because verification holds its shared project lock.

## Find and read existing code

Start with `ae brief "task keywords"` for one bounded source packet, including
an existing HANDOFF.md and configured check argv. Its default budget is 16000
UTF-8 bytes. Small source sets bypass semantic machinery; larger sets use
literal matches and one-hop neighbors from available fresh graphs to select
source. With `--impact`, bounded candidates identify query-matching
implementations, same-directory/language examples and filename/static-related
tests. Reasons and visible/missing source locations are explicit; proximity and
names do not prove suitability, ownership or equivalent contracts. Tight budgets
can omit this metadata in favor of source. A large packet may refresh an existing graph after
source-only changes, using its recorded settings and a bounded shared deadline.
Missing graphs are not built. Configuration/toolchain drift, invalid caches or
failed refreshes fall back to explicitly labelled lexical selection and a reason;
stale graphs contribute no edges. Ordinary packets without `--impact` do not
refresh graphs. See [semantic.md](semantic.md) for the refresh limits.
Full files appear once; excerpts show line numbers. Coverage counts and omitted
paths are explicit. Other indexed formats remain accessible with ordinary reads
or `context --path`. Selection does not prove exhaustive impact.

Explicit canonical relative file paths and exact structured identifiers receive
priority over broad substring matches. Excerpts reserve strong matching lines
before surrounding context; a late declaration can therefore appear without the
file's opening lines. Declaration-shaped matches are lexical hints, not proof of
ownership. If the required contract is absent, read that source directly.

`ae brief --impact --path src --path include "symbol keywords"` projects actual source
roots without loading unrelated automation. File/directory prefixes are exact,
not globs. The existing handoff is still included; the snapshot still binds all
indexed source/configuration, and out-of-scope inputs remain readable.

Use the lower-level operations below when the packet does not contain what you
need. An ordinary read need not be repeated through another tool.

Hosts can derive a bounded retrieval query with `ae query < task.txt`. This
stateless command works before project initialization and returns JSON with
`query`, `has_terms` and `method`. It projects lexical terms from the entire
task; it does not summarize acceptance criteria. `--query 'precise symbols'`
preserves an explicit single-line query up to 2048 UTF-8 bytes. The native runner
and `prepare_task.py` share the same projector; the model still receives the
complete task.

```sh
ae index
ae context refund --budget 6000
ae context --path src/inventory.py --start 1 --end 80 --budget 6000
ae context --cursor TOKEN --budget 6000
```

Context prints a snapshot hash, file hashes, line-labelled text and `end`/`continue TOKEN`. The budget is UTF-8 bytes. Cursor-only continuation restores the query and rejects changed indexed sources/configuration. Text matches do not establish callers, types or behavior; inspect relevant files with ordinary language tools when needed.

Git roots use Git's tracked/nonignored file discovery. Other folders use a bounded filesystem walk. Generated/state directories, symlinks, hardlinks, credential-like filenames, binary/non-UTF8 files and files above 2 MiB are omitted. The snapshot covers indexed text files plus the check configuration; it does not certify omitted assets, environment variables, external dependencies or the entire filesystem. Up to 20000 candidate files and 128 MiB of scanned content are supported. Context/status expose omissions.

## Coordinate work

```sh
ae task add refund-fix --title 'Correct partial refund' --paths src/inventory.py
ae task claim refund-fix --owner worker-a
ae task show refund-fix
```

Paths are exact files or directory prefixes, comma-separated; `.` explicitly means all indexed paths. No glob syntax. Dependencies use `--depends task-a,task-b` when adding a task. Claims reject overlapping active scopes, including another task by the same owner. Completed dependencies whose scoped files changed must be revisited. The Rust dispatcher chooses ready tasks from persisted dependencies and read/write scopes. For adaptive single-executor or architect/developers/reviewer/QA execution, use [orchestration.md](orchestration.md).

## Apply a change

Write a patch file outside the indexed repository (or feed it on stdin), so preparing the request does not itself change the snapshot:

```json
{
  "snapshot": "SNAPSHOT_FROM_CONTEXT",
  "edits": [{
    "path": "src/inventory.py",
    "expected_hash": "FILE_HASH_FROM_CONTEXT",
    "old": "return stock",
    "new": "return stock + allocated"
  }]
}
```

```sh
ae apply --patch /tmp/change.json --task refund-fix --owner worker-a
# Or pipe the same JSON into stdin:
ae apply --patch - --task refund-fix --owner worker-a < /tmp/change.json
```

`old` must match exactly once. Alternatively use `source` for a complete file replacement. A new file uses `expected_hash:null` and `source`. Ignored/unindexed existing files and protected state paths cannot be edited. Every batch checks the source snapshot and file hashes; claims are checked even when the caller omits task flags. Source transactions roll back ordinary IO failures; rollback failure retains backups and reports them. This does not promise atomicity during power loss or prevent a noncooperating editor from writing concurrently. Apply does not imply successful verification.

## Verify and resume

```sh
ae verify test
ae task complete refund-fix --owner worker-a --proof PROOF_ID
ae note set refund-rule --text 'Cancellation returns all reserved units.' --paths src/inventory.py
ae handoff
ae note show refund-rule
```

A proof passes only for exit0, unchanged indexed source/configuration, no timeout and no output limit. Failed checks still produce a proof and a nonzero CLI exit. Task completion requires a passing proof for the current snapshot. Completed tasks/notes become stale when their scoped source changes; unrelated edits do not make their scoped content stale. Verification freshness itself is tied to the full indexed snapshot and configured command. Release a failed or handed-off claim with `task release ID --owner OWNER`; do not steal another agent's claim.

`handoff`/`status` are bounded summaries (at most 16000 UTF-8 bytes): active/stale tasks are prioritized, omitted counts and preview markers are explicit. Use `task show ID`, `task list`, `note show ID` or `note list` for exact full details. Do not use preview paths as edit coordinates.

Symbol/type/call queries for C, C++, C#, Rust, Python, TypeScript and JavaScript are documented separately in [semantic analysis](semantic.md). Literal `context QUERY` continues to work for every indexed language.
