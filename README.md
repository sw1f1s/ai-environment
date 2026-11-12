<h1 align="center">AI Env</h1>

<p align="center">
  <strong>Repository context and persistent workflows for coding agents.</strong>
</p>

AI Env is a development skill with a Rust toolkit. It provides bounded source
context, static code relationships, local project conventions and persistent
task/check state. Use it with an existing language, editor and build system.

**Experimental:** improved project consistency, fewer coding errors and lower
development cost have not been demonstrated. Working navigation and execution
mechanisms are not evidence that an agent makes better architectural decisions.

<p align="center">
  <a href="#quick-start">Quick start</a> ·
  <a href="#capabilities">Capabilities</a> ·
  <a href="#languages">Languages</a> ·
  <a href="#current-direction">Direction</a> ·
  <a href="#documentation">Docs</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/status-experimental-d29922?style=flat-square" alt="Status: experimental">
  <img src="https://img.shields.io/badge/core-Rust-b7410e?style=flat-square&amp;logo=rust&amp;logoColor=white" alt="Core: Rust">
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux-586069?style=flat-square" alt="Platforms: macOS and Linux">
</p>

## Capabilities

- **Task context:** source excerpts, implementation candidates, nearby examples,
  related tests and applicable instructions within explicit byte budgets.
- **Static navigation:** symbols, calls, callers and type relationships from
  language-specific analyzers, with freshness checks and coverage limits.
- **Project conventions:** scoped formatter/linter settings, source examples and
  test discovery. Existing project rules remain authoritative.
- **Persistent progress:** task contracts, dependencies, attempts, proposals and
  source-bound notes stored under `.ai-env/`.
- **Controlled changes:** Rust validates scopes and original revisions, applies
  proposals and records real configured checks against the combined source.
- **Optional orchestration:** one executor or architect → scoped developers →
  reviewer → QA. Repairs preserve unaffected work; unchanged failures stop.

Candidate names and graph edges do not establish semantic equivalence or the
correct owner of a responsibility. Agents must inspect contracts and consumers.
The [consistency workflow](skills/ai-env/references/consistency.md) describes that
reasoning without requiring an additional model role.

## Quick start

Build requirements: **Rust**, **Python 3.11+** and, for the default C# helper, a
local **.NET 8 SDK**. The MVP supports macOS and Linux. Analyzer-specific tools
are listed below.

```sh
git clone https://github.com/sw1f1s/ai-environment.git
cd ai-environment
python3 scripts/dev.py build
python3 skills/ai-env/scripts/ae.py --root /path/to/project init
python3 scripts/install_skill.py --project /path/to/project
```

The installer creates `/path/to/project/.agents/skills/ai-env` and refuses to
overwrite an existing installation. Open the target project in a host supporting
repository skills, then ask:

```text
Use $ai-env to add the new import format through the existing pipeline.
Inspect its extension contract and an active adapter, preserve the project's
error handling and verify the change with the existing tests.
```

Use `build --offline` with cached Rust dependencies, or `build --without-csharp`
to omit the C# helper. Packaged binaries target the build OS and architecture.

<details>
<summary><strong>Configure checks and use the native runner</strong></summary>

Name actual project commands in `/path/to/project/.ai-env/config.json`. For a
project using Python unittest:

```json
{
  "checks": {
    "test": {
      "argv": ["python3", "-m", "unittest", "discover", "-s", "tests"],
      "timeout_seconds": 120
    }
  }
}
```

Use the project's existing runner. Discovery alone is not execution. Native
orchestration stops before model dispatch when it discovers tests but has no
approved check commands.

Write your request to a text file, then start or resume from this repository:

```sh
skills/ai-env/scripts/bin/aienv --root /path/to/project \
  orchestrate import-format --request /tmp/request.txt --mode auto

skills/ai-env/scripts/bin/aienv --root /path/to/project \
  orchestrate import-format --resume
```

