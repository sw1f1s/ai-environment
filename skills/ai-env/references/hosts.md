# Prepare context at dispatch

For adaptive Rust dispatch with persisted tasks and attempts, use
[orchestration.md](orchestration.md). This page covers context-only preparation,
which makes no model calls.

Interactive navigation spends a model turn discovering the tool and another
requesting source. A host that already knows the task and repository can prepare
the initial source packet without an inference call:

```sh
python3 -I <skill-directory>/scripts/prepare_task.py \
  --root /path/to/project --task /path/to/request.txt
```

The project must already be initialized and its named checks configured. The
helper prints the original task plus the current bounded `brief` packet and
short guidance. `--task -` reads UTF-8 stdin (up to 64 KiB). The shared Rust
`query` command derives navigation terms from the entire task: short tasks keep
their words, while long tasks select whole identifiers and varied lexical terms
across the request, bounded to 64 terms and 2048 UTF-8 bytes. This deterministic
selection does not infer semantic importance or guarantee every constraint is
represented. The original task is attached in full. `--query 'symbol keywords'`
overrides selection precisely; surrounding whitespace is trimmed, internal text
is preserved, and empty, multiline or oversized overrides fail explicitly.
Native orchestration uses the same projector. Large source sets use
`brief --impact` to attach fresh cached
caller/type/reuse evidence and locations of related tests, including outside
source scope. The map shares the source byte budget and explicitly reports
unknown/stale relationships and omissions. Small packets still read sources
directly. For large impact packets, existing source-stale graphs may refresh
using their recorded options within a shared 20-second deadline and at most two
attempts. Changed configuration/provenance or a failed refresh falls back to
lexical selection with a reason; missing graphs are never built automatically.
`--budget` is a UTF-8 byte limit for context, not model tokens.
Use `--scope src` (repeatable) when `src` is the project's actual source root;
include other required roots, such as `include`, explicitly. The handoff and
check argv and small indexed local check drivers remain available. Sources outside those prefixes are omitted with
visible coverage counts and remain readable with ordinary tools. This is a
context projection, not an edit permission boundary or complete impact proof.
It never calls a model, starts another agent, installs dependencies or executes
target project code. Errors produce no partial task prompt.

Preparation also discovers project `AGENTS.override.md`/`AGENTS.md` in the root,
directories leading to each scope, and selected subtrees. The first nonempty
override/base file per directory is attached with its applicable directory;
deeper rules govern that subtree. Generated directories and directory links are
excluded explicitly. Symlinked/hardlinked rule files, incomplete reads and more
than 32 KiB of rules fail preparation rather than silently dropping instructions.
Rules are separate from the source packet and its byte budget. The host still
supplies global/profile instructions and custom fallback filenames; this helper
does not reproduce a host's full configuration. Avoid attaching duplicate rules
if your host already supplied them. Inspect instructions before editing outside
the declared coverage. Discovery is repeated before publishing to detect changes.

Attach stdout to the host's initial agent request. For example, the existing
Codex CLI can consume it over stdin:

```sh
set -o pipefail
python3 -I <skill-directory>/scripts/prepare_task.py \
  --root /path/to/project --task /path/to/request.txt \
  | codex exec --sandbox workspace-write -C /path/to/project -
```

Run under a host pipeline with `pipefail`, or call the helper as an argv
subprocess and check its exit status before contacting the model. The helper
does not select a model or change the host's permissions/authentication.
Repository instructions and ordinary tools remain in force. The packet is
source data at its stated snapshot; reread missing/changed code as needed.
Its selection never proves exhaustive dynamic impact.

Prepare a fresh packet for a fresh session or a new external source edit;
do not attach a packet saved from an old checkout. Charge the preparation
latency and every attached token to development. This integration is distinct
from invoking the skill interactively after a model request has already begun.
Hosts without dispatch hooks can continue using `brief` interactively.
