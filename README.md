# scrt

[![CI](https://github.com/JadeZaher/scrt-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/JadeZaher/scrt-cli/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](./LICENSE)

**scrt** — *Speedy Context Retrieval Tool* — is a long-running
**processing engine for grounded generative context**: token-budgeted
**context-node** retrieval, an instantiable **mind palace** of saved
results, and **lexical similarity / link discovery** over that palace —
usable as a CLI, a warm stdio/HTTP service, or embedded in-process in
Node and Python harnesses.

The problem it solves: feeding an LLM raw search hits, command output, or
fetched pages burns the context window on text you don't need. scrt sizes
every result in **tokens, not lines**, attributes it to `file:line`, and
lets you stash the useful parts into a persistent mind palace that
survives compaction and session boundaries — so long-horizon work recalls
prior findings instead of re-searching for them.

> **Status: implemented.** Search, mind palace, transports (stdio / HTTP
> / NAPI / PyO3), CLI, similarity + link-suggestions are built and tested.
> The **semantic** tier — a trained embedding model — lives in a separate
> project, [scrt-evolve](../scrt-evolve), which consumes this engine.

> **Coming from Node `mpg`?** scrt is a Rust engine that reads the same
> palace file format and honours the `MPG_*` env vars, so migration is
> drop-in. See [MIGRATION.md](./MIGRATION.md). Byte-for-byte mpg parity is
> no longer a design goal — scrt is its own product with its own surface.

## Why Rust

- **In-process search** via the BurntSushi `grep` crates — no subprocess
  `rg`, no Node cold-start. ~5–7× faster warm and cold on typical corpora.
- **In-memory palaces** alongside on-disk ones — ephemeral per-task
  memory without filesystem churn.
- **Multi-tenant** — one engine per machine, many scoped palaces.
- **FFI** (NAPI + PyO3) — embed the engine directly in JS/Python
  harnesses, zero subprocess.

## Usage

### Search

```bash
scrt "TODO" --in src/                 # token-budgeted search → context nodes
scrt "useAuth" --in . --effort scan   # cheap inventory pass (many small hits)
scrt "rate.?limit" --in src/ --effort deep   # detailed drill, big windows
scrt "ERROR" --cmd "journalctl -u app --since '1h ago'"  # search command output
scrt "auth" --url https://example.com/docs   # search a fetched URL
```

Effort presets size the per-node window + node cap (`scan` / `normal` /
`deep`); `--max-tokens`, `--window-curve`, and `--sort` shape the budget.

### Mind palace

```bash
scrt "auth" --in src/ --mp-stash auth-login "auth + rate limiting"  # save a result
scrt --mp-list                         # what's captured
scrt --mp-list --mp-list-search auth   # filter the listing by name/note/pattern/tag
scrt --mp-list --mp-list-tag security  # filter the listing by tag
scrt --mp-get auth-login --full        # recall (card view by default)
scrt "x" --in . --mp-from auth-login   # re-scope a search to a stash's files (~3× cheaper)
scrt --mp-link auth-login rate-limiter see-also   # connect two stashes
scrt --mp-prune-expired                # housekeeping
```

`--mp-list-search <text>` (alias `--mp-find`) keeps only stashes whose
**name, note, search pattern, or any tag** contains the text
(case-insensitive substring); it composes with `--mp-list-tag`. Use it to
find a stash by intent when the palace has grown past a glance. For
*ranked* fuzzy discovery rather than a substring filter, see
[Similarity & link discovery](#similarity--link-discovery) below.

Stashes survive compaction and session boundaries; the on-disk format
round-trips with Node mpg.

### Similarity & link discovery

Find stashes that are *about the same thing* — to discover prior work, or
to build an interconnected palace as you go.

```bash
scrt --mp-similar auth-login                 # rank stashes similar to one
scrt --mp-similar auth-login --match full    # chunked: best for "shares a section"
scrt --mp-similar auth-login --match vector  # weighted-cosine (random-projection)
scrt --term "login rate throttle"            # rank against a raw query, no stash
scrt --mp-similar auth-login --score 8 --top 5   # tighter, top 5 only
```

**Link-as-you-stash:** every `--mp-stash` surfaces related existing
stashes plus a ready `--mp-link` command:

```
scrt: created stash "auth-test" …
scrt: ~ related stashes (link suggestions):
   78%  auth-login  [chunked]   scrt --mp-link auth-test auth-login see-also
```

Silence with `--no-suggest-links`; tune the bar with `--link-threshold
<0-100>`.

#### The three signals (and their honest limit)

| `--match` | Method | Best for |
| :--- | :--- | :--- |
| `note` (default) | whole-note SimHash (Hamming) | quick "same intent" |
| `full` | chunked best-pair + MinHash-Jaccard | "shares a section" / near-dup |
| `vector` | random-projection cosine | smoother weighted-lexical match |

All three are **lexical / structural** — they match shared words and
shape, **not meaning**. "dog Rex" and "my pet's name" share no surface
form and will not match. Crossing that semantic gap needs a trained
embedding model — that's [scrt-evolve](../scrt-evolve)'s job, not this
engine's. These three are cheap, deterministic, and model-free.

`--score 1–10` reshapes the ranking spread only (1 = wide net, 10 =
near-identical); it does not distort the displayed relevance.

### As an engine (transports + FFI)

```bash
scrt --serve                      # stdio NDJSON dispatcher (warm, long-running)
scrt --serve-http --port 17317    # HTTP: POST / , GET /health
scrt tool-spec --format anthropic # function-calling tool descriptors
```

Node (NAPI) and Python (PyO3) bindings call the same dispatcher
in-process — see `crates/scrt-napi` and `crates/scrt-py`.

### Agent / MCP tools

`scrt tool-spec` emits six function-calling tools (OpenAI / Anthropic /
Gemini shapes): `scrt_search`, `scrt_stash`, `scrt_list_stashes`,
`scrt_get_stash`, `scrt_drop_stash`, and `scrt_similar`.
`scrt_list_stashes` takes `tag_filter` and a free-text `search` to narrow
the listing the same way the `--mp-list-search` flag does. Tool names are
stable, so agents migrating from Node mpg keep working unchanged.

## Documents

| Doc | What it covers |
| :--- | :--- |
| [INSTALL.md](./INSTALL.md) | Install + agent integration (Claude/Gemini plugins, Pi extension, Node/Python, `--serve`). |
| [RELEASING.md](./RELEASING.md) | How releases are cut (tag → CI gate → cross-platform binaries). |
| [DESIGN.md](./DESIGN.md) | Thesis, feature surface, non-goals, dependency choices. |
| [COMPAT.md](./COMPAT.md) | The mpg JSON schemas scrt round-trips — the migration boundary. |
| [MIGRATION.md](./MIGRATION.md) | Coming from Node `mpg`: what's identical, what differs, perf. |

The semantic / self-evolution work (embedding adapters, training) lives in
the separate [scrt-evolve](../scrt-evolve) project, which consumes this
engine.

## Workspace layout

```
crates/
  scrt-core     engine: search + node construction + mind palace + similarity (lib)
  scrt-cli      the `scrt` binary
  scrt-server   stdio NDJSON + HTTP transports over one dispatcher
  scrt-napi     Node addon (napi-rs) — in-process dispatch
  scrt-py       Python bindings (PyO3, abi3 wheel via maturin)
```

`scrt-napi` and `scrt-py` are excluded from `default-members` (they need
the Node / Python toolchains); build with `cargo build -p <crate>`.

> A transitional `crates/scrt-evolve` (corpus export + an InfoNCE training
> seam) still lives here; it is the seed for the standalone
> [scrt-evolve](../scrt-evolve) project and will be lifted out of this
> workspace as that repo matures.

## License

MIT.
