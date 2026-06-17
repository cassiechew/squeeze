#!/usr/bin/env python3
"""Register the squeeze PreToolUse hook in ~/.claude/settings.json.

Idempotent: if the hook is already present, prints a message and exits 0.
Merges into existing settings without clobbering other config.
"""

import json
import os
import sys

SETTINGS = os.path.expanduser("~/.claude/settings.json")
HOOK_CMD = os.path.expanduser("~/.claude/hooks/squeeze-rewrite.py")


def main():
    if os.path.exists(SETTINGS):
        with open(SETTINGS) as f:
            settings = json.load(f)
    else:
        settings = {}

    hook_entry = {
        "matcher": "Bash",
        "hooks": [
            {
                "type": "command",
                "command": HOOK_CMD,
                "timeout": 5,
                "statusMessage": "checking for squeeze rewrite",
            }
        ],
    }

    hooks = settings.setdefault("hooks", {})
    pre = hooks.setdefault("PreToolUse", [])

    already = any(
        any(
            h.get("command", "").endswith("squeeze-rewrite.py")
            for h in entry.get("hooks", [])
        )
        for entry in pre
        if entry.get("matcher") == "Bash"
    )

    if already:
        print("Hook already registered in settings.json")
        return

    pre.append(hook_entry)

    with open(SETTINGS, "w") as f:
        json.dump(settings, f, indent=2)
        f.write("\n")

    print(f"Registered PreToolUse hook in {SETTINGS}")


if __name__ == "__main__":
    main()
