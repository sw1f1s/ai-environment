# Extend the existing project

Use this reference when a change adds a responsibility, its owner is uncertain,
or neighboring implementations follow competing patterns. It guides the current
executor/review loop; it does not add a mandatory role, report or model call.

## Choose evidence before choosing structure

Start with the original task, applicable project instructions and the current
`brief --impact` packet. Its bounded matches and local examples are candidates,
with selection reasons and source references. Lexical similarity, proximity and
graph relationships do not establish which implementation owns a responsibility.
Missing evidence is a read gap, not permission to invent a replacement.

Use known exact identifiers or relative file paths to focus retrieval when
available. Strong matching lines can be shown without a whole definition or its
callers; inspect the missing contract rather than inferring it from a candidate
label. Names that differ from the request can still own the required capability.

Inspect the likely contract, at least one actual consumer and a relevant local
implementation/test path as needed. Prefer the subsystem being extended and its
active integration paths over a repository-wide majority. Confirm which paths
are used before treating an older implementation or deprecation comment as
current guidance. Explicit project rules outrank inferred examples. If those
conflict, identify the conflict rather than silently turning a sample into policy.

Different names can implement the same responsibility; similarly named helpers
can have incompatible units, inputs, errors or effects. Compare behavior and
callers before reuse. Request focused missing source rather than repeatedly
fetching the whole packet. A graph is a navigation aid with stated coverage,
not proof of exhaustive impact or semantic equivalence.

## Make the extension decision visible

Put a short decision in the current task or plan, proportional to the change:

- **Reuse:** name the existing capability and the contract/call sites that fit.
- **Extend:** name its owner and extension point, and which consumers or tests
  must adapt.
- **New:** explain the required behavior that existing contracts cannot provide,
  and where it belongs in this subsystem.

Cite current repository paths and lines; use packet hashes/references when
available. Say what is still unknown. This decision is not an additional model
response or a mandatory persisted artifact for a one-line fix. New abstractions
are allowed when justified; avoid speculative infrastructure and unrelated
architectural repairs.

For example, a new import format may belong behind the existing adapter contract,
letting the pipeline retain validation, transactions and idempotency. Inspect that
pipeline and an active adapter/test before adding a second database-writing
import service. A streaming importer with different lifecycle requirements may
legitimately need another contract; explain the mismatch instead of forcing it
through the old adapter.

## Review consistency in the actual diff

Compare changed paths and affected consumers with the selected examples:

- Does an existing owner still own the rule, and do consumers actually call it?
- Do dependencies, public API and errors follow the relevant subsystem contract?
- Do state changes, external effects and retry behavior use its established path?
- Do tests use the local fixtures, assertions and actual configured discovery?
- Is new code duplicating a capability already present under another name?

A finding needs the changed location, the applicable rule or analogous existing
source, and the concrete consequence. Passing tests do not establish architectural
consistency. A personal preference, different valid local pattern or justified new
abstraction is not a blocker. Do not score SOLID, count interfaces or infer quality
from code length. Use [architecture.md](architecture.md) when responsibilities,
public contracts or dependency direction need a deeper review.

## Retain decisions that will matter again

Existing source-bound notes can retain a verified ownership/extension decision
that a later task would otherwise rediscover. For example:

```sh
python3 <skill-directory>/scripts/ae.py --root <repository> note set import-owner --text 'src/import/pipeline.py owns transaction and retry behavior; vendor adapters supply parsed records. Reuse that pipeline for another vendor.' --paths src/import,tests/import
```

Use the real paths that support the decision, including relevant tests and
contracts. `note list`, `note show ID` and `handoff` expose staleness when those
paths change; re-inspect stale source before reusing the decision. A fresh note
only means its recorded paths have not changed. It does not prove the decision
correct or complete, and changes outside that scope can invalidate its reasoning.
Do not save every task summary or replace source inspection with a growing
catalog of unverified rules.
