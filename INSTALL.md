# scrt — Installation & Agent Integration Guide

scrt works with **any** coding agent. It's a single Rust binary (no
runtime, no `rg` dependency), so the install is trivial; the integration
path depends on how your agent executes tools.

| Agent | Integration path | Difficulty |
| :--- | :--- | :--- |
| **Claude Code** | Plugin (bundles tool-spec + skill) | Easy |
| **Claude API** | `scrt tool-spec --format anthropic` | Easy |
| **Gemini CLI / API** | Extension (tool-spec + commands) | Easy |
| **Pi Agent** | Custom extension (`--serve` stdio + skill) | Easy |
| **Cline / Windsurf / Continue** | Shell command, or `--serve-http` daemon | Easy |
| **Cursor** | Shell command in Composer + `.cursorrules` | Medium |
| **Aider** | `/run` shell command | Easy |
| **Node harness** | `scrt-napi` in-process binding (no subprocess) | Easy |
| **Python harness** | `scrt-py` PyO3 wheel (no subprocess) | Easy |
| **Any harness making many calls** | `scrt --serve` (warm stdio / HTTP process) | Easy |
| **Any agent that can shell out** | `cargo install` + plain `scrt …` | Trivial |

---

## Universal install (prerequisite for all paths)

scrt is a self-contained binary — **no Node, no Python, no ripgrep** at
runtime.

```bash
# From source (until a published crate / prebuilt binary lands):
git clone https://github.com/JadeZaher/scrt-cli.git
cd scrt-cli
cargo build --release            # produces target/release/scrt
# put it on PATH, e.g.:
cp target/release/scrt ~/.local/bin/      # or a dir already on PATH

# Verify:
scrt --version
scrt --help
scrt --ls                        # lists searchable files in the cwd
```

> Requires a Rust toolchain (1.79+). Once scrt is published, `cargo
> install scrt-cli` and prebuilt release binaries will be the trivial path.

A quick smoke test any agent can run after install:

```bash
echo "// TODO: test" > /tmp/scrt-verify.ts
scrt "TODO" --in /tmp/scrt-verify.ts --format json | grep '"status"'
# -> "status": "ok"
rm /tmp/scrt-verify.ts
```

---

## The six tools

Every integration ultimately exposes the same six function-calling tools.
Generate the provider-shaped schema at install time so it stays in sync
with the installed version:

```bash
scrt tool-spec --format anthropic > tools/scrt-anthropic.json
scrt tool-spec --format openai    > tools/scrt-openai.json
scrt tool-spec --format gemini    > tools/scrt-gemini.json
```

| Tool | What it does |
| :--- | :--- |
| `scrt_search` | Token-budgeted search → context nodes with `file:line` attribution |
| `scrt_stash` | Save a result to the mind palace (suggests links to related stashes) |
| `scrt_list_stashes` | List stashes, filterable by tag |
| `scrt_get_stash` | Recall one stash (card view by default; `with_nodes` for bodies) |
| `scrt_drop_stash` | Remove a stash |
| `scrt_similar` | Rank stashes by similarity to a stash or term — discover prior work + links |

The first five are inherited from `mpg` (same names, so migrating harnesses
keep working); `scrt_similar` is scrt's extension for similarity / link
discovery.

---

## Claude Code (plugin)

The recommended path. Package scrt as a Claude Code plugin so a session
gets the tools **and** the usage guidance together.

A plugin is a directory with a manifest, the generated tool-spec, and a
skill that teaches the workflows. Minimal shape:

```
scrt-plugin/
  plugin.json                 # manifest (name, version, entry)
  tools/scrt-anthropic.json   # from `scrt tool-spec --format anthropic`
  skills/scrt/SKILL.md         # the recurring-jobs guidance (below)
  commands/                    # optional slash commands wrapping common calls
```

**Generate the tool-spec into the plugin:**

```bash
scrt tool-spec --format anthropic > scrt-plugin/tools/scrt-anthropic.json
```

**Wire the tools to execution.** scrt has no bundled MCP server; the
plugin runs the binary. For many calls per session, point the plugin at a
warm process (`scrt --serve`, below) instead of re-spawning the CLI.

**Skill content.** The skill is what makes scrt *used*, not just installed.
This repo ships one at [`skills/scrt-context/SKILL.md`](./skills/scrt-context/SKILL.md)
— the lens mental model, when-NOT-to-use cutoffs, recurring jobs,
similarity/link-discovery, and `--serve` orchestration. Copy it into the
plugin's `skills/scrt/SKILL.md` (or load it into the session's custom
instructions).

> If you just want the raw tools without a plugin, register the generated
> `tools/scrt-anthropic.json` directly in your harness (next section).

