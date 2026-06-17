# scrt — Design

scrt is a long-running **processing engine for grounded generative
context**: in-process token-budgeted retrieval, an instantiable mind
palace, and lexical similarity / link discovery over it, embeddable in
Node and Python harnesses.

It began as a Rust port of the Node `mpg` CLI ([`mind-palace-graph`](../mind-palace-graph))
and keeps mpg's palace file format and `MPG_*` env vars for **user
migration**. It is no longer bound to byte-for-byte mpg parity: scrt is its
own product, ships its own Node/Python packages over the Rust core, and
extends the surface (similarity retrieval, the `scrt_similar` tool, the
`--mp-similar` family). The semantic tier — a trained embedding model —
is a separate project, [scrt-evolve](../scrt-evolve).

This document states the thesis, the feature surface, the non-goals, and
the dependency choices. The mpg migration boundary (the JSON schemas scrt
round-trips) is in [COMPAT.md](./COMPAT.md); coming-from-mpg differences in
[MIGRATION.md](./MIGRATION.md).

## 1. Thesis

**Any directory is a database.** scrt is the layer between an agent (the
processor) and a directory of unstructured agent-relevant content —
markdown, JSON, code, logs, notes (the store). It hands the agent
token-budgeted slices of that store on demand.

This is the OneLake pattern — one substrate, many engines reading it —
scoped down to the unstructured, agent-relevant content a local agent
accumulates, rather than warehouse-scale structured data.

The bet, validated by the Node line: **lazy interpretation.** No
write-time LLM compression, no fact extraction at ingest, no embedding
pass on stash. The repo *is* the memory; meaning is computed at read time,
from raw bytes, by the agent consuming the nodes. Memory-stacks that beat
the Node line on LongMemEval (Mem0, Zep, Letta, Mastra) all spend
write-time LLM compute; scrt keeps deferring, and wins on cost, fidelity,
and the fact that the memory is just files you can read, diff, and grep
without the engine running.

**What the Rust engine adds** beyond a 1:1 rewrite:

- **Long-running engine semantics** — a warm process that holds state and
  answers many requests, not a CLI paying cold-start per call.
- **In-memory palace instances** — palaces that never touch disk, for
  ephemeral per-task memory.
- **Multi-tenant isolation** — one process, N named palaces routed by ID,
  file-backed or memory-backed side by side.
- **FFI bindings** — the engine embeds directly in Node (NAPI) and Python
  (PyO3), no subprocess.
- **Lexical similarity / link discovery** — SimHash-family ranking over
  stashes (`--mp-similar`), with link-suggestions at stash time. Cheap,
  deterministic, model-free; the semantic complement is scrt-evolve.

## 2. Feature surface

### 2.1 Search
- Sources: **file / glob / directory (recursive, `.gitignore`-respected) /
  command stdout / URL fetch / stdin**, plus `@file` / `@-` path-list
  indirection and comma-lists.
- In-process regex search with ripgrep-equivalent semantics (the BurntSushi
  `grep` crates), so patterns paste over from mpg unchanged.
- **Node construction**: match line + sized before/after context,
  token-budget trimmed. The `nodes[]` JSON shape is the `Node` type
  ([COMPAT.md §1](./COMPAT.md)).
- Token estimation: `max(1, ceil(len/4))`, matching mpg so node counts and
  trim boundaries agree.

### 2.2 Budgeting / presets / envelope
- `--effort scan|quick|normal|deep` (`quick` default; `auto` aliases
  `normal`) — before/after/max-nodes bundles per [COMPAT.md §2.1](./COMPAT.md).
- `--strategy fill|deep`, `--clip <N>`, `--sort recent|oldest|default`,
  `--window-curve flat|linear|log`.
- `--fuzzy`: trigram-union driver + Levenshtein ≤2 post-filter (skipped on
  regex-metacharacter patterns).
- Output formats: `llm` (default), `markdown`, `json`, `text`,
  `agent-json` (the structured agent control-loop envelope).

### 2.3 Mind palace
- On-disk JSON (`Palace` / `Stash` / `StashedNode`, `version: 2`),
  byte-compatible with mpg so a file opens in both during migration.
