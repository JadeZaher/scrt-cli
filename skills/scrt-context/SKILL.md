---
name: scrt-context
description: >
  Token-budgeted context retrieval with a composable mind palace and
  cheap lexical similarity / link discovery. Search files, command
  output, and URLs for regex patterns; results return as context nodes
  sized in tokens (not lines) with file:line attribution. A persistent
  mind palace of named stashes can be composed, intersected, linked into
  a graph, ranked by similarity, and pruned by age/tag/count. Integration
  paths: CLI shell-out (any agent), `scrt --serve` warm process
  (stdio/HTTP), and in-process Node (scrt-napi) / Python (scrt-py)
  bindings. Use for codebase exploration, multi-step investigation,
  finding references, filtering opaque tool output, and building
  cross-session working memory.
  DO NOT use scrt for: a single known file path (use Read); a single
  symbol grep on a small tree where you expect ≤ ~30 hits (use Grep);
  small files you'll fully consume. The "When NOT to use scrt" section
  makes the cutoff concrete. The similarity signal is LEXICAL, not
  semantic — it matches shared words, not meaning.
tools:
  - scrt_search
  - scrt_stash
  - scrt_list_stashes
  - scrt_get_stash
  - scrt_drop_stash
  - scrt_similar
install:
  source: git clone https://github.com/JadeZaher/scrt-cli && cd scrt-cli && cargo build --release
  verify: scrt --version
---

# scrt-context — Context Retrieval & Orchestration Skill

## The lens mental model

scrt is a single **lens over the corpus**, not a tool you reach for after
grep and read. There are no boundaries between files — you dial the lens
to fit the task:

- **Focal points** = matches (set by the pattern + `sort` + `fuzzy`).
- **Depth at each focal point** = the window (`effort` / `clip_chars` /
  `before` / `after` / `window_curve`).
- **Surface** = where the lens looks (`in`, `mp_from`, `compose`, `page`).

One `scrt_search` call replaces 1–N `grep` + `read` combos:

| Job | Lens setting |
| :--- | :--- |
| "List file:line hits, like grep" | `effort: scan, clip_chars: 30` |
| "Read this one file for what it says about X" | `in: [file], effort: deep` |
| "Browse recent memory for X" | `effort: scan, sort: recent, page: 1` |
| "Compact a topic to N tokens" | `effort: scan, clip_chars: 30, max_tokens: N` |
| "Catch a typo'd term" | `fuzzy: true` |
| "Search only files I already touched" | `mp_from: <stash>` |

The mind palace is persistent state for the lens: stash a result and the
next search can be scoped to those files across the whole corpus without
re-scanning. Similarity (`scrt_similar`) is how you navigate the palace
once it grows.

## When NOT to use scrt

| Situation | Use instead | Why |
| :--- | :--- | :--- |
| You know the exact file path | `Read` | No search needed. |
| One symbol, small tree, ≤~30 hits expected | `Grep` | Budgeting overhead isn't worth it. |
| A file you'll read in full anyway | `Read` | scrt trims; you want the whole thing. |
| Definitions / references / diagnostics | LSP tools | scrt is lexical, not semantic-aware. |
| "Same idea, different words" matching | embeddings (scrt-evolve) | scrt similarity is lexical — see below. |

scrt earns its keep on **>1 KB result sets**, **multi-step investigation**,
and **anything you want to survive compaction**.

## Recurring jobs

### Compaction at near-zero cost
Instead of a summarization round-trip, scan + clip + cap:
```
scrt "<topic terms>" --in . --effort scan --clip 30 --max-tokens 2000
```
Stash it (`--mp-stash compact-<topic> --mp-ttl 6h`) so the next turn
references it instead of re-running.

### Codebase exploration: scan → stash → drill
```
scrt "useAuth|withAuth|authMiddleware" --in . --effort scan --clip 30 \
  --mp-stash auth-surface --mp-tag scan --mp-ttl 4h        # inventory
scrt "useAuth" --mp-from auth-surface --in src/auth/login.tsx --effort deep   # drill cheaply
```
`--mp-from` re-scopes the deeper search to the inventory's files — far
cheaper than re-searching the tree.

### Multi-thread research: set ops over stashed evidence
```
scrt "rate.?limit" --in src/  --mp-stash rl-impl --mp-tag rl --mp-ttl 24h
scrt "rate.?limit" --in docs/ --mp-stash rl-docs --mp-tag rl --mp-ttl 24h
scrt "Redis|Memcached" --mp-compose rl-impl rl-docs       # union
scrt "TODO"            --mp-intersect rl-impl rl-docs      # files in BOTH
```