### Avoid permission-prompt spam (permissions + PreToolUse hook)

An agent that runs `scrt` over the shell will, by default, hit a
Bash/PowerShell permission prompt on **every** retrieval call. This repo
ships a [`.claude/settings.json`](./.claude/settings.json) that fixes it two
ways — copy it into your project (or merge into `~/.claude/settings.json`):

**1. An allow-list** for the common commands, so they never prompt:

```jsonc
{
  "permissions": {
    "allow": [
      "Bash(scrt:*)",                 // any scrt invocation
      "Bash(cargo build:*)", "Bash(cargo test:*)", "Bash(cargo clippy:*)"
    ],
    "deny": [
      "Bash(scrt --mp-drop:*)",       // destructive ops still prompt
      "Bash(scrt --mp-prune-all:*)",
      "Bash(scrt evolve train:*)"
    ]
  }
}
```

`deny` takes precedence, so destructive palace ops still require explicit
approval even though `scrt:*` is allowed.

**2. A `PreToolUse` hook**
([`.claude/hooks/allow-scrt.mjs`](./.claude/hooks/allow-scrt.mjs)) for the
long tail of flag combinations. It inspects each Bash/PowerShell call and
**auto-approves read-only / non-destructive scrt commands**, while staying
silent (deferring to your normal allow/deny) for everything else —
including destructive scrt ops and any non-scrt command. It never *denies*;
it only fast-paths the safe cases.

```jsonc
{
  "hooks": {
    "PreToolUse": [
      { "matcher": "Bash",       "hooks": [ { "type": "command",
        "command": "node \"$CLAUDE_PROJECT_DIR/.claude/hooks/allow-scrt.mjs\"" } ] },
      { "matcher": "PowerShell", "hooks": [ { "type": "command",
        "command": "node \"$CLAUDE_PROJECT_DIR/.claude/hooks/allow-scrt.mjs\"" } ] }
    ]
  }
}
```

The hook is the robust path (covers arbitrary flag combos); the allow-list
is the simple path. Use either or both — they compose. The hook needs
`node` on PATH (used only to evaluate the decision, not to run scrt).

---

## Claude API (tool_use)

Generate the schema, register it, dispatch tool calls to the binary or a
warm `--serve` process:

```bash
scrt tool-spec --format anthropic > tools/scrt-anthropic.json
```

```ts
const tools = JSON.parse(fs.readFileSync("tools/scrt-anthropic.json", "utf8"));

const response = await anthropic.messages.create({
  model: "claude-sonnet-4-6",
  tools,
  // ...
});

// On a tool_use block, run the matching scrt operation (CLI or --serve)
// and return the JSON as a tool_result. e.g. scrt_search -> `scrt <pattern>
// --in ... --format agent-json`, scrt_similar -> `scrt --mp-similar ...`.
```

For a Node harness, prefer the in-process `scrt-napi` binding over shelling
out (see "Node / Python" below) — same dispatcher, zero subprocess.

### Claude Code PreCompact hook (snapshot before compaction)

Crystallize working memory into stashes right before the context is
compacted, so it survives:

```jsonc
{
  "hooks": {
    "PreCompact": [
      { "hooks": [ {
        "type": "command",
        "command": "scrt --mp-prune-expired && scrt --mp-list --json > .mpg/precompact-snapshot.json"
      } ] }
    ]
  }
}
```

This prunes expired scratch stashes, then snapshots what survived. Pair it
with TTL discipline at stash time:

| Stash kind | TTL | Survives compaction? |
| :--- | :--- | :--- |
| Exploratory scan | `--mp-ttl 4h` | No — pruned before snapshot |
| Confirmed finding | `--mp-ttl 24h` | Yes |
| Canonical context | _(no TTL)_ | Yes, always |

After compaction, the first move is `scrt --mp-list` / `scrt --mp-get
<name>` — stash nodes carry `file:line` + budget-bounded text, so the
compacted session gets its evidence back without re-scanning.

---

## Gemini CLI / API (extension)

Package scrt as a Gemini extension — the tool-spec plus optional commands:

```bash
scrt tool-spec --format gemini > scrt-gemini-extension/tools/scrt-gemini.json
```

```ts
const { functionDeclarations } = JSON.parse(
  fs.readFileSync("tools/scrt-gemini.json", "utf8")
);

