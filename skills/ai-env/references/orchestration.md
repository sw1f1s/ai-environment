# Adaptive native orchestration

Models decide what is needed and assess the result. Rust owns dispatch, task and
attempt state, source application and check gates. Progress lives under
`.ai-env/`; recovering a session must not recreate already applied work.

## Start and resume

Build/package AI Env, initialize an ordinary project, and configure its actual
checks in `.ai-env/config.json` before dispatch:

```sh
aienv --root /project init
```

```json
{"checks":{
  "unit":{"argv":["python3","-m","unittest","discover","-s","tests/unit"],"timeout_seconds":120},
  "integration":{"argv":["python3","-m","unittest","discover","-s","tests/integration"],"timeout_seconds":120}
}}
```

Use the project's existing runner, not these example commands when inappropriate.
`profile` supplies static evidence and candidates, never fabricated execution.
Discovered tests without approved check commands stop dispatch before models or
workflow creation. Explicit `--check` selects fixed configured names. Otherwise
names containing test/tests/unit/integration/qa/check/checks/lint/style tokens
separated by `-` or `_` are selected; `workflow checks` exposes this policy.
Models cannot add executable commands or omit an approved check.

Checks establish only the behavior they exercise. Contracts and regression tests
should cover changed callers and state invariants, including numeric ranges,
overflow, atomic rejection and idempotence where relevant. Review and QA return
concrete source/test assessments and coverage gaps. An assessment is a model's
claim, not a replacement for an actual test or formatter/linter proof.

```sh
aienv --root /project orchestrate change-pricing \
  --request /tmp/request.txt --mode auto --workers 2 --rounds 2 \
  --check unit --check integration

aienv --root /project orchestrate change-pricing --resume
```

Use `scripts/bin/aienv` from the skill bundle, or the `ae.py` exec launcher if
convenient. The orchestrator itself is Rust. Python remains only in existing
launch/context/compiler helpers and can also be a target project's test runner.

The optional model transport invokes the installed/logged-in Codex CLI with
structured output and read-only sessions. It consumes real model allowance.
Model selection stays with the existing host unless `--model` or a role-specific
`--coordinator-model`, `--architect-model`, `--developer-model`,
`--reviewer-model`, `--qa-model` is supplied. `--effort` overrides host effort.
No key provisioning or user-settings changes are performed. Other model hosts
can use the same native task/attempt commands without the Codex transport.

## Choose the smallest useful team

- `auto`: a coordinator examines the goal, current implementation, project rules,
  style/test profile and approved checks. It returns a contract and selects
  single or team execution. It must explain what is already present and reuse it.
- `single`: the coordinator also proposes the scoped implementation. Separate
  architect and reviewer calls are skipped. Rust applies edits and executes
  approved checks, then a QA assessment evaluates the combined result against
  acceptance. This has no independent code-review stage; request team mode when
  that assurance is required.
- `team`: an architect returns developer tasks and acceptance criteria. Ready
  independent tasks run up to `--workers` concurrently, followed by independent
  review, actual checks and QA. Auto mode can return this plan directly without
  an additional architect call.

A task contract includes stable identity, objective/instructions, writable paths,
read context, dependencies and acceptance/checks from its plan. The input snapshot
and owner are captured per attempt. Context is scoped separately for each task;
explicit repository rules outrank inferred style. Different files do not prove
independence. Shared interfaces precede consumers through explicit dependencies;
overlapping writes serialize execution. A task reading another task's write
scope must explicitly depend on that provider, including transitively.
Consolidate global rule/build configuration edits into one prerequisite task
before other tasks; the controller treats these settings as shared inputs.
Review should still check relationships missing from the declared graph.

Write/read scopes are canonical repository-relative file paths or directory
prefixes: `src` or `src/service.cpp`, not `./src`, `src/` or `src/*.cpp`.
In native model responses each proposed file occurs once. For several local edits use
`{path,replacements:[{old,new},...]}`. Replacements run in order
against virtual content; each nonempty `old` must match exactly once at its step.
Alternatively use `{path,source}`. Do not return or compute `expected_hash`.
Rust saves original indexed file hashes privately before dispatch and binds them
when receiving or replaying an answer; it never recaptures current hashes for a
saved response. New files receive a null original hash. Do not mix forms. All files
are validated before one native source transaction writes the result.
Direct `workflow attempt-propose`, `workflow apply` and patch API callers still
supply `expected_hash` themselves: their inputs already are native edit objects.

## Change-specific verification

Native planners select task `read_refs` from the supplied `planning_refs`
catalog. Keep `paths` for writable scopes only; never put prose or check commands
in them. Rust resolves read references to task context and adds direct dependency
outputs as read context. These references do not grant write permission.

