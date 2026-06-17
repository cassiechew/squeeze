#!/usr/bin/env python3
"""
PreToolUse hook for Bash: auto-wrap high-value commands through squeeze.

When a recognised command (cargo test, pytest, etc.) is detected, the
tool input's command is rewritten to:

    set -o pipefail && { CMD ; } 2>&1 | ~/.cargo/bin/squeeze -- CMD

`pipefail` makes the pipeline return the leftmost non-zero exit code,
so if the test command fails, the whole pipeline fails with that code.
This is shell-agnostic (works in both bash and zsh) unlike PIPESTATUS
which is bash-only.

Compound commands (containing &&, ||, ;, |), already-wrapped commands,
and commands with shell substitutions are passed through unchanged.

Output protocol: emits JSON to stdout with hookSpecificOutput.updatedInput.
Empty stdout + exit 0 means "no rewrite, continue normally".
"""

import json
import os
import re
import sys


# Patterns that mark a command as a "high-value" candidate for compression.
# Every pattern is anchored to start-of-string and uses \b to require a word
# boundary so we don't accidentally match e.g. "go-test-helper".
#
# `(?:\S*/)?` allows an optional path prefix so we still match when the user
# invokes the binary by absolute or relative path (e.g. `~/.cargo/bin/cargo
# test` or `./node_modules/.bin/jest`).
PATH_PREFIX = r"(?:\S*/)?"

# Direct binary invocations.
TRIGGERS = [
    rf"^{PATH_PREFIX}cargo\s+test\b",
    rf"^{PATH_PREFIX}cargo\s+nextest\b",
    rf"^{PATH_PREFIX}cargo\s+clippy\b",
    rf"^{PATH_PREFIX}go\s+test\b",
    rf"^{PATH_PREFIX}pytest\b",
    rf"^{PATH_PREFIX}py\.test\b",
    rf"^{PATH_PREFIX}python3?\s+-m\s+pytest\b",
    rf"^(?:npx\s+)?{PATH_PREFIX}vitest\b",
    rf"^(?:npx\s+)?{PATH_PREFIX}jest\b",
    rf"^(?:npx\s+)?{PATH_PREFIX}eslint\b",
    rf"^{PATH_PREFIX}biome\s+check\b",
    # Bun's built-in test runner.
    r"^bun\s+test\b",
]

# Package-manager script invocations. We don't know what the script actually
# runs - could be jest, vitest, mocha, eslint, anything. We trust that:
# - if it's a known test runner, squeeze's output-shape detection routes it
# - if it's something else (e.g. a build script aliased as "test"), squeeze
#   falls back to its lossless passthrough parser
# Either way, wrapping is safe and the worst case is "passthrough did nothing
# useful" (a no-op cost of a few ms).
SCRIPT_TRIGGERS = [
    # `npm test` is shorthand for `npm run test`. We accept either form,
    # plus any namespaced variant (test:integration, test:e2e, etc.) and
    # the closely-related lint scripts.
    r"^(?:npm|yarn|pnpm)\s+(?:run\s+)?tests?\b",
    r"^(?:npm|yarn|pnpm)\s+(?:run\s+)?lint\b",
    # bun has both `bun test` (built-in) and `bun run X` (package script).
    r"^bun\s+run\s+(?:tests?|lint)\b",
]
TRIGGERS.extend(SCRIPT_TRIGGERS)

# Tokens that mean "this is a compound command" - we leave those alone
# rather than try to splice squeeze into a chain.
COMPOUND_OPERATORS = ("&&", "||", ";", "|")

# Tokens that mean "shell substitution is happening" - re-evaluating the
# command after `--` would run them twice, which is unsafe.
SUBSTITUTION_TOKENS = ("$(", "`", ">(", "<(")

# Known env-activation prefixes that appear before the real command.
# These are safe to treat as a "preamble && actual_cmd" and wrap only
# the actual_cmd part. The preamble sets PATH / env vars and does not
# produce output the agent cares about.
ENV_PREFIXES = re.compile(
    r"^\s*(?:"
    r"\.\s+(?:[^\s;|&]+(?:/activate-hermit|/activate|/env)|\.env[^\s;|&]*)"  # . bin/activate-hermit, . .env
    r"|source\s+(?:[^\s;|&]+(?:/activate-hermit|/activate|/env)|\.env[^\s;|&]*)"  # source ...
    r"|export\s+\S+"  # export FOO=bar
    r"|cd\s+\S+"  # cd /path
    r")"
    r"\s*&&\s*",
)


def strip_env_prefix(cmd: str) -> tuple[str, str]:
    """Strip known env-activation prefixes from the front of a compound command.

    Returns (prefix, remainder). prefix includes the trailing ' && '.
    If no known prefix is found, returns ('', cmd).
    Handles chained prefixes like '. env && cd dir && cargo test'.
    """
    remaining = cmd
    prefix_parts = []
    while True:
        m = ENV_PREFIXES.match(remaining)
        if not m:
            break
        prefix_parts.append(m.group(0))
        remaining = remaining[m.end():]
    return ("".join(prefix_parts), remaining)


def should_wrap(cmd: str) -> bool:
    if not cmd:
        return False
    if "squeeze" in cmd:
        return False

    # Try stripping known env-activation prefixes first. If what remains
    # is still compound (has operators), bail.
    _, actual = strip_env_prefix(cmd)

    if any(op in actual for op in COMPOUND_OPERATORS):
        return False
    if any(tok in actual for tok in SUBSTITUTION_TOKENS):
        return False

    trimmed = actual.lstrip()

    # `git log` is special-cased: --oneline is already compact, skip.
    if re.match(rf"^{PATH_PREFIX}git\s+log\b", trimmed):
        return "--oneline" not in actual

    return any(re.match(p, trimmed) for p in TRIGGERS)


def main() -> None:
    try:
        data = json.load(sys.stdin)
    except Exception:
        # Malformed input - pass through with no change.
        return

    cmd = (data.get("tool_input") or {}).get("command", "")
    if not should_wrap(cmd):
        return

    prefix, actual = strip_env_prefix(cmd)
    sq = os.path.expanduser("~/.cargo/bin/squeeze")

    # Use `set -o pipefail` so the pipeline returns the leftmost non-zero
    # exit code. This is shell-agnostic (works in both bash and zsh) unlike
    # PIPESTATUS which is bash-only and doesn't survive && chains.
    if prefix:
        new_cmd = (
            f"{prefix}set -o pipefail && "
            f"{{ {actual}; }} 2>&1 | {sq} -- {actual}"
        )
    else:
        new_cmd = (
            f"set -o pipefail && "
            f"{{ {cmd}; }} 2>&1 | {sq} -- {cmd}"
        )

    out = {
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "updatedInput": {"command": new_cmd},
            "additionalContext": (
                "[squeeze] Auto-wrapped high-value command for lossless "
                "compression. Output is grouped failure-first; any "
                "omissions are declared with `(... pass --flag to include)` "
                "footers. Strip the wrapper if you need raw output."
            ),
        }
    }
    json.dump(out, sys.stdout)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