const model = genAI.getGenerativeModel({
  model: "gemini-2.5-pro",
  tools: [{ functionDeclarations }],
});
```

On a `functionCall`, run the matching scrt operation and return the result.
The extension's command set can wrap the common calls (search, stash,
similar) so the model reaches for them by name.

---

## Pi Agent (custom extension)

Pi gets a first-class custom extension that drives scrt over `--serve`
stdio (one spawn per session, no per-call startup):

```
scrt-extension/
  extension.json              # pi extension manifest
  skills/scrt/SKILL.md         # recurring-jobs guidance
  lib/scrt-client.ts           # spawns `scrt --serve`, sends NDJSON
```

The extension spawns scrt once as a warm process and sends NDJSON
requests (see "Warm-process mode"). It composes with other Pi skills the
same way mpg did — a Grounding-phase retrieval tool, complementary to
`scout` for structured retrieval, and useful for finding task-ID
references in conductor contexts.

> Windows note: spawn the **resolved binary path** directly
> (`scrt --print-entry` prints it), not a shell wrapper, to avoid argv
> mangling. For regexes with shell metacharacters, write the pattern to a
> file and pass `--pattern-file <path>` so it never crosses argv.

---

## Node / Python (already available — in-process, no subprocess)

The engine embeds directly, with **no subprocess and no serialization
round-trip**. Both bindings call the same `dispatch(method, params_json)
-> json_string` surface as the server.

### Node (`scrt-napi`)

```bash
cargo build --release -p scrt-napi   # produces the .node addon
```

```ts
import { dispatch } from "./scrt-napi";   // the built addon

const res = JSON.parse(
  dispatch("search", JSON.stringify({ pattern: "TODO", in: ["src/"], max_tokens: 2000 }))
);
const similar = JSON.parse(
  dispatch("palace.similar", JSON.stringify({ name: "auth-login", match: "full" }))
);
```

### Python (`scrt-py`)

```bash
cd crates/scrt-py
PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 python -m maturin develop --release
```

```python
import scrt_py, json

res = json.loads(scrt_py.dispatch("search",
    json.dumps({"pattern": "TODO", "in": ["src/"], "max_tokens": 2000})))
similar = json.loads(scrt_py.dispatch("palace.similar",
    json.dumps({"name": "auth-login", "match": "vector"})))
```

Passing JSON **strings** across the FFI boundary is measurably cheaper than
marshalling deep objects — that's why the binding takes/returns strings.

---

## Warm-process mode (`scrt --serve`)

scrt's cold start is already fast (~16 ms, no Node, no `rg` spawn), but a
harness making many calls per task should still run one warm process and
skip even that. Two transports.

### Stdio (best when the agent spawns scrt as its own child)

```bash
scrt --serve
```

Newline-delimited JSON in, newline-delimited JSON out — one request per
line:

```json
{ "id": "req-1", "method": "search", "params": { "pattern": "TODO", "in": ["src/"], "max_tokens": 2000 } }
```

```json
{ "id": "req-1", "result": { "status": "ok", "nodes": [ ... ] } }
```

### HTTP (best for a shared local daemon)

```bash
scrt --serve --serve-http --port 17317
```

- `POST /` with `{ "method": "<name>", "params": { ... } }`.
- `GET /health` → `{ "ok": true, "version": "<semver>", "palace_path": "<abs>" }`.

```bash
curl http://127.0.0.1:17317/health
curl -s http://127.0.0.1:17317/ -H "Content-Type: application/json" \
  -d '{"method":"search","params":{"pattern":"TODO","in":["src/"],"max_tokens":2000}}'