The installed executable is `.agents/skills/ai-env/scripts/bin/aienv`. Rust owns
scheduling, state and applying changes. Model sessions are read-only and return
structured proposals. Progress moves through proposed → applied → verified →
completed. Resume retains recorded work and cumulative usage.

| Mode | Execution |
| --- | --- |
| `auto` | A coordinator chooses single or team execution. |
| `single` | One executor plans and implements; checks and QA follow. |
| `team` | Architect → scoped developers → independent reviewer → checks and QA. |

[Task contracts, recovery and limits](skills/ai-env/references/orchestration.md).

</details>

## Hosts

| Integration | Available today |
| --- | --- |
| Codex CLI | Native dispatch through an existing authenticated installation, with usage and execution limits. |
| Repository-skill hosts | Skill instructions and packaged tools; discovery and permissions depend on the host. |
| Other hosts | Direct CLI tools and an optional context-preparation hook. |

Model dispatch consumes the host's allowance. The runner does not provision API
keys or change IDE settings. [Host integration](skills/ai-env/references/hosts.md).

## Languages

| Language | Semantic backend | Host requirements |
| --- | --- | --- |
| C / C++ | libclang | Compatible libclang and project compilation settings |
| C# | Roslyn | .NET 8 runtime; SDK to build the helper |
| Rust | rust-analyzer | rust-analyzer and rust-src |
| Python | Jedi / Parso | Host Python; dependencies bundled |
| TypeScript / JavaScript | TypeScript Compiler API | Node.js; TypeScript bundled |

Source discovery, literal search, scoped edits and configured checks work
independently of language. Static graphs report incomplete coverage and freshness;
Python inference is always partial. [Semantic setup](skills/ai-env/references/semantic.md).

## Current direction

The product hypothesis is **better consistency when extending existing projects**:
reuse the actual responsibility owner and preserve the affected subsystem's API,
errors, state handling and test approach. This benefit remains unproven.

The next step is a bounded diagnosis of a real maintenance failure: identify
whether the agent missed existing code, misunderstood its contract or chose an
incompatible approach despite having the evidence. A proposed context chain is
owner → contract → active consumer → analogous extension → test. It is a design
target, not an implemented ownership model.

Broad orchestrator expansion is paused. Develop a targeted mechanism only after
the diagnosis, then check transfer to separate tasks. Continue only if it prevents
confirmed unnecessary duplication or unjustified deviations with functional
correctness preserved. Tokens and time remain accounted costs; they do not
substitute for better code. [Scope and decision rules](docs/AI-ENV-FOCUS.md).

## Development

Build the bundle, then run product checks:

```sh
python3 scripts/dev.py build --offline
python3 scripts/dev.py check --offline
```

Checks cover Rust state/transactions, native integration and actual semantic
backends. Missing required analyzers fail strict validation. These checks verify
mechanisms; they do not establish better agent decisions. [Coverage](docs/VERIFICATION.md).

Jedi, Parso and TypeScript are bundled with upstream licenses. Versions and
hashes are pinned in `skills/ai-env/vendor-lock.json` and verified during
build/check; `python3 scripts/vendor.py` checks them separately.

Snapshots cover indexed text/configuration, not the entire filesystem. Generated
files, links, credential-like names, binaries and oversized/non-UTF8 inputs are
omitted. Analysis does not execute target modules, build scripts or plugins.
Configured checks run with ordinary user permissions. Source transactions handle
ordinary IO rollback; power-loss atomicity is not guaranteed.

## Documentation

[Skill](skills/ai-env/SKILL.md) · [Commands](skills/ai-env/references/commands.md) ·
[Architecture](docs/ARCHITECTURE.md) · [Consistency](docs/PROJECT-CONSISTENCY.md) ·
[Review criteria](skills/ai-env/references/architecture.md) ·
[Verification](docs/VERIFICATION.md) · [Evaluation method](docs/EVALUATION.md) ·
[Issues](https://github.com/sw1f1s/ai-environment/issues)