- Two backends behind one `Palace` trait: **`FilePalace`** (disk, atomic
  writes, `MPG_MIND_PALACE` env / `--mp-path`) and **`MemoryPalace`**
  (in-process).
- Ops: stash, drop, list, get (card + `--with-nodes`), compose, intersect,
  except, from, link, unlink, related, graph, prune_* (older-than / keep /
  tag / expired / all), tags, TTL, content-hash + mtime staleness.
- **Multi-tenancy**: a `Registry` holds N named palaces, routing ops by ID.

### 2.4 Similarity & link discovery
- `scrt --mp-similar <stash>` / `--term <text>` ranks stashes by lexical
  similarity, with three axes (`--match note|full|vector`):

  | axis | method | best for |
  | :--- | :--- | :--- |
  | `note` | whole-note SimHash (Hamming) | quick "same intent" |
  | `full` | chunked best-pair + MinHash-Jaccard | "shares a section" / near-dup |
  | `vector` | random-projection cosine | smoother weighted-lexical match |

- `--score 1–10` reshapes the ranking spread (not the displayed relevance);
  `--top N` truncates.
- **Link-as-you-stash**: `--mp-stash` surfaces related stashes + ready
  `--mp-link` commands (`--no-suggest-links` / `--link-threshold` to tune).
- Fingerprints live in a scrt-only `.mpg/fingerprints.json` sidecar, so the
  palace JSON stays mpg-compatible.
- **Honest limit:** all three signals are lexical/structural — they match
  surface form, not meaning. The semantic bridge is scrt-evolve.

### 2.5 Engine transports + tool-spec
- **stdio NDJSON** (`scrt --serve`), **HTTP/axum** (`--serve-http`: `POST /`,
  `GET /health`), **NAPI** + **PyO3** bindings — all over one
  transport-agnostic `dispatch(method, params)`.
- Methods: `search`, `palace.{list,get,stash,drop,compose,intersect,except,
  link,graph,similar,prune_*}`, `tool_spec`, `health`.
- `scrt tool-spec --format openai|anthropic|gemini` emits six
  function-calling tools (`scrt_search`, `scrt_stash`, `scrt_list_stashes`,
  `scrt_get_stash`, `scrt_drop_stash`, `scrt_similar`).

