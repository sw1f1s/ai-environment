---
name: ai-env
description: Extend existing repositories with task-specific code context, reuse of existing capabilities, local project conventions and a durable adaptive Rust agent workflow.
---

Find existing implementation and define acceptance before editing. Choose the
smallest useful team: one executor for a small change; architect → scoped
developers → reviewer → QA for work requiring independent parts or review.
Read [orchestration.md](references/orchestration.md) when dispatching models with
the Rust runner. Reuse its persisted workflow instead of recreating completed
tasks. When assigned one role, perform only that role; do not start a nested team.
For changes to module boundaries or public contracts, record responsibilities,
dependency rules and a plausible extension dry run; small fixes need no invented
future scenario. Review actual code against those boundaries. Blocking findings
need a current location, owner, consequence, minimal fix and verification;
advisories do not trigger repair. Use [architecture.md](references/architecture.md)
for this review, within the existing reviewer or single-mode QA call.

Parallel work needs independent contracts and read/write scopes. Implement shared
interfaces before their consumers; different filenames alone do not prove independence.

Before adding or changing a responsibility, inspect its existing owner, contract,
callers and an appropriate local implementation/test example. Make a concise
reuse/extend/new decision with source references; explain new code when existing
contracts do not fit. Keep it in the current task or plan, without another model
call. Preserve the subsystem's API, errors, state/effect handling and test approach,
not just formatting. Review the actual diff against those examples and check for
duplicated existing responsibilities. Candidate matches are navigation evidence,
not authoritative owners. Different local patterns or a justified new abstraction
can be correct. For uncertain ownership, competing patterns or a substantial
extension, use [consistency.md](references/consistency.md).

For source navigation, use a current work packet already attached by the host;
otherwise request one packet:

```sh
python3 <skill-directory>/scripts/ae.py --root <repository> brief --impact "task keywords"
```

It includes source, an existing HANDOFF.md and configured checks. Impact packets also supply bounded task matches and local source/test examples with reasons and source references; inspect their contracts before choosing an approach. Missing candidates or omitted examples do not prove that a capability is absent. Small projects use direct source; larger packets select sources using search and available fresh compiler graphs. Large impact packets can refresh an existing graph after source-only changes within bounded limits; missing graphs, changed analysis configuration and failed refreshes remain explicit gaps. Large packets also identify callers, types, test paths and read gaps within the same budget. Coverage and omitted files/lines are explicit. Read missing code as needed; avoid rereading source already in the packet. Graph selection is not exhaustive runtime reachability.

`profile --path PREFIX` supplies scoped formatter/linter configs, neighboring
style evidence and a global test inventory. Explicit repository rules govern;
inferred observations are examples, not invented policy. Preserve local naming,
layout and line endings. Reuse unit/integration suites and add regression cases
for changed behavior in the approved runner’s actual discovery paths. Merely adding a test file is not execution evidence; inspect the check driver. Discovery is not a passing test result.

In an orchestrated run executors return scoped proposals; Rust persists each
attempt before applying edits through snapshot/file CAS. Applied work is not
completed work: evaluate the combined result against the original request and
fresh approved checks. Repair affected tasks and their consumers, preserving
unaffected work. Replan only when the plan is wrong; repeated identical failure
without progress requires reassessment. A configured check may already build and run public tests;
avoid duplicate builds. A large impact packet handles eligible source-only graph
refreshes. Refresh explicitly when a reported analysis gap needs correction
before another semantic query.

In the native runner, return text edits without file hashes; Rust binds the
original dispatched revision. Select plan `read_refs` and QA `source_refs` from
the host catalogs; Rust derives scopes and locations. Plans record a few
change-specific verification obligations. QA reports actual evidence or a gap
for each, within the existing role. Selector-only corrections preserve the saved
code and assessment; unresolved evidence requires reassessment. Static inspection
is not test execution; all approved checks remain required.

Load references only for an operation you actually need: [semantic.md](references/semantic.md) for graph setup/precise impact; [commands.md](references/commands.md) for adoption, CAS edits, cooperating task claims or source-bound notes/proofs. For existing AI Env tasks/notes use `handoff`; stale state needs source inspection. Do not replace the project's language, tests or editor.

Hosts can attach the packet before a model request with `scripts/prepare_task.py`:
see [hosts.md](references/hosts.md) when integrating a host, not on ordinary edits.
