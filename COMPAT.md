# scrt — mpg Migration Boundary

This document enumerates the **mpg JSON schemas that scrt round-trips**,
with example payloads copied verbatim from the `mind-palace-graph` v0.3.2
source (`src/types.ts`, `src/format.ts`, `src/mind-palace.ts`,
`src/tool-spec.ts`, `src/pagination.ts`). It is the reference for what an
mpg user or harness can rely on when moving to scrt.

> **Scope note.** scrt no longer treats byte-for-byte mpg parity as an
> enforced design goal (scrt is its own product now). What remains a firm
> commitment — and what this doc specifies — is the **migration boundary**:
> the palace file format round-trips losslessly, and the search/envelope
> output stays compatible. scrt also *extends* the surface (e.g. the
> `scrt_similar` tool, the fingerprint sidecar) without breaking these
> shapes. Where scrt deliberately diverges, see [MIGRATION.md](./MIGRATION.md).

Two boundaries stay **lossless** so existing data and harnesses keep working:

1. **palace file format** — on-disk JSON; a file written by either impl
   opens in the other.
2. **agent-json envelope** — the agent control-loop primitive.

Plus the **`Node` / `Result` json shape** underneath them, and the
**tool-spec** descriptors (the 5 mpg tools keep their schemas; scrt adds
`scrt_similar`).

A note on field order: Node's `JSON.stringify` emits keys in
**insertion/declaration order**. `serde` emits struct fields in
**declaration order**. scrt orders each struct's fields to match the
v0.x object literal so `--format json` / `agent-json` produce a byte-
equal diff (modulo branding, §Branding, and timing fields, §Excluded).

---

## 1. `Node` (the retrieval unit)

Source: `src/types.ts`. Every `nodes[]` entry in search output, the
agent envelope, and `--format json` is this shape.

```jsonc
{
  "id": 1,                          // 1-indexed position in the result set
  "source": {                       // Source object (§1.1)
    "id": "src/auth/login.ts",
    "type": "file",
    "label": "src/auth/login.ts"    // optional; defaults to id
  },
  "match_line": 8,                  // 1-indexed line of the match
  "start_line": 5,                  // 1-indexed first context line (<= match_line)
  "end_line": 11,                   // 1-indexed last context line (>= match_line)
  "context_before": [               // pre-context, text only, no line numbers
    "// Authentication flow for the public API.",
    "// Validates credentials, then issues a short-lived session token.",
    "export async function login(user: User, password: string) {"
  ],
  "match_text": "  // TODO: add rate limiting per IP+user to prevent brute force",
  "context_after": [
    "  const valid = await db.users.verifyPassword(user.id, password);",
    "  if (!valid) {",
    "    logger.warn(`failed login for ${user.id}`);"
  ],
  "match_spans": [[5, 9]],          // [start,end) offsets within match_text
  "tokens": 196                     // chars/4 estimate of the whole node
}
```

### 1.1 `Source`

```jsonc
{
  "id": "src/auth/login.ts",   // stable id: file path, command, URL, or "stdin"
  "type": "file",              // "file" | "command" | "stdin" | "url" | "bulk"
  "label": "src/auth/login.ts" // optional display label
}
```

### 1.2 Token estimate — must match exactly

`tokens` uses the v0.x heuristic verbatim:

```
estimate(text) = text.length === 0 ? 0 : max(1, ceil(text.length / 4))
estimateMany(texts) = sum(estimate(t) for t in texts)
```

Node `tokens` = `estimateMany(context_before) + estimate(match_text) +
estimateMany(context_after)`. Because the trim-to-budget walk
(`trimLinesToBudget`) makes keep/drop decisions off this estimate,
**any divergence changes which lines are kept**, not just the count.
scrt reimplements `ceil(len/4)` with `max(1, …)` and the same balanced
outward-expansion walk (prefer the side with more remaining lines; on a
tie, expand the *low* side first).

---

## 2. `Result` + pagination (`--format json`)

Source: `src/types.ts`, `src/pagination.ts`. `--format json` is
`JSON.stringify(result, null, 2)` of this object.

```jsonc
{
  "pattern": "TODO",
  "effort": "normal",            // "scan"|"quick"|"normal"|"deep"|"auto"
  "strategy": "fill",            // "fill"|"deep"
  "total_nodes": 4,              // pre-pagination total
  "total_tokens": 566,           // pre-pagination total
  "sources_count": 3,
  "truncated": false,
  "nodes": [ /* Node[] — §1, post-pagination slice */ ],
  "duration_ms": 30,             // EXCLUDED from parity diff (§Excluded)
  "before_tokens": 500,          // resolved per-node window actually applied
  "after_tokens": 500,
  "max_nodes": 30,
  "max_tokens": 8000,            // optional; omitted when unset
  "auto_tune_applied": false,    // optional; present only when wide-record tune fired
  "status": "ok",                // "ok"|"no_matches"|"truncated"|"error"
  "page_tokens": 566,            // tokens of the returned (paginated) nodes
  "pagination": {                // optional; absent when pagination is off
    "page": 1,
    "page_size": 10,
    "total_items": 4,
    "total_pages": 1,
    "has_next": false,
    "has_prev": false
  }
}
```

