# Migrating from `mpg` (mind-palace-graph) to `scrt`

`scrt` grew out of the Node `mpg` CLI and stays a **near drop-in** for the
common path: the JSON envelope, palace file format, flag surface, and
exit-code contract all match, and the five original tools keep their names
(scrt adds a sixth, `scrt_similar`). Byte-for-byte parity is no longer an
enforced *goal* — scrt is its own product now — but the migration boundary
below is what existing mpg users and harnesses can rely on.

> Behaviour is guarded by checked-in tests: `crates/scrt-core/tests/
> roundtrip.rs` (76 search/format cases), `palace_migration.rs` (a
> Node-written palace round-trips byte-for-byte), and `tool_spec_parity.rs`
> (the tool-spec contract — the 5 mpg tools remain a recognizable superset).
> What round-trips and what's excluded is specified in
> [COMPAT.md](./COMPAT.md).

## Drop-in: what's identical

- **Every CLI flag** in the v0.x "Command reference" — same names,
  defaults (effort `quick`, strategy `fill`, format `llm`), greedy `--in`
  / `--mp-compose` / `--mp-intersect` / `--mp-except`, `@file`/`@-`
  indirection, comma lists.
- **All output formats** (`llm` / `markdown` / `json` / `text` /
  `agent-json`) byte-for-byte (modulo branding, below).
- **Palace file** (`.mpg/mind-palace.json`) — same schema, version 2;
  files written by either tool open in both.
- **tool-spec** for OpenAI / Anthropic / Gemini — the five mpg tools keep
  their schemas and names (renamed `mpg_*` → `scrt_*`); scrt adds a sixth,
  `scrt_similar` (see "What's new" below).
- **Env vars** `MPG_MIND_PALACE` and `MPG_PATTERN`, and the `.mpg/`
  default directory — kept as-is so existing configs resolve unchanged.
- **Exit codes** `0` (match) / `1` (no-match) / `2` (bad-args) / `4`
  (palace-error) / `99` (unexpected).

## What's new (beyond mpg)

scrt extends the surface — none of this breaks an mpg workflow:

- **Similarity & link discovery.** `scrt --mp-similar <stash>` / `--term`
  ranks stashes by lexical similarity (`--match note|full|vector`); every
  `--mp-stash` suggests related stashes to link. Exposed to agents as the
  `scrt_similar` tool and the `palace.similar` server method.
- **In-memory palaces + multi-tenancy** via the engine transports
  (`--serve` / NAPI / PyO3), alongside the on-disk palace mpg users know.

## Intentional differences

### 1. Exit code 3 is gone

v0.x exits `3` when `ripgrep` is not installed. **scrt owns the regex
engine** (the BurntSushi `grep` crates, in-process) — there is no `rg`
dependency, so the failure mode that code 3 reported cannot occur. Code 3
is never returned. Scripts that branch on `=== 3` should treat it as
unreachable; all other codes are unchanged.

### 2. `--ls` / `--tree` no longer shells out to `rg --files`

v0.x's `--ls` spawned `rg --files`. To keep scrt free of any `rg` runtime
dependency, `--ls` lists files via the same `ignore` walker scrt uses for
searches. Output is the set of searchable files, honoring `.gitignore` /
`--hidden` / `--no-ignore` identically — the file *set* is the same; the
*order* may differ (see #4).

### 3. Branding: `scrt` in user-facing output

Per the project rename, scrt emits `scrt` where v0.x emitted `mpg` in
**user-facing output**:

| Location | v0.x | scrt |
| :--- | :--- | :--- |
| tool-spec tool names | `mpg_search`, `mpg_stash`, … | `scrt_*` |
| `llm`/`text` result block tag | `<mpg result …>` | `<scrt result …>` |
| palace block tags | `<mpg mind-palace …>` etc. | `<scrt …>` |
| stderr confirmations | `mpg: …` | `scrt: …` |

**Not rebranded** (byte-identical): the on-disk palace file (carries no
brand string), tool-spec JSON Schemas and descriptions, the
`MPG_MIND_PALACE`/`MPG_PATTERN` env vars, and the `.mpg/` directory.
Harnesses that match on the old tool names need to update to `scrt_*`;
everything that reads the palace or env is unaffected.

### 4. Directory-walk order may differ (same content)

For a **single file**, scrt's node order is byte-identical to v0.x. For a
**directory** search, rg and the `ignore` walker visit files in different
orders, so the *sequence* of nodes (and therefore which nodes land on a
given `--page`) can differ — though the **set** of nodes, their counts,
tokens, and content are identical. Pass `--sort recent|oldest` for a
deterministic, tool-independent order. (This is why the parity sweep
compares directory cases as sets and single-file/sorted cases exactly.)

### 5. `--no-auto-tune` accepted but inert

The v0.x wide-record auto-tune (shrinking before/after to 0 on JSONL-style
corpora) is not yet ported; `--no-auto-tune` is accepted as a no-op so
existing command lines don't error. `auto_tune_applied` is therefore never
`true` in scrt output (it's omitted, exactly as v0.x omits it when the tune
doesn't fire). Tracked for a later pass.

### 6. agent-json envelope opts (unchanged behavior, noted for clarity)

Like v0.x, the **CLI** builds the `agent-json` envelope with no extra opts,
so `--fuzzy` / `--no-fill` never change `fallback_used` or add the
strict-mode warning *from the CLI*. Those opts are threaded only by the
warm-process **server** (`scrt --serve`, `format: "agent-json"`). This
matches v0.x exactly (see COMPAT.md §3); flagged here only because it
surprises people.

## Performance

Measured on the same machine, release build, a 3-file corpus, `--effort
scan` (p50 of 5 runs):

| Workload | Node `mpg` | `scrt` | Speedup |
| :--- | ---: | ---: | ---: |
| **Cold-start** CLI invocation | ~92 ms | ~16 ms | **~5.7×** |
| **Warm** in-process `dispatch` (server) | — | p50 3.3 ms | (no Node equiv) |

The cold-start win comes from eliminating Node startup *and* the `rg`
subprocess spawn. The warm path (`scrt --serve` / the NAPI addon) removes
per-call startup entirely — a JS harness calling the NAPI `dispatch`
skips the subprocess boundary altogether. Larger corpora widen the gap
further (the in-process `grep` search has no per-call process spawn).

> These are indicative, not a benchmark suite. A formal perf scorecard is
> deferred; when it lands it will report any workloads where scrt loses,
> honestly.

## Quick switch

```bash
# Node:
mpg "TODO" --in src/ --effort scan --clip 30

# scrt — same flags, same output (scrt-branded block tag):
scrt "TODO" --in src/ --effort scan --clip 30

# tool-spec: update the tool names mpg_* -> scrt_* in your harness.
scrt tool-spec --format anthropic > tools/scrt-anthropic.json

# Existing palace files just work:
MPG_MIND_PALACE=./.mpg/mind-palace.json scrt --mp-list
```
