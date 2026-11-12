# Project consistency and reuse

The objective is to extend an existing project through its established owners and
appropriate local approach. This includes responsibilities, dependency direction,
API/errors, state/effects and test idioms as well as formatting. The hypothesis
that AI Env improves those decisions remains unproven.

## Current implementation

`brief --impact` supplies bounded implementation, local-example and related-test
candidates with source locations, selection reasons, visibility and omissions.
Selection uses lexical evidence, locality, filename conventions and available
fresh static graph edges. Exact structured identifiers and explicit canonical
relative paths receive priority. Excerpts reserve strong matching lines before
surrounding windows; a late definition can therefore survive early generic hits.
Scopes and UTF-8 byte budgets remain enforced.

This is navigation. A declaration-shaped line is a lexical hint, not a proven
owner. Similarity and proximity do not establish semantic equivalence, active
usage or correct architecture. Missing candidates do not establish absence.
`profile` exposes explicit configuration, source conventions and test discovery;
it does not derive a complete project development policy.

The skill asks the current executor to make a concise reuse/extend/new decision
and compare the actual diff with appropriate examples. Native plans and executor
summaries retain decisions in existing artifacts; review/QA receive those claims,
current source and diff. No additional role is required by this mechanism.
Summaries are not source evidence or executed-test proofs.

## Example

A request adds another vendor's import format. The project already has an
`ImportPipeline` owning validation, transactions and idempotency, an adapter
contract, a shared error type and an active neighboring adapter/test.

The expected extension supplies vendor conversion through that contract and
wires the existing registration path. A second service writing directly to
storage can pass its happy-path test while bypassing the project's policy.

If the new format requires a lifecycle the adapter cannot support, extending its
owner or introducing a separate contract may be justified. Forced reuse is not
consistency. Confirm actual consumers and contract fit before choosing.

## Workflow using existing roles

1. Locate the likely owner, inspect its contract and an actual consumer, then
   inspect an appropriate active implementation/test example.
2. Identify applicable project instructions and explicit configuration. Preserve
   competing legacy/current examples as uncertainty until their scope is clear.
3. State what can be reused, what must be extended and what requires new code.
   Explain a new responsibility when an existing alternative appears relevant.
4. Review the actual combined diff for duplicated responsibilities, dependency
   changes, API/error/state handling and relevant test discovery.
5. Execute real project checks and meaningful regressions. Preserve only useful,
   source-bound decisions for later tasks; re-inspect stale evidence.

A similarly named helper can have different units, errors or effects. A global
majority pattern can be inappropriate for a particular subsystem. A valid local
alternative or a justified abstraction is not a violation.

## Proposed next mechanism

The proposed evidence chain is owner → contract → active consumer → analogous
extension → test. Current candidate selection does not reliably construct or
validate that chain. It needs a diagnosed real maintenance failure before further
implementation. See [scope and decision rules](AI-ENV-FOCUS.md).

Possible future evidence should retain current source references, applicability,
consumer paths and uncertainty. It must not become an unbounded prompt or stale
global policy. A source-bound note only establishes freshness within its recorded
paths; it does not prove the reasoning correct or complete.

## Review standard

A material finding needs the changed location, applicable existing contract or
example, concrete consequence, minimal correction and verification. Naming a
principle such as SOLID is insufficient. Do not penalize valid alternatives or
score quality by class count, code length or pattern familiarity.

The desired result is fewer confirmed unnecessary competing implementations and
unjustified deviations, with complete behavior preserved. Tool tests verify
navigation and controller behavior; they do not establish that result.

[Agent reference](../skills/ai-env/references/consistency.md) ·
[Architecture review](../skills/ai-env/references/architecture.md) ·
[Evaluation method](EVALUATION.md).