Native planners return `verification`: 1–6 concrete obligations covering every
zero-based `acceptance` index. Each has `id`, `acceptance`, responsible `tasks`,
a short `risk` and relevant approved `checks`. Omit obligation `paths`: Rust
derives them from the named tasks' write/read scopes without widening them.
Too many scopes require a narrower plan, not silent truncation. A check name
records intended evidence, not proof that the runner discovers a particular test.
Use related risks together; do not replace the full user request with a generic
checklist or invent one obligation per assertion.

QA receives the original goal, these obligations, current changed sources and
their declared read context, scoped style excerpts and actual check results.
Its initial source/diff shares are bounded; omitted scopes and excerpts remain
explicit read gaps. The global test inventory and inferred style samples are not
repeated in this packet. Broad declared scopes can still require extra reading.
Every approved check runs on the combined result, including checks not selected
by a particular obligation; fresh proofs are reused for report-only retries.

QA returns exactly one `verification` result per obligation: `id`, `status`,
actual `proofs`, `source_refs` selected from that obligation's supplied
`source_refs.by_obligation` list, and concise `evidence`. Do not return
`locations`; Rust resolves references to current source locations. A reference
identifies an actual visible source fragment, not proof of behavioral coverage.
`verified` needs all of that obligation's named checks to have fresh,
complete passing proofs, and source/test evidence relevant to the risk. Rust
checks identity, scope, freshness and execution; whether the tests actually
exercise the risk remains QA's semantic judgment. A green build alone is not
behavioral coverage. With no approved checks, use `inspected` (static-only), never
`verified`. `gap` reports a defect or missing evidence, requires `changes` and
routes repair to the obligation's owners and dependent tasks. It can have no
reference when the missing artifact does not exist. If there are no eligible
visible references, report the evidence gap. Unrelated work is retained.

These are built-in model transport fields. Direct controller callers still use
task `context`, obligation `paths` and QA `locations`; the native controller's
scope and proof requirements are unchanged.

Status exposes `verification`, `verification_report`,
`verification_report_fresh` and `verification_execution`. Evidence is bound to
the source snapshot and contract. Changing the verification contract invalidates
old evidence while preserving unchanged implementation contracts. The same gap
on unchanged source stops repeated repair even if its explanation is rephrased.
Other native-controller hosts may omit obligations for legacy/manual plans;
declared obligations require complete reporting.

## Architecture contract and review

The built-in planner records `architecture` with `boundaries`, `dependencies`
and `extension_scenarios`. Each scenario has `change`, `expected_paths` and
`preserved_paths`: it describes a plausible future change tied to the request,
not new write permissions or an extra feature to implement. Small fixes may use
an empty scenario list. Boundaries and dependency rules follow the project;
interfaces and layers are not mandatory patterns.

Reviewer and QA use `architecture: {assessment, issues}`. An issue has
`severity` (`blocking` or `advisory`), `tasks`, `path`, `line`, `problem`,
`consequence`, `fix` and `verification`. Paths and one-based lines must point to
current indexed source; at least one named task must own that file. Use exact task
IDs. Return architecture issues once; do not duplicate them in legacy
`findings`/`tasks`, which remain available for separate functional findings.

A blocking issue requires `changes` and targets its owners and their dependent
closure. Advisory-only reports accept and keep the issues without another repair
round. Rust validates shape, ownership, location and freshness; the model must
justify the substantive architecture claim. Static review is not an executed
proof. [Criteria and examples](architecture.md).

Team review requires this assessment when the plan declares an architecture
contract. Single-mode QA performs it without a separate reviewer. Team QA receives
the independent report and its freshness; it cannot replace that report with its
own assessment. Status exposes `architecture`, `architecture_report` and
`architecture_report_fresh`, so a restarted role can reuse evidence while seeing
when it is stale. A changed global contract invalidates the old plan's work for
reassessment. Native hosts can continue existing workflows without an architecture
contract; declared contracts require the corresponding review gate.

## Durable progress and repair

For an extension, keep the source-backed reuse/extend/new decision in the
existing architecture boundaries and task instructions; select the owner/example
through plan `read_refs`. Executors retain a concise decision and rationale in
their ordinary summary. Review/QA receive bounded historical summaries, omission
counts and current task status alongside source and diff. They verify the chosen
approach against actual applicable examples; summaries and candidate matches are
not proof of ownership or correctness. This adds no role or model call.

Task identity is separate from execution attempts. Rust records a running attempt
before dispatch and its proposal/error after it finishes. A successful sibling is
saved without waiting for every other model to succeed. Source-bound proposals
can be applied on continuation without regenerating their implementation.
An already running sibling can still save its valid proposal after another
failure blocks the workflow. Blocking never permits application or new dispatch.

A proposal is not completion. Status distinguishes proposed, applied, verified
and completed work. Dependencies can proceed after an interface is applied, but
final completion requires assessment and fresh passing proofs for every approved
check on the combined source/configuration. A no-op result is legitimate when
existing code already satisfies the contract; it passes the same final gates.

