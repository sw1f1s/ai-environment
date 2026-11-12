# Product focus and next decision

AI Env's primary hypothesis is better consistency when coding agents extend an
existing repository: reuse the actual responsibility owner, preserve the local
approach and avoid unnecessary competing implementations. Functional correctness
is required. Tokens and duration remain visible costs.

**This benefit is not established.** Source visibility, valid static graphs,
persistent task state and passing tool tests demonstrate mechanisms. They do not
prove fewer architectural mistakes, better understanding or lower maintainer
rework. Current documentation makes no comparative efficiency or quality claim.

## Current scope

Keep the existing source/context tools, compiler adapters, scoped edits and
recorded check proofs available. Preserve the Rust runner's recovery and execution
gates. Broad orchestrator expansion is paused while the consistency hypothesis
is investigated. Additional roles, a new language or an editor are outside this
next step.

Current context selection is lexical and structural. It supplies candidates,
nearby examples and available static relationships. It does not establish active
ownership, semantic equivalence or which local pattern a new feature should use.

## Next step: diagnose one real maintenance failure

Choose a concrete existing-project change for which a maintainer can explain a
necessary correction: a duplicated service, bypassed shared policy, inappropriate
legacy pattern or inconsistent failure/state handling. Preserve the source before
the change, original request, agent result and the maintainer's correction.
Do not select a case merely because it creates a favorable comparison.

Determine the cause before changing the product:

| Observed cause | Question to answer |
| --- | --- |
| Existing capability was not found | Which owner, consumer or example was missing from the evidence available to the agent? |
| Contract was misunderstood | Was the actual behavior, error/effect contract or extension constraint visible and correctly interpreted? |
| An incompatible approach was chosen despite visible evidence | Which decision or diff check could have identified the concrete consequence? |
| Both ordinary development and AI Env already follow the project | Is there an actual failure for this mechanism to address? |

Keep missing evidence distinct from evidence that was present but ignored.
Navigation changes alone cannot establish a remedy for all four situations.

## Candidate intervention

A useful context chain may be:

**responsibility owner → contract → active consumer → analogous extension → test**.

This is a proposed design target, not a completed ownership model. Every proposed
relationship needs current source locations, scope and an explanation of why it
applies. Explicit applicable rules outrank inferred examples. Deprecated paths,
conflicts and uncertainty must remain visible. Different names may own the same
responsibility; same-name helpers may implement incompatible contracts.

Choose one intervention after diagnosis. Avoid building a global unverified rule
catalog or requiring a new model role for every change. Include the effort of
preparing and maintaining any project-specific evidence in its cost.

## Continue or stop

Use one development case to design the mechanism, then check it on separate
maintenance tasks that were not used to tune it. Fix requirements, allowed tools,
model/profile, evaluation rules and spending limits before execution. Preserve
unsuccessful and inconclusive outcomes.

Continue only when separate tasks show fewer independently confirmed unnecessary
duplicates or unjustified approach deviations, with complete delivery and no
added material defect. Describe the actual maintainer corrections prevented;
do not substitute line counts, source visibility, test counts or lower tokens.
A tie is not a consistency improvement. If the targeted intervention repeats the
same mistakes or has no relevant advantage, stop or narrow this direction.

[Consistency workflow](PROJECT-CONSISTENCY.md) ·
[Evaluation method](EVALUATION.md) · [Tool verification](VERIFICATION.md).