### 2.6 CLI
- Every mpg flag, same names/defaults. **Exit codes**: 0 match / 1 no-match
  / 2 bad-args / 4 palace-error / 99 unexpected. (mpg's code 3 "ripgrep not
  installed" is dropped — scrt owns the regex engine.)

## 3. Non-goals

- **No write-time interpretation.** No embeddings, fact extraction, or
  entity-temporal graph *in this engine*. Lazy interpretation is the thesis;
  the semantic tier is the separate scrt-evolve project, by design.
- **No surgical stash editing.** Stashes stay append/replace + merge-on-
  dedup (Letta-style in-place core-memory editing is out).
- **No real tokenizer.** The `chars/4` heuristic is kept deliberately —
  changing it shifts node counts and trim boundaries (and breaks mpg
  migration parity). A real tokenizer is a pluggable boundary, not a default.
- **No bundled MCP-server rewrite.** The engine transports (stdio / HTTP /
  NAPI / PyO3) plus `tool-spec` cover harness integration; a native MCP
  transport can sit on the dispatcher later.

## 4. Dependency choices

Each justified against the alternative; versions pinned in the workspace
`[workspace.dependencies]`.

### 4.1 Search: `grep-searcher` + `grep-regex` + `grep-matcher` (not subprocess `rg`)
**The entire reason for the port.** mpg shells out to `rg --json`, paying a
process spawn per call and making a warm in-process engine impossible. The
BurntSushi `grep` family is the library *behind* ripgrep — same searcher,
regex matcher, line/context machinery — callable in-process.
- *Keep subprocess `rg`:* rejected — defeats the warm-engine + NAPI goals,
  reintroduces the "rg not installed" failure mode.
- *`regex` crate alone:* rejected — we'd reimplement line iteration, context
  windows, binary detection, multiline that `grep-searcher` already does.
- The acceptance bar is **semantics match**: patterns that worked against
  `rg --json` produce the same matches (`grep-regex` wraps the same `regex`
  engine rg uses).

### 4.2 Directory walking: `ignore`
The crate ripgrep itself uses for `.gitignore`-respecting parallel walks, so
`--no-ignore` / `--hidden` / `--type` behave like rg without
reimplementation. (*`walkdir` + hand-rolled gitignore:* rejected — a bug
farm that diverges from rg precedence.)

### 4.3 Globs: `globset`
Same BurntSushi family, the glob matcher rg uses for `--include` /
`--exclude` / `'**/*.ts'`. Consistent with the walker.

### 4.4 Serialization: `serde` + `serde_json` (`preserve_order`)
The migration boundary *is* JSON. `serde` emits struct fields in declaration
order; we order fields to match mpg's object literals where a byte diff
matters. **`preserve_order` is required** — without it `json!`/`Value` maps
alphabetize keys and break tool-spec key order. `to_string_pretty` mirrors
`JSON.stringify(x, null, 2)`.

### 4.5 Async runtime: `tokio`, scoped to the server
`scrt-core` stays **sync** — search is CPU-bound and `grep-searcher` is a
sync API; async there buys nothing and complicates the FFI. `tokio` enters
only at `scrt-server` (concurrent stdio, the HTTP server, URL fetch). The
`url-source` feature gates the one async dependency `scrt-core` needs.

### 4.6 HTTP: `axum`
Tokio-native, minimal, obvious for a `POST /` + `GET /health` surface.
(*`hyper` directly:* more boilerplate; *`actix-web`:* heavier, own runtime.)

### 4.7 URL fetch: `reqwest` (rustls, no default features)
`rustls-tls` avoids a system OpenSSL build dependency (clean Windows
install). Default features off keeps the surface small. The mpg 16 MB / 30 s
caps are reimplemented on top.

### 4.8 Hashing: `sha1`
mpg anchors stash content-drift on a 12-hex-char SHA1 of the matched line.
To keep `match_line_hash` byte-identical (so a palace validates in both
impls), the algorithm + truncation must match. Similarity fingerprints use a
non-crypto hash (SipHash via `DefaultHasher`) — speed matters there,
collision-resistance doesn't.

### 4.9 Parallelism: `rayon`
Parallel multi-file search collection and palace-path canonicalization.

### 4.10 FFI: `napi-rs` (Node), `pyo3` (Python)
`napi-rs` is the standard for N-API-ABI-stable Node addons; `pyo3` (abi3-py39
for forward-compat) for Python extension modules. Both are out of
`default-members` so a plain `cargo build` needs neither toolchain. The
binding shape is `dispatch(method, params_json) -> json_string` — passing
JSON strings across the FFI boundary is measurably cheaper than marshalling
deep objects.

### 4.11 Errors: `anyhow` + `thiserror`
`thiserror` for the typed error enum in `scrt-core` (CLI maps variants →
exit codes); `anyhow` at the binary/server boundary.

## 5. Branding

scrt emits `scrt` where mpg emitted `mpg` in user-facing output: tool-spec
names (`scrt_search`, …), `next_suggestion` command strings, `<scrt …>`
result-block tags, `scrt:` stderr. The **palace file format carries no brand
string** and is untouched, so a palace round-trips between impls unchanged.
The `MPG_MIND_PALACE` / `MPG_PATTERN` env vars and `.mpg/` directory are
**kept** so existing configs and palace files resolve without migration. The
exhaustive brand-bearing field list is in [COMPAT.md §Branding](./COMPAT.md).

## 6. Workspace structure

`scrt-core` (lib) → `scrt-server` (transports) → `scrt-cli` (binary);
`scrt-napi` / `scrt-py` depend on core/server but are out of
`default-members`. The release profile uses thin LTO + `codegen-units = 1` +
`strip` for a small, fast binary. MSRV is 1.79 (uses `std::path::absolute`).

The semantic / self-evolution work (embedding adapters, training data
generation) is the separate [scrt-evolve](../scrt-evolve) project, which
depends on `scrt-core`. A transitional `crates/scrt-evolve` (corpus export +
an InfoNCE seam) seeds it and will be lifted out as that repo matures.
