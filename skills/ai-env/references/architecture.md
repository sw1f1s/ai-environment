# Focused architecture review

Use this reference for a change whose module boundaries, public contracts or dependencies matter. It supplements review of the actual code and tests. It does not require another agent, a new framework or a full repository audit. Small fixes need only the relevant checks below; single-executor workflows apply them during QA, while team workflows use their existing independent reviewer.

## Review the change in its project

Start from the original request, task contracts, changed code, directly affected callers and existing neighboring implementations. Explicit project rules govern. Infer a convention only from concrete examples and label uncertainty. A source graph helps locate relationships; missing or partial edges are read gaps, not evidence that no dependency exists.

Review these questions only where the change makes them relevant:

| Concern | Concrete review question | Useful evidence |
| --- | --- | --- |
| Responsibilities | Does the change keep an existing responsibility with its owner, or place domain decisions in transport/UI/storage code without a reason? | Changed functions and an analogous existing path |
| Dependency direction | Does a lower-level policy now import a router, service instance or application entry point? Does the change introduce a cycle or couple independent modules? | Actual imports/calls/types and the project's established direction |
| Authoritative rules | Do all affected consumers use the existing policy and units, rather than copy a formula or call a similarly named unrelated helper? | Qualified symbols, consumer call sites and policy-change tests |
| API and errors | Is the new contract explicit, including named fields, validation, failure behavior and supported callers? | Definitions plus callers, contract tests and error cases |
| Effects and state | Are validation and fallible calculations completed before mutation? Are audit, stock, money and external effects consistent on success, rejection and retry? | Operation ordering, state snapshots, boundary/idempotency tests |
| Extension cost | For one realistic change of the requested rule, is its ownership still clear, or must several consumers change the same decision? | A brief dry run over current code, not a speculative implementation |
| Testability | Can the changed rule be tested independently where appropriate, and do tests also cover its integration? Does the approved runner actually execute those tests? | Test source, configured runner, discovered/executed cases and fresh proof |

When an interface or subtype changes, compare implementations with caller
assumptions: accepted inputs, results, failures and effects must still satisfy the
shared contract. Check whether consumers are forced to depend on unrelated
operations. These are concrete substitution and interface-size questions, not a
requirement to introduce an interface for every function.

An existing project may intentionally use a different dependency direction or tightly coupled design. Do not block a correct local change merely because a preferred architecture would be different. Moving code, adding interfaces or splitting files is justified by a demonstrated problem, not by a pattern's name. A class, a function and a small module can each be the right abstraction.

## Use a small extension dry run

For an existing merchandise quote change, ask: “If the requested discount rule changes again, where would the rule and its tests change, and which consumers should continue working unchanged?” Trace the current authoritative pricing function through reservation, reporting and protocol output. Distinguish a consumer reading `quote.total` from a consumer recomputing the discount. Check that shipping and coupon policies with similar names remain separate.

This is a reasoning check, not a new feature request. Do not implement a hypothetical discount, plugin system or generic policy registry. Do not require consumers to stay unchanged when the actual public contract legitimately changes. For a new named Quote field, the correct outcome may instead be an explicit list of callers that must adapt, verified by compilation and contract tests.

For a stateful release operation, trace both a successful path and an error after a potentially failing calculation. Repricing must use the reservation's original inputs where required; full release must avoid calling positive-only quote with zero; a rejected operation must leave stock/order/counters/audit unchanged. These are observable contract properties, not preferences about file layout.

## Make findings actionable

Report only findings supported by the current change. State the following content in the host's supported report format; this list does not introduce a separate API schema:

- **Disposition:** blocking or advisory. Blocking means a concrete requirement, invariant or established integration contract is violated. Advisory means a nonessential improvement with an explicit benefit/tradeoff.
- **Location:** repository-relative path and one-based line in the reviewed source; include a related caller only when needed.
- **Ownership:** affected existing task ID or IDs from the supplied contracts. A file name is not a task ID.
- **Consequence:** the trigger and resulting incorrect behavior or dependency problem. “Not clean” and “not extensible” are insufficient.
- **Minimum correction:** the smallest change that restores the requirement or boundary. Do not prescribe a broad rewrite when a local fix suffices.
- **Verification:** distinguish static reading, a test that exists but was not run, and an actual fresh proof. Cite only supplied proof IDs or an actual recorded execution; say what remains untested.

