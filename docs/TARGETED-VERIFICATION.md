# Original-revision edits and targeted QA

The native model transport binds proposals to saved original revisions and
requires change-specific evidence against the combined source. Rust enforces
identity, freshness, scope and completion gates; models judge semantic coverage.

## Model edits

The native coordinator/developer returns one of these forms per file:

```json
{"path":"src/policy.py","replacements":[{"old":"LIMIT = 10","new":"LIMIT = 20"}]}
```

```json
{"path":"src/result.py","source":"VALUE = 20\n"}
```

`expected_hash` is rejected in model output. Rust captures indexed original
hashes before dispatch and persists a bounded private manifest in that call's
`inputs.json`. Receiving and replaying the raw answer use this same manifest.
Per-file hashes are omitted from native source/profile prompts. The snapshot
identifier still binds the entire response.

The manifest checksum catches corruption or accidental mixing, not malicious
host-state tampering. It never authorizes paths: existing scope, task ownership,
snapshot/file CAS and all-or-nothing edit validation remain in force. A new file
is bound to absence, so a newly appearing destination conflicts rather than
being overwritten. Saved inputs are never rebuilt from current file hashes.
Sequential replacement anchors must still match exactly once; this does not
solve ambiguous anchors automatically.

Manual/native edit API callers still provide `expected_hash`. The hashless
protocol belongs to the built-in model transport, not every patch API.

## Plan and evidence

The existing planner call creates 1–6 obligations, covering all zero-based
acceptance indices. For example, within the corresponding task's actual scopes:

```json
{
  "id":"rejection-is-atomic",
  "acceptance":[0,1],
  "tasks":["change-policy"],
  "risk":"Invalid limits must be rejected before cached state changes; the public caller must preserve the existing policy.",
  "checks":["unit"]
}
```

An obligation records responsible tasks, a concrete risk and approved check
names. Native model plans use task `read_refs` from the host catalog instead of
writing read-context paths. Rust resolves these references, adds direct
dependency outputs and derives obligation paths from the named tasks' write/read
scopes. It rejects an overlarge union instead of dropping scopes or widening
permissions. Direct controller plans still provide `context` and obligation
`paths`. Contract validation requires
complete acceptance-index coverage; it cannot prove that the plan understood
every user requirement. The full original goal remains in QA's prompt.

Within its existing call QA returns exactly one result per obligation:

```json
{
  "id":"rejection-is-atomic",
  "status":"verified",
  "proofs":["ACTUAL_PROOF_ID"],
  "source_refs":["ACTUAL_SOURCE_REF_ID"],
  "evidence":"The discovered regression checks rejected limits and unchanged cache state through the public caller; the current unit proof passed."
}
```

The IDs above illustrate the protocol; an actual report selects a fresh proof
and an eligible ID from the host's per-obligation source catalog. Rust binds that
ID to a visible, scoped source fragment and stores ordinary controller locations.
Invisible, omitted or unindexed files receive no fabricated reference. Direct
controller reports still provide `locations`. Evidence is a short semantic
explanation, not a copied checklist.

| Status | Required meaning |
| --- | --- |
| `verified` | Current scoped locations and evidence, plus fresh complete passing proofs covering all check names selected for this obligation. |
| `gap` | A defect or missing evidence. Requires `changes`; repair targets the obligation's owners and dependents. Locations may be empty for an absent artifact. |
| `inspected` | Static evidence only, allowed when no checks are approved. No execution proof or claim of runtime verification. |

All approved project commands still run against the combined source, even if an
obligation selects a subset. A passing command cannot override a reported gap.
Rust validates identities, source/configuration freshness, scope, proof execution
and report completeness. QA must still establish that discovered tests exercise
the relevant behavior: a passing command alone is insufficient semantic evidence.
Malformed selectors receive a bounded correction of those selectors only;
semantic or unresolved evidence requires substantive QA. Neither correction
regenerates implementation or reruns still-fresh checks. Genuine gaps trigger
addressed repair, preserving independent
tasks; architecture blockers and separate functional findings retain their gates.

`verification_report` is stored against both source snapshot and contract hash.
Changes stale old evidence. Updating only risk wording does not discard unchanged
implementation contracts. A repeated identical gap on unchanged source is keyed
by obligation IDs and contract, so rewording the explanation cannot reset its
no-progress guard. Attempts, rounds and cumulative usage remain bounded.

## QA context

The source packet selects obligation paths, actual changed files and responsible
tasks' declared read context. Ancestor project instructions apply to all requested
scopes, including omitted source selections. Missing paths do not expand the
instruction scan to unrelated trees. Small source/diff budgets, path counts,
omitted/missing scopes and clipped excerpts are explicit.

The compact profile retains discovery counts, relevant style excerpts and
approved check commands; it drops the global test-file inventory, candidate
commands and inferred style samples. The source brief retains its ordinary
configured-driver navigation. Broad declared scopes can still select unrelated
source; the model must choose narrow risks and read any reported gap. Selection
is static navigation, not exhaustive dependency or test coverage.

## Source references and bounded metadata correction

The runner persists planning/source catalogs before dispatch and
keeps the raw model answer immutable. Selector-only correction receives affected
rows and permitted choices without the full source/profile packet. Rust merges
only references; code, decisions, evidence and verdict remain the original
answer. It verifies the original artifact hash and dispatch bases on replay, and
also rejects a changed QA contract even when the source snapshot is unchanged.
All corrections count toward the existing call, usage and no-progress limits.

A valid source reference establishes navigation provenance, not correctness.
The model must still decide whether tests exercise the requirement. If no visible
eligible reference supports the saved assessment, correction must be unresolved
and the role must reassess. [Protocol and limits](SOURCE-REFERENCE-BINDING.md).

## Validation and limits

Product tests exercise hashless coordinator/developer edits, persisted-answer
replay, strict shapes, concurrent-writer conflicts, scopes and atomic batches;
obligation validation, fresh/forged/failed proofs, semantic gaps despite green
commands, static-only reporting, addressed repair, retained siblings and bounded
no-progress handling; and QA scope/budget/instruction behavior. The native CLI
tests replace inference with deterministic responses but use real Rust state,
filesystem transactions and configured Python checks.

See [product verification](VERIFICATION.md) for current coverage and commands.
These tests establish controller behavior. They do not establish better agent
judgment, complete test coverage or a reduction in maintainer rework. Ambiguous
replacement anchors still require a revised edit; reference binding does not
resolve source ambiguity.
