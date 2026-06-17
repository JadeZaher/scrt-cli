#!/usr/bin/env node
// Golden generator: runs the Node `mpg` reference against the checked-in
// corpus and writes normalized JSON goldens that the Rust round-trip test
// (`tests/roundtrip.rs`) diffs scrt's output against.
//
// Normalization (matches COMPAT.md §Excluded + §Branding):
//   - duration_ms -> 0
//   - source.id absolute path -> the corpus basename (portable across machines)
//   - mpg -> scrt is NOT applied here (the `json` format carries no brand
//     token; branding only affects llm/text/tool-spec, handled in Prompt 3/6).
//
// Usage:
//   node gen_goldens.mjs <path-to-mpg-dist-index.js>
// Re-run whenever the case matrix changes. Goldens are checked in so the
// Rust test runs without Node present.

import { execFileSync } from "node:child_process";
import { writeFileSync, mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join, basename } from "node:path";

const __dirname = dirname(fileURLToPath(import.meta.url));
const corpusDir = join(__dirname, "corpus");
const goldenDir = join(__dirname, "golden");
mkdirSync(goldenDir, { recursive: true });

const mpgEntry = process.argv[2];
if (!mpgEntry) {
  console.error("usage: node gen_goldens.mjs <path-to-mpg/dist/index.js>");
  process.exit(2);
}

// Case matrix — name -> argv (pattern + flags). `--in` paths are filled in
// per-corpus below. Each case is run against every corpus file.
const CASES = [
  { name: "normal", args: ["--effort", "normal"] },
  { name: "quick", args: ["--effort", "quick"] },
  { name: "scan", args: ["--effort", "scan"] },
  { name: "deep", args: ["--effort", "deep"] },
  { name: "scan-clip3", args: ["--effort", "scan", "--clip", "3"] },
  { name: "scan-clip20", args: ["--effort", "scan", "--clip", "20"] },
  { name: "maxnodes2", args: ["--effort", "scan", "--max-nodes", "2"] },
  { name: "maxtokens10", args: ["--effort", "normal", "--max-tokens", "10"] },
  { name: "icase", args: ["--effort", "normal", "-I"] },
  { name: "word", args: ["--effort", "normal", "-w"] },
  // Prompt 3 surface:
  { name: "curve-linear", args: ["--effort", "normal", "--window-curve", "linear"] },
  { name: "curve-log", args: ["--effort", "normal", "--window-curve", "log"] },
  { name: "strategy-deep", args: ["--effort", "normal", "--max-tokens", "12", "--strategy", "deep"] },
  { name: "agentjson-ok", args: ["--effort", "normal", "--format", "agent-json"] },
  { name: "agentjson-fill", args: ["--effort", "scan", "--max-nodes", "3", "--format", "agent-json"] },
  { name: "agentjson-nofill", args: ["--effort", "scan", "--max-nodes", "99", "--no-fill", "--format", "agent-json"] },
  { name: "agentjson-nomatch", args: ["--effort", "normal", "--format", "agent-json"], patternOverride: "zzznope" },
  { name: "fuzzy-scan", args: ["--fuzzy", "--effort", "scan", "--clip", "10"] },
  { name: "fuzzy-agentjson", args: ["--fuzzy", "--effort", "scan", "--clip", "10", "--format", "agent-json"] },
];

// pattern per corpus file
const CORPUS = [
  { file: "basic.txt", pattern: "TODO" },
  { file: "multi.txt", pattern: "ab" },
  { file: "code.txt", pattern: "alpha" },
  { file: "fuzz.txt", pattern: "Provider" },
];

function normalize(jsonText, corpusFile) {
  const obj = JSON.parse(jsonText);
  // Result objects carry duration_ms; the agent-json envelope does not.
  // Only normalize it when present, so we don't inject a spurious key.
  if ("duration_ms" in obj) obj.duration_ms = 0;
  for (const n of obj.nodes ?? []) {
    if (n.source && typeof n.source.id === "string") {
      // Replace any absolute path ending in the corpus basename with the
      // basename itself, so goldens are machine-independent.
      const b = basename(n.source.id.replace(/\\/g, "/"));
      n.source.id = b;
    }
  }
  return JSON.stringify(obj, null, 2);
}

let count = 0;
for (const { file, pattern } of CORPUS) {
  const corpusPath = join(corpusDir, file);
  for (const c of CASES) {
    const pat = c.patternOverride ?? pattern;
    // Default to json unless the case args already set a --format.
    const hasFormat = c.args.includes("--format");
    const fmtArgs = hasFormat ? [] : ["--format", "json"];
    const argv = [mpgEntry, pat, "--in", corpusPath, ...fmtArgs, ...c.args];
    let out;
    try {
      out = execFileSync(process.execPath, argv, { encoding: "utf8" });
    } catch (e) {
      // mpg exits 1 on no-match but still prints JSON on stdout.
      out = e.stdout ? e.stdout.toString() : null;
      if (!out) {
        console.error(`case ${file}/${c.name} produced no stdout: ${e.message}`);
        continue;
      }
    }
    const normalized = normalize(out, file);
    const goldenName = `${basename(file, ".txt")}.${c.name}.json`;
    writeFileSync(join(goldenDir, goldenName), normalized + "\n");
    count++;
  }
}
console.log(`wrote ${count} goldens to ${goldenDir}`);
