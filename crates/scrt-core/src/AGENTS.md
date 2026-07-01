# scrt-core/src — module notes

Directory-level rationale for `scrt-core`. Source files carry terse one-line
doc-comments; the "why" lives here.

## §format (`format.rs`)

`format.rs` renders a `SearchResult` in five output formats. `json` /
`agent-json` are byte-compatible with v0.x mpg (see `COMPAT.md`); `text` /
`markdown` are close ports. The **`llm` format** (the CLI default) diverges
on purpose.

### Why the `llm` node framing is compact (TOON-style)

The v0.x `llm` output spent a large fraction of its tokens on decoration an
LLM consumer never needs:

- a `--- NODE N of M | file:line | ~T tokens ---` banner per node — the words
  `NODE`, `of`, `tokens` and the dashes are scaffolding; only `id`,
  `file:line`, and the token count are load-bearing;
- a blank line before every node purely as a separator;
- an absolute line number on *every* context line, when the header already
  anchors the match line and the lines are contiguous;
- a `--- TOTAL ---` banner on the footer.

The compact framing keeps every piece of **metadata** and drops only the
decoration:

```
§3 src/auth.ts:42 ~87t
   const token = ...
42› const «hit» = verify(token)
   return token
```

- `§<id> <file>:<line> ~<tokens>t` — one sigil (`§`) marks the node boundary,
  so it replaces both the banner *and* the leading blank line.
- Context lines get a bare gutter (width matches the match-line number so the
  code column stays aligned); only the **match line** carries its absolute
  number plus the `›` marker. This is the "middle ground" tradeoff: the match
  anchor stays precise, context lines lose their per-line numbers (recoverable
  by counting from the anchor).
- `«hit»` replaces `**hit**` for the highlight (one code unit each side vs two).
- `Σ …` replaces `--- TOTAL ---`.

Net effect is roughly a 15–25% token reduction on multi-node results with no
loss of the metadata (id, path, line, token count, match span) an agent uses
to reason about or re-fetch a hit.

### Contract boundary

This divergence is **not** part of the v0.x parity contract. The checked-in
goldens (`tests/golden/`) only cover `json` / `agent-json`, which are
brand-token-free and remain byte-identical. The inline
`llm_block_is_scrt_branded` test in `format.rs` pins the new shape.
