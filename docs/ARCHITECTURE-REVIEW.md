# Architectural review

Plans record responsibility boundaries, allowed dependency directions and a
relevant extension scenario when the change warrants one. The scenario is a dry
run against current code, not an instruction to implement speculative features.

Review evaluates the actual combined change. Team mode has an independent
reviewer; single mode uses the existing QA assessment. A material finding contains
severity, actual responsible task IDs, a current source location, the concrete
consequence, minimal correction and verification. Advisory findings are retained
without starting repair; blockers return the owners and dependent consumers.

[Criteria and examples](../skills/ai-env/references/architecture.md) live in the
installable skill. [Native lifecycle](../skills/ai-env/references/orchestration.md)
describes role output and repair. [Consistency](PROJECT-CONSISTENCY.md) covers
applicable local examples and existing responsibilities.

Rust validates report shape, task/path/line eligibility, freshness and routing.
The model makes substantive judgments. A principle name or stylistic preference
without a reachable consequence is not a blocker. Legitimate new abstractions
and locally appropriate patterns must remain acceptable.

Controller tests cover addressed repairs, retained independent work, advisories,
single/team gates, stale assessments, contract changes and Unicode boundaries.
These checks establish mechanism behavior; they do not prove fewer architectural
mistakes in agent-generated code. [Verification](VERIFICATION.md).