Optionality rules (preserved exactly):
- `max_tokens`, `auto_tune_applied`, `pagination` are **omitted** when
  unset (not `null`). serde: `#[serde(skip_serializing_if = "Option::is_none")]`.
- `pagination` appears only when `--page` is set and `--all` is not.

### 2.1 Effort presets — exact bundles

Source: `EFFORT_PRESETS` in `src/cli.ts`. Default effort is **`quick`**.

| effort | before | after | max_nodes |
| :--- | ---: | ---: | ---: |
| `scan`   | 20   | 20   | 100000 (effectively uncapped / index mode) |
| `quick`  | 200  | 200  | 10  |
| `normal` | 500  | 500  | 30  |
| `deep`   | 2000 | 2000 | 100 |
| `auto`   | 500  | 500  | 30  (aliases `normal`) |

> The runtime preset list is `scan/quick/normal/
> deep`; `auto` is a valid `Effort` value that
> resolves to the `normal` bundle. `--strategy` defaults to `fill`.

---

## 3. agent-json envelope **[load-bearing]**

Source: `AgentEnvelope` + `buildAgentEnvelope` in `src/format.ts`.
`--format agent-json` is `JSON.stringify(envelope, null, 2)`.

```jsonc
{
  "status": "ok",            // "ok"|"no_matches"|"truncated"|"partial"|"error"
  "pattern": "TODO",
  "n_literal_matches": 12,   // literal-match count (0 when fuzzy fired)
  "n_fuzzy_matches": 0,      // total_nodes when fuzzy fired, else 0
  "fallback_used": null,     // null | "fuzzy" | "fill"
  "warning": null,           // string | null
  "nodes": [ /* Node[] — §1 */ ],
  "next_suggestion": null,   // string | null
  "errors": []               // unknown[]; the result's errors, or []
}
```

Construction rules to match (from `buildAgentEnvelope`):
- `status` starts from the `Result.status`; becomes `"truncated"` when
  the result was truncated; `"no_matches"` when 0 literal and no fuzzy.
- `fallback_used`:
  - `"fuzzy"` when the fuzzy driver fired;
  - else `"fill"` when fill was *not* suppressed, nodes were returned,
    and `total_nodes === max_nodes`;
  - else `null`.
- `n_literal_matches` = `fuzzyFired ? 0 : literalMatches`.
- `n_fuzzy_matches` = `fuzzyFired ? total_nodes : 0`.
- Warning / next_suggestion heuristics:
  - 0 literal + fuzzy fired → `warning: "literal pattern matched
    nothing; fell back to fuzzy"`, `next_suggestion: "try rephrasing or
    use simpler keywords"`.
  - 0 literal + no fuzzy → `status: "no_matches"`,
    `next_suggestion: "no matches — try simpler keywords or check the path"`.
  - \>50 literal + truncated → `warning: "pattern matched N lines;
    consider narrowing"`.
  - `--no-fill` + fewer nodes than `max_nodes` → its own warning branch.

> Note: `status: "partial"` exists **only** in the agent envelope, never
> in raw `Result.status`. The
> `warning`/`next_suggestion` strings are part of the contract — scrt
> reproduces them verbatim (subject to §Branding for any command text).

> **CLI vs server opts (load-bearing).** The v0.x **CLI** path calls
> `buildAgentEnvelope(result)` with **no opts** (`format.ts:101`). So from
> `mpg --format agent-json`, the construction rules above run with
> `fuzzyFired=false`, `noFill=false`, `literalMatchCount=total_nodes`
> ALWAYS — meaning the CLI never emits `fallback_used:"fuzzy"`, never the
> "fell back to fuzzy" warning, and never the strict-mode warning, even
> when `--fuzzy` / `--no-fill` were passed. Those opts are threaded only by
> the warm-process **server** (`server.ts`). scrt reproduces this: the CLI
> uses default opts; the engine's `search_with_meta` returns
> `{fuzzy_fired, literal_match_count}` for the server to thread. A naive
> "thread the opts in the CLI too" implementation diverges from mpg — this
> was caught by the round-trip sweep.

---

## 4. Palace file format **[load-bearing]**