A failed real check is also supplied to QA for attribution; Rust retains the
actual failing proof and never accepts a failed check as completion.
Actionable review/QA findings return to the affected task IDs and their dependent
closure. Unaffected implementations, summaries and attempt history remain.
Unlocalized findings conservatively invalidate all tasks. A broken plan requires
explicit replanning; a failing test alone does not discard the plan. Replanning
retains applicable work only when its contract and declared inputs remain valid.
External indexed edits invalidate the current workflow snapshot; inspect them and
explicitly replan rather than silently rebasing a stale model response.

Apply intent is persisted before source mutation. Recovery reconciles committed
changes instead of blindly replaying patches. Ambiguous source state stops for
inspection. Changes to `.gitignore` through workflow proposals are rejected because they can
change snapshot membership during application. Make index-rule changes separately
and explicitly replan. CAS and normal IO rollback do not guarantee power-loss atomicity;
uncooperating editors can still write independently.

Repeated identical failure on unchanged code is not useful progress and stops the
loop for reassessment. Attempts and repair rounds are bounded. Failed sessions,
invalid responses and resumes count toward cumulative call/usage limits.
Rejected structured responses retain their error and answer path for targeted
correction. Invalid read/source references can receive a small selector-only
correction call. It gets the affected rows, saved meaning and permitted choices,
without the full source packet. Rust merges only selectors into the immutable
original answer; code, plan decisions, evidence and verdict cannot change in
that correction. `unresolved` requires a substantive new role assessment. All
correction calls count toward the same usage and no-progress limits. Saved
catalogs, original-answer hashes and current QA contracts are checked on replay;
stale evidence is never rebound to current files. The runner retries within the
remaining budget; two identical rejected
responses or three rejected responses for the same role/task/snapshot stop for
reassessment. Transport failures still return control. Invalid plans are checked
before committing automatic single/team routing. Accepted task work is retained.
`--max-calls` limits dispatched attempts; `--max-tokens` stops subsequent dispatch
after observed input+output tokens reach the threshold. Running calls may exceed
it. Unknown usage stays unknown and prevents further token-budgeted dispatch.
Per-model `--timeout` bounds execution. Raising limits is explicit on continuation;
`--resume --check` cannot change the original approved checks.

Prompts, structured answers, execution receipts, original-source artifacts and
check proofs remain local under `.ai-env/`. Only scoped current context, relevant
findings and actual changes are supplied to a fresh role, not the full previous
conversation. A per-workflow lock prevents duplicate dispatch; project locks cover
short native state/source transitions instead of the entire model call. A Rust
worker owns each model process group and watches a lifetime pipe; loss of the
dispatcher cancels the model and retains its receipt. Resume waits for that worker
to stop before deciding whether another attempt is needed.

The current execution mode proposes patches. Separate worktrees for autonomous
builds and experiments are a later stage. The runner does not commit, push or
deploy. Role count and durable state alone do not establish token savings; measure
all calls against a comparable baseline.

## Native controller for another host

```text
workflow checks
workflow list
workflow start ID --request FILE --mode single|team --workers 2 --rounds 2 [--check NAME]*
workflow status ID
workflow plan ID --file PLAN
workflow ready ID
workflow attempt-start ID --file CLAIM
workflow attempt-propose ID --file PROPOSAL
workflow attempt-fail ID --file FAILURE
workflow attempt-preview ID --file ATTEMPTS
workflow attempt-apply ID --file ATTEMPTS
workflow attempt-refresh ID
workflow attempt-recover ID
workflow review ID --file REVIEW
verify CHECK
workflow verify ID --file VERIFICATION
workflow qa ID --file QA
workflow replan ID
```

Plan: `snapshot`, `tasks`, `checks`, `acceptance`, optional `verification` (required
by the built-in model transport). A task has `id`, `title`,
`instructions`, `paths`, optional `context`, `depends`. Start attempt:
`{task,owner}`. Proposal: `{attempt,snapshot,summary,edits}` with native edit
objects including original `expected_hash`. Failure: `{attempt,error}`. Apply: `{attempts:[IDs]}`. Reports stay outside
indexed source or under `.ai-env/` so they do not stale their own snapshots.

Review/QA reports have `snapshot`, `verdict` (`accept`/`changes`), `findings`,
optional affected `tasks`. QA adds actual `proofs` IDs and `assessment` evidence (required in single mode).
The built-in model transport requires a nonempty bounded `assessment` from both
reviewer and QA and keeps it in the role's answer artifact. Other controller
integrations may supply optional review assessment evidence as well.
Verification reports contain `{snapshot,proofs}` and persist the verified stage
before the final assessment. `workflow list` and `handoff` summarize resumable
progress without replaying source or model transcripts. Team mode requires review;
single mode goes directly to checks/QA after applying the sole task. Preview/direct
batch application is available to integrations that already provide native edit
batches; native scope, snapshot and proof gates still apply. Use the attempt commands
for durable execution ownership and proposal recovery.
