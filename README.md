# scrt

**scrt** — *Speedy Context Retrieval Tool* — is a long-running
**processing engine for grounded generative context**: token-budgeted
**context-node** retrieval, an instantiable **mind palace** of saved
results, and **lexical similarity / link discovery** over that palace —
embeddable in-process in Node and Python harnesses.

It began as a Rust port of [`mind-palace-graph`](../mind-palace-graph)
(the Node `mpg` CLI) and keeps its palace file format and `MPG_*` env
vars for drop-in migration. **As of the 2026-06 direction shift, scrt is
its own product** — it ships its own Node and Python packages over the
Rust core and extends the surface beyond mpg (e.g. similarity retrieval).
Byte-for-byte mpg parity is no longer a design goal.

> **Status: implemented.** Search, mind palace, transports (stdio / HTTP
> / NAPI / PyO3), CLI, similarity + link-suggestions are built and tested.
> The **semantic** tier — a trained embedding model — lives in a separate
> project, [scrt-evolve](../scrt-evolve), which consumes this engine.

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
scrt --mp-get auth-login --full        # recall (card view by default)
scrt "x" --in . --mp-from auth-login   # re-scope a search to a stash's files (~3× cheaper)
scrt --mp-link auth-login rate-limiter see-also   # connect two stashes
scrt --mp-prune-expired                # housekeeping
```

Stashes survive compaction and session boundaries; the on-disk format is
byte-compatible with Node mpg.

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
`scrt_get_stash`, `scrt_drop_stash`, and **`scrt_similar`**. The five
mpg-inherited tools keep their names so migrating agents keep working;
`scrt_similar` is the scrt extension for ranking/link discovery.

## Documents

| Doc | What it covers |
| :--- | :--- |
| [INSTALL.md](./INSTALL.md) | Install + agent integration (Claude/Gemini plugins, Pi extension, Node/Python, `--serve`). |
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