Source: `Palace` / `Stash` / `StashRelation` / `StashedNode` in
`src/mind-palace.ts`. On-disk at `./.mpg/mind-palace.json` (override:
`MPG_MIND_PALACE` env or `--mp-path`). `version` is **2**. Files with
version < 2 (or no version) load fine — the two staleness fields are
optional and default-safe; no migration step.

```jsonc
{
  "version": 2,
  "stashes": {
    "auth-todos": {
      "name": "auth-todos",
      "note": "Auth TODOs to review",
      "tags": ["auth", "p0"],
      "created_at": "2026-06-13T08:42:00.000Z",   // ISO 8601
      "updated_at": "2026-06-13T08:42:00.000Z",
      "expires_at": null,                          // ISO string | null
      "search": {
        "pattern": "TODO",
        "effort": "normal",
        "sources_count": 3
      },
      "sources": [                                 // all source paths (context)
        "src/auth/login.ts",
        "src/auth/session.ts"
      ],
      "nodes": [ /* StashedNode[] — §4.1 */ ],
      "file_paths": [                              // filesystem paths only
        "src/auth/login.ts",
        "src/auth/session.ts"
      ],
      "relations": [ /* StashRelation[] — §4.2 */ ]
    }
  }
}
```

### 4.1 `StashedNode`

A compact, self-contained snapshot of a `Node` (no `id`/`match_spans`;
adds `file_path`, `source_type`, and the two staleness anchors).

```jsonc
{
  "source": "src/auth/login.ts",
  "file_path": "src/auth/login.ts",   // canonical fs path, or null if not a file
  "source_type": "file",
  "match_line": 8,
  "start_line": 5,
  "end_line": 11,
  "context_before": ["...", "...", "..."],
  "match_text": "  // TODO: add rate limiting ...",
  "context_after": ["...", "...", "..."],
  "tokens": 196,
  "source_mtime_ms": 1718270520000,   // optional; file-source nodes only
  "match_line_hash": "a1b2c3d4e5f6"   // optional; 12-hex-char SHA1 (§4.3)
}
```

Optionality: `source_mtime_ms` and `match_line_hash` are **omitted** for
legacy/non-file nodes (treated as "unknown" freshness), not emitted as
`null`. scrt uses `skip_serializing_if = "Option::is_none"` so a stash it
writes is byte-identical to a v0.x stash for the same capture.

### 4.2 `StashRelation`

```jsonc
{
  "target": "perf-hotspots",
  "type": "depends-on",        // or any custom string
  "note": "shared db layer",
  "created_at": "2026-06-13T08:45:00.000Z"
}
```

### 4.3 Staleness anchors — must match exactly

- `source_mtime_ms`: file mtime in **ms since epoch** at capture time.
- `match_line_hash`: **first 12 hex chars of the SHA1** of the trimmed
  match line. Anchor text = `match_text` (the raw matched line);
  if empty, fall back to the first non-empty `context_before`, then
  `context_after`. scrt uses the `sha1` crate and truncates to 12 hex
  chars to keep this byte-identical, so a palace written by either impl
  validates against the other.

Computed `stale` states a retrieved node may carry (render-time, not
stored): `false` | `"unknown"` | `"mtime_advanced_content_intact"` |
`"mtime_advanced"` | `"content_drifted"` | `"file_missing"`.

---

## 5. tool-spec output **[load-bearing]**

Source: `src/tool-spec.ts`. `scrt tool-spec --format <fmt>` emits the
descriptor for five tools: `*_search`, `*_stash`, `*_list_stashes`,
`*_get_stash`, `*_drop_stash`. **Branding:** v0.x names them `mpg_*`;
scrt emits `scrt_*` (see §Branding). The schemas (`parameters`/
`input_schema`) are otherwise identical.

### 5.1 OpenAI shape — array of `{ type, function }`

```jsonc
[
  {
    "type": "function",
    "function": {
      "name": "scrt_search",              // v0.x: "mpg_search"
      "description": "Token-budgeted codebase search via ripgrep. ...",
      "parameters": {
        "type": "object",
        "properties": { /* SEARCH_PROPERTIES — §5.4 */ },
        "required": ["pattern"]
      }
    }
  }
  // ... scrt_stash, scrt_list_stashes, scrt_get_stash, scrt_drop_stash
]
```

### 5.2 Anthropic shape — array of `{ name, description, input_schema }`

```jsonc
[
  {
    "name": "scrt_search",                // v0.x: "mpg_search"
    "description": "Token-budgeted codebase search via ripgrep. ...",
    "input_schema": {
      "type": "object",
      "properties": { /* SEARCH_PROPERTIES — §5.4 */ },
      "required": ["pattern"]
    }
  }
  // ...
]
```

### 5.3 Gemini shape — `{ functionDeclarations: [...] }`