curl -s http://127.0.0.1:17317/ -d '{"method":"palace.similar","params":{"name":"auth-login","match":"full"}}'
```

### Methods (both transports)

| Method | Description |
| :--- | :--- |
| `search` | Token-budgeted search (mirrors the CLI search flags) |
| `palace.list` / `palace.get` / `palace.stash` / `palace.drop` | Core stash ops |
| `palace.compose` / `palace.intersect` / `palace.except` | Set ops over stash file-sets |
| `palace.link` / `palace.graph` | Relations + graph traversal |
| `palace.similar` | Rank stashes by similarity (`name` or `term`, `match`, `score`, `top`) |
| `palace.prune_expired` / `prune_tag` / `prune_older_than` / `prune_keep` | Housekeeping |
| `tool_spec` | Provider tool schema (for harness bootstrap) |
| `health` | `{ ok, version, palace_path }` |

Param names are **snake_case**, mirroring the CLI flags.

**Stdio vs HTTP:** stdio when the agent owns the lifecycle (spawns scrt,
kills it with the session) — zero config, works in sandboxes; HTTP when
multiple processes share one daemon or you want it to outlive a session.

---

## Generic / any agent (shell-out)

If your agent can run a shell command, just use the CLI:

```bash
scrt "auth|login" --in . --effort quick --format json          # quick recon
scrt "session" --in src/ --effort deep --format json           # deep dive
scrt "TODO" --in src/ --mp-stash my-todos "TODO findings" --mp-tag review
scrt --mp-similar my-todos --match full                        # find related stashes
scrt "TODO" --mp-compose stash-a stash-b --format json          # set-op over stashes
```

All output formats (`llm` / `markdown` / `json` / `text` / `agent-json`)
are designed to be read directly by an LLM. `--format agent-json` adds the
structured control-loop envelope (`status`, `n_literal_matches`, `warning`,
`errors`).

For **Cursor**, put a shell-out rule in `.cursorrules`; for **Aider**, use
`/run scrt …`; for **Cline / Windsurf / Continue**, either shell out or
point them at a `--serve-http` daemon.

---

## For AI agents setting scrt up for themselves

If you're an agent (Claude Code, Cursor, Cline, Pi, …) installing scrt on a
user's machine, the install is only half the job. The other half is
teaching **future sessions** how to use it — otherwise the next session
reaches for raw grep + read every time and loses the token-budget and
cross-session-memory wins entirely.

**Recommended setup pass:**

1. Install + smoke-test (above).
2. **Wire the permissions + hook** ([`.claude/settings.json`](./.claude/settings.json)
   + [`.claude/hooks/allow-scrt.mjs`](./.claude/hooks/allow-scrt.mjs)) so the
   user isn't prompted for every scrt call (see "Avoid permission-prompt
   spam" above). This is the single biggest ergonomics win.
3. Read [`skills/scrt-context/SKILL.md`](./skills/scrt-context/SKILL.md) end
   to end — it's the orchestration reference. Copy it into the user's global
   agent config (`~/.claude/CLAUDE.md` for Claude Code, `.cursorrules`,
   `~/.aider.conf.yml`, etc.), or surface its key sections there:
   - **Compaction at near-zero cost** — `--effort scan --clip 30
     --max-tokens N` instead of a summarization round-trip.
   - **Scan → stash → drill** — `--effort scan` to inventory, `--mp-from`
     to re-scope a deeper search to the flagged files (~3× cheaper than
     re-searching the tree).
   - **Similarity & link discovery** — `scrt --mp-similar <stash>` to find
     related prior work; every `--mp-stash` suggests links to build an
     interconnected palace. Three axes: `--match note|full|vector`.
   - **Set-op research** — `compose` / `intersect` / `except` over stashed
     evidence packets.
   - **Long-session memory** — TTL every stash (`--mp-ttl 4h` scratch,
     `24h` findings), prune by tag between phases, one palace per task via
     `MPG_MIND_PALACE`.
   - **Filtering opaque output** — route long CI logs, `gh … --json`,
     `kubectl describe`, and web fetches through `scrt --cmd "…"` /
     `scrt --url "…"` with a `--max-tokens` cap rather than dumping them
     into context. Hard caps protect you (`--url` 16 MB / 30 s,
     `--cmd` 64 MB / 60 s; truncated payloads return with a marker).
4. Surface the **failure channels** so the next session knows when to dig:
   `status: "partial"`, `result.errors[]`, the corrupt-palace stderr
   warning — a quiet `no_matches` is reliable, a quiet `partial` is not.
5. Note the load-bearing behaviors: `--json` aliases `--format json`;
   `--mp-get` defaults to a **card view** (note + tags + relations +
   sources, no node bodies — much cheaper), pass `--with-nodes` / `--full`
   (or `with_nodes: true` over the SDK) only when you need the bodies; the
   similarity signal is **lexical, not semantic** (it matches shared words,
   not meaning — "dog Rex" won't match "my pet's name").

This is the difference between scrt being *installed* and scrt being
**used**.

---

## Honest scope notes

- **No bundled MCP server (yet).** mpg shipped an `mcp-server.js`; scrt's
  integration path is `tool-spec` + `--serve` + the plugin/extension
  wrappers above. A native MCP transport can sit on the dispatcher later;
  it isn't here today.
- **The skill + permission/hook config ship today.**
  [`skills/scrt-context/SKILL.md`](./skills/scrt-context/SKILL.md) and the
  [`.claude/`](./.claude/) settings + PreToolUse hook are real and tested.
  The Claude Code *plugin*, Gemini *extension*, and Pi *extension* packaging
  shapes above are the intended wrappers around them; the binary,
  `tool-spec`, `--serve`, skill, and permission config they bundle all work
  now.
- **The semantic tier is a separate project.** scrt's similarity is cheap
  and lexical. Meaning-based retrieval (trained embeddings) lives in
  [scrt-evolve](../scrt-evolve), which consumes this engine.
