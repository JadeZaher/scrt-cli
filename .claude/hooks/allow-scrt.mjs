#!/usr/bin/env node
// PreToolUse hook: auto-approve `scrt` shell invocations so an agent isn't
// prompted for a PowerShell/Bash permission on every retrieval call.
//
// Wired in .claude/settings.json under hooks.PreToolUse for the Bash and
// PowerShell tools. It reads the tool call on stdin, and:
//   - APPROVES read-only / non-destructive scrt commands (search, --mp-list,
//     --mp-get, --mp-similar, --serve, tool-spec, etc.),
//   - stays SILENT (falls through to normal permission flow) for everything
//     else — including destructive scrt ops (--mp-drop, --mp-prune-all,
//     --mp-link/--mp-unlink) and any non-scrt command.
//
// "Silent" = exit 0 with no decision, so the user's normal allow/deny still
// applies. We never DENY here; we only fast-path the safe cases.

import { readFileSync } from "node:fs";

function readStdin() {
  try {
    return readFileSync(0, "utf8");
  } catch {
    return "";
  }
}

let payload = {};
try {
  payload = JSON.parse(readStdin() || "{}");
} catch {
  process.exit(0); // unparseable — defer to normal flow
}

// The shell command string lives under tool_input.command for both Bash and
// the PowerShell tool.
const command = String(payload?.tool_input?.command ?? "");
if (!command) process.exit(0);

// Destructive scrt subcommands we deliberately do NOT auto-approve.
const DESTRUCTIVE = [
  /--mp-drop\b/,
  /--mp-prune-all\b/,
  /--mp-prune-tag\b/,
  /--mp-prune-older-than\b/,
  /--mp-prune-keep\b/,
  /--mp-unlink\b/,
  /\bevolve\s+train\b/,
];

// Does the command invoke scrt? Match `scrt ` / `scrt.exe ` as a leading
// token of a (possibly chained) command, not a substring of some other word.
const INVOKES_SCRT = /(^|[;&|]\s*|\bnpx\s+|&&\s*)scrt(\.exe)?\b/;

function autoApprove() {
  // PreToolUse approve decision (Claude Code hook protocol).
  process.stdout.write(
    JSON.stringify({
      hookSpecificOutput: {
        hookEventName: "PreToolUse",
        permissionDecision: "allow",
        permissionDecisionReason: "scrt read-only/non-destructive invocation (auto-approved by allow-scrt hook)",
      },
    })
  );
  process.exit(0);
}

if (INVOKES_SCRT.test(command) && !DESTRUCTIVE.some((re) => re.test(command))) {
  autoApprove();
}

// Not a safe scrt command — defer to the normal permission flow.
process.exit(0);