```jsonc
{
  "functionDeclarations": [
    {
      "name": "scrt_search",              // v0.x: "mpg_search"
      "description": "Token-budgeted codebase search via ripgrep. ...",
      "parameters": {
        "type": "object",
        "properties": { /* SEARCH_PROPERTIES — §5.4 */ },
        "required": ["pattern"]
      }
    }
    // ...
  ]
}
```

### 5.4 Property schemas (verbatim, abbreviated)

The `search` tool's `properties` (full text in `src/tool-spec.ts`
`SEARCH_PROPERTIES`): `pattern` (required), `in` (string[]), `cmd`,
`url`, `effort` (enum `scan|normal|deep|auto`), `max_tokens`,
`max_nodes`, `clip_chars`, `sort` (enum `relevance|recent|oldest`),
`window_curve` (enum `flat|linear|log`), `retriever`, `mp_from`,
`mp_stash`, `mp_tag`, `mp_ttl`, `page`, `page_size`.

The `stash` tool's `properties` (`STASH_PROPERTIES`): `name` (required),
`note` (required), `tags` (string[]), `replace` (bool), `ttl`,
`palace_path`.

> The description strings mention "ripgrep" and contain no `mpg`/`scrt`
> token, so they pass through unchanged. The **only** brand-bearing field
> in tool-spec is the tool `name`. See §Branding.

---

## Branding — the `scrt` ↔ `mpg` normalization

Per DESIGN.md §5 (decision: rebrand to `scrt`), scrt emits `scrt` where
v0.x emits `mpg` in **user-facing JSON**. This is the exhaustive list of
brand-bearing positions, so the parity harness's normalization pass
(applied before diffing) is complete, not best-effort:

| Location | v0.x | scrt | In parity diff |
| :--- | :--- | :--- | :--- |
| tool-spec tool `name` | `mpg_search`, `mpg_stash`, `mpg_list_stashes`, `mpg_get_stash`, `mpg_drop_stash` | `scrt_*` | normalized then compared |
| `llm`/`text` result block tag | `<mpg result …>` … `</mpg result>` | `<scrt result …>` | normalized (non-JSON formats) |
| palace block tags | `<mpg mind-palace …>`, `<mpg mind-palace-get …>`, `<mpg mind-palace-stash …/>` | `<scrt …>` | normalized |
| `next_suggestion` / `warning` strings | (none contain `mpg` today) | (same) | pass-through; re-audit per release |
| env var | `MPG_MIND_PALACE` | **kept** (`MPG_MIND_PALACE`) | n/a — not in output |
| default dir | `.mpg/` | **kept** (`.mpg/`) | n/a — not in output |

**Not rebranded (byte-identical):** the palace file format (§4) carries
no brand string in any field — `version`, stash keys, all node fields are
brand-free — so a palace round-trips unchanged. tool-spec `description`
strings and all JSON Schema `properties` are byte-identical. The
`MPG_MIND_PALACE` env var and `.mpg/` directory are kept so existing
configs and palace files resolve without migration.

> The golden harness normalizes `\bmpg\b` ↔ `scrt` and `mpg_` ↔ `scrt_`
> on **both** sides before diffing, and asserts byte-equality on
> everything else. The on-disk palace is **not** touched by branding at all.

---

## Excluded from the parity diff

These fields are environment/timing-dependent and are normalized out (or
asserted only for presence/type, not value) before a byte diff:

- `Result.duration_ms` — wall-clock; varies per run.
- Any `…ms` / timestamp field whose value is "now" at write time
  (`created_at`, `updated_at` on a freshly written stash) — compared for
  ISO-8601 *shape*, not value, in golden tests; pinned via injected clock
  where a deterministic fixture is needed.
- `source_mtime_ms` — filesystem-dependent; asserted as "present and
  numeric for file nodes", value pinned only in hermetic fixtures.

Everything else is a byte-for-byte contract.

---

## Golden fixtures

No palace-file fixtures ship in `mind-palace-graph/test/` (only
`smoke.ts`). The round-trip harness therefore **generates** goldens by
running Node `mpg` v0.3.2 against a fixed corpus (the `mind-palace-graph`
repo itself) and capturing:

1. `--format json` for a representative pattern set → node-shape goldens.
2. `--format agent-json` across status outcomes → envelope goldens.
3. A `--mp-stash` round → a real `mind-palace.json` → palace-format golden.

scrt's output is diffed against these (timing/branding excluded per above);
the goldens are checked into the workspace as the executable form of this
document. The **tool-spec** is checked differently: rather than a
byte-for-byte golden diff, `tool_spec_parity.rs` asserts a *contract* — all
three provider shapes are well-formed, the 5 mpg tools remain present as a
recognizable superset, `scrt_similar` is present, and per-tool key order is
preserved.