### Filtering opaque tool output / web fetches (high-leverage)
Route anything large you only partly care about through scrt instead of
dumping it into context:
```
scrt "FAIL|error TS" --cmd "gh run view --log 12345" --effort scan --clip 80 --max-tokens 2000
scrt "Warning|Error" --cmd "kubectl describe pod my-pod" --effort scan --clip 100
scrt "auth|token"    --url "https://example.com/api/docs" --effort scan --max-tokens 1500
```
Hard caps protect you: `--url` 16 MB / 30 s, `--cmd` 64 MB / 60 s;
truncated payloads return with a marker, not a hang. Always set
`--max-tokens` when filtering opaque payloads — tool output can spike.

### Long-session memory management
TTL every stash at creation; prune between phases:
```
scrt ... --mp-stash <name> --mp-ttl 4h  --mp-tag scan      # scratch
scrt ... --mp-stash <name> --mp-ttl 24h --mp-tag finding   # keep a day
scrt --mp-prune-tag scan                                   # drop scratch, keep findings
scrt --mp-prune-expired                                    # session open: drop expired
```
Budget: ≤20 active stashes per palace; one palace per task via
`MPG_MIND_PALACE`.

## Similarity & link discovery (`scrt_similar`)

Once a palace has stashes, find the ones *about the same thing* — to
recall prior work or build an interconnected palace.

```
scrt --mp-similar auth-login                 # rank stashes similar to one
scrt --mp-similar auth-login --match full    # chunked: "shares a section"
scrt --mp-similar auth-login --match vector  # weighted-cosine lexical
scrt --term "login rate throttle"            # rank against a raw query, no stash
scrt --mp-similar auth-login --score 8 --top 5
```

Three axes:

| `--match` | Best for |
| :--- | :--- |
| `note` (default) | quick "same intent" (the stash note only) |
| `full` | "shares a section" / near-dup (chunked best-pair + Jaccard) |
| `vector` | smoother weighted-lexical match (random-projection cosine) |

`--score 1–10` widens (1) or tightens (10) the ranking spread; it does
**not** change the displayed relevance.

**Link-as-you-stash:** every `--mp-stash` already suggests related
stashes + ready `--mp-link` commands. Act on them to build the graph as
you go (`--no-suggest-links` to silence, `--link-threshold <0-100>` to
tune).

> **The honest limit — internalize this.** All similarity here is
> **lexical/structural**: it matches shared words and shape, NOT meaning.
> "dog Rex" and "my pet's name" share no surface form and will not match.
> When you need meaning-based recall, that's the embedding tier
> (scrt-evolve), not this engine. Use scrt similarity for "find the stash
> with these words/this structure," not "find the stash about this idea."

## Orchestrating with `scrt --serve`

When a task makes many calls, run ONE warm process and skip per-call
startup. Spawn `scrt --serve` (stdio NDJSON) once, send one request per
line, read one response per line:

```json
{ "id": "1", "method": "search", "params": { "pattern": "TODO", "in": ["src/"], "max_tokens": 2000 } }
{ "id": "2", "method": "palace.similar", "params": { "name": "auth-login", "match": "full" } }
```

Methods mirror the CLI (snake_case params): `search`,
`palace.{list,get,stash,drop,compose,intersect,except,link,graph,similar,
prune_*}`, `tool_spec`, `health`. For a shared daemon use
`--serve-http --port N` (`POST /`, `GET /health`). Node/Python harnesses
call the same dispatcher in-process via `scrt-napi` / `scrt-py` — no
subprocess at all.

## Behaviors you can rely on

- `--json` aliases `--format json`; `--format agent-json` adds the
  structured control-loop envelope (`status`, `n_literal_matches`,
  `warning`, `errors`).
- **`--mp-get` defaults to a card view** (note + tags + relations +
  sources, no node bodies — much cheaper). Pass `--with-nodes` / `--full`
  (CLI) or `with_nodes: true` (SDK) only when you need the bodies.
- Fingerprints for similarity live in a `.mpg/fingerprints.json` sidecar,
  computed at stash time and recomputable — never edit it by hand.
- Directory scans go through the `ignore` walker; don't pre-expand to file
  lists in your calls. `--sort recent|oldest` makes dir order
  deterministic.

## Failure channels — when to dig

A quiet `no_matches` is reliable; a quiet `partial` is not. Check:
- `status: "partial"` in `agent-json` — some sources errored.
- `result.errors[]` — populated whenever any source failed.
- the corrupt-palace stderr warning + `*.corrupt.*` backup — the palace
  was unreadable; saves refuse to clobber it.

Tuning env vars: `MPG_MIND_PALACE` (palace path / isolation per task),
`MPG_PATTERN` (pattern via env), `MPG_FORCE_RESET` (overwrite a tainted
palace — only after inspecting the backup).
