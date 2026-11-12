# Host-bound references in model responses

The built-in model transport lets models select references from source catalogs.
Rust resolves paths and locations against the saved dispatch inputs, reducing
manual metadata copying while preserving scope, freshness and proof gates.
This is provenance validation, not a correctness or architectural judgment.

## Planning

Rust captures a private, checksummed catalog from the dispatched source snapshot.
The model sees up to 128 short references and their repository paths, within
16 KiB, plus explicit omission counts. Tasks return writable `paths`, selected
`read_refs` and explicit `depends`. Rust resolves read references and includes
direct dependencies' output scopes as context. Consumers must declare their
providers; different files do not imply independence.

Verification obligations name tasks, acceptance indices, risks and approved
checks. Rust derives their paths from those tasks' write/read scopes. It
minimizes only redundant nested scopes; more than 16 remaining scopes is an
explicit contract error. References grant no write permission. New output paths
remain explicit, canonical repository paths, including valid Unicode and spaces.

## QA

Rust parses the actual full-file and numbered-excerpt cards in the dispatched
brief, checking their content against the indexed snapshot. It catalogs only
visible fragments that intersect an obligation and its owners' scopes. A public
catalog supplies IDs, paths, ranges and eligible obligation IDs, with explicit
omissions. It never creates a line-one citation for an invisible file.

Each result returns `source_refs` eligible for that obligation. `verified` and
`inspected` need a reference; `gap` may have none for an absent artifact. Rust
resolves references into controller locations. Selection establishes which
source was visible, not that a test covers the behavior. Fresh passing proofs,
actual semantic evidence and all existing completion gates remain required.

## Metadata-only correction

1. Persist the original raw answer and original private dispatch bases.
2. When a selector is invalid, prepare only its affected rows, saved task or
   assessment meaning, permitted choices and the validation error.
3. Ask the same role for `{snapshot, selections: [{id, refs, unresolved}]}`.
   This call has no full source packet or project profile and requests no tools.
4. Merge selectors into the saved original answer. Reject changes to other
   fields, owner identity, snapshots or allowed choices. Apply the usual plan,
   edit, scope, proof and assessment gates to the merged answer.
5. If support is unavailable, `unresolved` requires substantive reassessment.
   All attempts consume the normal cumulative usage and no-progress budgets.

Replay checks the original answer's SHA-256, matching role/snapshot and private
dispatch bases. QA additionally compares the current contract, including task
ownership, instructions, dependencies, goal, architecture, acceptance and checks.
Updating contract wording without changing
source still invalidates an old reference catalog. Current files never replace
the saved original revision. Checksums detect accidental corruption/mixing;
local workflow storage is a trusted host boundary, not protection from a host
that rewrites every artifact and checksum.

The correction specification is limited to 32 KiB. Oversized or malformed
correction inputs stop explicitly; the host does not silently drop affected rows
or escape the limit by dispatching an unbounded assessment.

## Boundaries and verification

Manual controller callers continue to send task `context`, obligation `paths`
and QA `locations`. The native model fields do not alter these APIs or weaken
their permissions. Architecture findings still use their own source locations.

Offline tests cover real brief formats, Unicode/CRLF, source-card spoofing,
ownership, unknown/duplicate references, strict shapes, bounded catalogs,
dependency outputs, immutable replay, tampered origins, contract changes,
unresolved assessment and bounded repeated failure. Native integration tests
use deterministic model responses with real Rust state, source transactions and
configured checks. They establish controller behavior, not real model-token
savings or improved application quality.

See [product verification](VERIFICATION.md) for current coverage. Source binding
does not guarantee that a selected fragment supports the model's conclusion.
