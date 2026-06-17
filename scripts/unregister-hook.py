#!/usr/bin/env python3
"""Remove the squeeze PreToolUse hook from ~/.claude/settings.json.

Idempotent: if no hook is found, prints a message and exits 0.
Cleans up empty hooks/PreToolUse keys if they become empty.
"""

import json
import os
import sys

SETTINGS = os.path.expanduser("~/.claude/settings.json")


def main():
    if not os.path.exists(SETTINGS):
        print("No settings.json found")
        return

    with open(SETTINGS) as f:
        settings = json.load(f)

    pre = settings.get("hooks", {}).get("PreToolUse", [])
    filtered = [
        entry
        for entry in pre
        if not (
            entry.get("matcher") == "Bash"
            and any(
                h.get("command", "").endswith("squeeze-rewrite.py")
                for h in entry.get("hooks", [])
            )
        )
    ]

    if len(filtered) == len(pre):
        print("No squeeze hook found to remove")
        return

    settings["hooks"]["PreToolUse"] = filtered
    if not filtered:
        del settings["hooks"]["PreToolUse"]
    if not settings.get("hooks"):
        del settings["hooks"]

    with open(SETTINGS, "w") as f:
        json.dump(settings, f, indent=2)
        f.write("\n")

    print(f"Removed squeeze hook from {SETTINGS}")


if __name__ == "__main__":
    main()