Example of a blocking finding:

> `src/service.py:45`, task `release`: release duplicates the merchandise discount instead of using the required existing PricePolicy. A later change to that authoritative policy changes QUOTE/RESERVE but leaves the released remainder on the old rule. Reuse PricePolicy with stored unit price and tier before mutating stock; preserve the zero-remainder case. Evidence: static call-site inspection. Verification needed: approved regression runner exercising the changed shared policy, original-price repricing and rejection without mutation. No execution claimed.

The path/line above is illustrative, not a finding against a current repository file. A concrete report must use the actual reviewed location. A similarly worded finding is invalid if the code already calls the authoritative policy, or if the project explicitly requires a separate rule.

Example of an advisory:

> Two protocol branches obtain the same quote independently. A shared small formatting path could reduce repetition, but current behavior and policy ownership are correct. Optional local simplification; no functional repair required. Static observation only.

A real violation must not be downgraded to advisory merely because ordinary tests pass. Conversely, a test gap is not proof that a bug exists: identify the missing required behavior or state the uncertainty. A passing check establishes the tested cases, not complete architectural quality.

## Route findings without losing work

A blocking finding requests changes and names the task owning the defect. Rust invalidates that task and its dependent closure; independently applied work remains. If the finding cannot be localized after inspecting contracts, report that explicitly so the conservative broader repair is understood. Do not send an empty target list as a shorthand for a known single task.

Advisories accompany acceptance in the assessment or the host's dedicated advisory field. They do not request changes, invalidate tasks or dispatch more developers. A report containing both kinds requests repair only for the blocking findings. Avoid serial rounds for cosmetic preferences.

For a small single-executor change, the existing QA role checks relevant ownership, dependencies and effects against acceptance and actual proofs. It must disclose that there was no independent code-review role. Do not create a reviewer merely to satisfy this reference. In team mode, the independent reviewer inspects the combined result; per-developer summaries are claims and do not replace reading the integration.

Changing a repair's implementation is different from changing the plan. Replan only when responsibilities, interfaces or dependencies in the plan are themselves wrong. Source-bound checks must run again on the resulting combined snapshot; preserving another task's implementation does not preserve an old global proof.

## Behavioural evaluation of this review path

Use deterministic controller tests for routing and a separate bounded real-role trial for whether the model detects the defect. Freeze the fixtures and criteria before the trial. These scenarios test the mechanism without manufacturing a quality score:

1. **Targeted architecture blocker.** Prepare task A owning the shared policy, its consumer A2 depending on A, and independent task B. The reviewer finds a real duplicated-rule defect in A and targets A. Assert that only A/A2 are invalidated and re-executed; B's source and successful attempt stay intact. After repair, review and all required checks evaluate the new combined snapshot.
2. **Advisory only.** Provide a correct implementation with a small optional extraction opportunity. The report accepts with the advisory; no developer retry or repair round follows. An advisory must not silently become a blocking finding through a string parser.
3. **Mixed findings.** One blocking dependency violation and one unrelated formatting preference produce targeted repair for the former only. The latter neither expands write scope nor invalidates its owner.
4. **Single-path assessment.** A small change is reviewed by its existing QA stage for an actual policy/effect issue. Assert that no architect or reviewer is dispatched merely for architecture checks; a real blocker still requests the responsible task's repair.
5. **Already reused policy.** Supply a superficially similar consumer that correctly calls the authoritative policy, plus an unrelated similarly named helper. Static review must not invent duplication or require the unrelated module to change. A hidden policy mutation can independently test actual reuse after development.
6. **Test evidence gap.** Add a meaningful regression outside the approved runner's discovery. QA distinguishes “test written” from “test executed”; it cannot cite that test as passing. In a separately preconfigured fixture, a deliberately failing discoverable test must produce a real failed proof and a targeted correction before acceptance.
7. **Contract versus preference.** The same local structure is acceptable under one project's explicit pattern and a violation under another project's established dependency rule. The finding must cite the relevant evidence rather than apply a universal layering template.

Record reached roles, actual task generations/attempts, preserved source hashes, report disposition, proof freshness and model usage. Keep static findings separate from executed failures. Retain failed trials and unknown evidence. Successful routing demonstrates orchestration behaviour; a wider repeated development experiment is still required to establish fewer defects, less rework or lower token cost.
