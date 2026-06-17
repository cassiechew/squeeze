//! Parser for `gh pr view` and `gh pr list`. Both subcommands use the
//! same surface because the detection cost is minimal.

use crate::core::{CommandParser, CompressOptions, CompressedOutput, Strategy};
use crate::lossless::strip_ansi;

pub struct GhPrParser;

impl CommandParser for GhPrParser {
    fn strategy(&self) -> Strategy {
        Strategy::GhPrView
    }

    fn can_handle(&self, command: &str, _output: &str) -> bool {
        is_gh_pr(command)
    }

    fn parse(&self, output: &str, _opts: &CompressOptions) -> CompressedOutput {
        let clean = strip_ansi(output);
        if is_list_command() || looks_like_list(&clean) {
            return parse_list(&clean, Strategy::GhPrList);
        }
        parse_view(&clean, Strategy::GhPrView)
    }
}

fn is_gh_pr(command: &str) -> bool {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    let mut seen_gh = false;
    let mut seen_pr = false;
    for t in tokens {
        if t == "gh" {
            seen_gh = true;
            continue;
        }
        if seen_gh && !t.starts_with('-') {
            if t == "pr" {
                seen_pr = true;
                continue;
            }
            if seen_pr {
                return matches!(t, "view" | "list" | "status" | "checks" | "ready" | "diff");
            }
        }
    }
    false
}

// Hack: we can't see the command inside parse() in the current trait, so the
// list vs view split is determined from output shape in `looks_like_list`.
// This keeps the trait surface small; the list format is distinctive enough
// (tabular rows starting with `#N`).
fn is_list_command() -> bool {
    false
}

fn looks_like_list(output: &str) -> bool {
    // Heuristic: multiple lines starting with `#` followed by digits, or
    // tab-separated table shape.
    let mut candidate_rows = 0;
    for l in output.lines().take(20) {
        let t = l.trim_start();
        if t.starts_with('#') && t.chars().nth(1).is_some_and(|c| c.is_ascii_digit()) {
            candidate_rows += 1;
        }
        if l.contains('\t') && l.split('\t').count() >= 3 {
            candidate_rows += 1;
        }
    }
    candidate_rows >= 2
}

pub(crate) fn parse_list(input: &str, strategy: Strategy) -> CompressedOutput {
    // Minimal-intervention philosophy: the agent needs the data, not a
    // reformat. We strip the `Showing N of M ...` preamble and collapse
    // runs of whitespace (which is how space-padded gh output wastes
    // bytes) but we do NOT reshape tab-delimited rows - those are already
    // tight and any formatting we add would grow the output.
    let mut out = String::new();
    for raw in input.lines() {
        let line = raw.trim_end();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("Showing ") {
            continue;
        }
        if line.contains('\t') {
            // Tab-delimited: keep as-is, tabs are cheap and unambiguous.
            out.push_str(line);
        } else {
            // Space-padded: collapse runs of 2+ spaces to a single space.
            // Preserves every token and every column value.
            out.push_str(&collapse_runs_of_spaces(line));
        }
        out.push('\n');
    }
    CompressedOutput {
        content: out,
        strategy,
        omitted: None,
        lossless: true,
    }
}

fn collapse_runs_of_spaces(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_space_run = false;
    for c in s.chars() {
        if c == ' ' {
            if !in_space_run {
                out.push(' ');
                in_space_run = true;
            }
        } else {
            out.push(c);
            in_space_run = false;
        }
    }
    out
}

pub(crate) fn parse_view(input: &str, strategy: Strategy) -> CompressedOutput {
    // Minimal-intervention philosophy: preserve every field, every body
    // line, every comment. The only things we touch are decorative chrome
    // that carry no agent-usable signal: the 80-char `----...----`
    // horizontal rule between body and comments, and runs of 2+ blank
    // lines collapsed to one.
    let mut out = String::new();
    let mut prev_blank = false;
    for raw in input.lines() {
        let line = raw.trim_end();
        if is_horizontal_rule(line) {
            continue;
        }
        let is_blank = line.is_empty();
        if is_blank && prev_blank {
            continue;
        }
        out.push_str(line);
        out.push('\n');
        prev_blank = is_blank;
    }
    // Trim trailing blank line.
    while out.ends_with("\n\n") {
        out.pop();
    }
    CompressedOutput {
        content: out,
        strategy,
        omitted: None,
        lossless: true,
    }
}

/// A line of 10+ consecutive `-` characters with no other content is the
/// decorative separator gh prints between a PR/issue body and its
/// comments. Drop it.
fn is_horizontal_rule(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.len() >= 10 && trimmed.chars().all(|c| c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW_INPUT: &str = "\
title:\tAdd user authentication
state:\tOPEN
author:\tcassie
labels:\tenhancement, backend
base:\tmain
--
Implements JWT authentication for API endpoints.
Tokens expire after 24h.
--------------------------------------------------------------------------------
author:\treviewer
created at:\t2026-04-22
--
Looks good, one nit...

author:\tcassie
created at:\t2026-04-22
--
Fixed, thanks!
";

    const LIST_INPUT: &str = "\
Showing 3 of 3 pull requests in owner/repo

#42\tAdd user authentication\tfeature/auth\tOPEN\tabout 2 days ago
#41\tFix config loading\tbugfix/config\tMERGED\tabout 3 days ago
#40\tInitial scaffold\tinit\tCLOSED\tabout 1 week ago
";

    #[test]
    fn view_preserves_body_and_comments() {
        let p = GhPrParser;
        let out = p.parse(VIEW_INPUT, &CompressOptions::default());
        assert!(out.content.contains("Add user authentication"));
        assert!(out.content.contains("Implements JWT authentication"));
        assert!(out.content.contains("Tokens expire after 24h"));
        assert!(out.content.contains("Looks good, one nit"));
        assert!(out.content.contains("Fixed, thanks"));
        // The decorative `----...----` rule between body and comments is stripped.
        assert!(!out.content.contains("----------"));
        assert!(out.lossless);
        assert!(out.omitted.is_none());
    }

    #[test]
    fn list_preserves_all_rows_and_fields() {
        let p = GhPrParser;
        let out = p.parse(LIST_INPUT, &CompressOptions::default());
        // Every PR number and title is preserved verbatim (tab-delimited).
        for needle in [
            "#42",
            "Add user authentication",
            "feature/auth",
            "OPEN",
            "#41",
            "Fix config loading",
            "MERGED",
            "#40",
            "Initial scaffold",
            "CLOSED",
        ] {
            assert!(out.content.contains(needle), "missing: {needle}");
        }
        // The `Showing N of M ...` preamble is dropped.
        assert!(!out.content.contains("Showing"));
        assert!(out.lossless);
    }

    #[test]
    fn list_space_padded_collapses_runs() {
        let p = GhPrParser;
        // Space-padded form (terminal-attached gh output).
        let input = "\
Showing 2 of 2 pull requests

#42  Add auth          feature/auth  OPEN    about 2 days ago
#41  Fix config        bugfix/cfg    MERGED  about 3 days ago
";
        let out = p.parse(input, &CompressOptions::default());
        // Multi-space padding collapsed.
        assert!(
            !out.content.contains("  "),
            "runs of spaces should be collapsed; got:\n{}",
            out.content
        );
        for n in ["#42", "Add auth", "feature/auth", "OPEN", "#41"] {
            assert!(out.content.contains(n));
        }
    }

    #[test]
    fn empty_input_is_safe() {
        let p = GhPrParser;
        let out = p.parse("", &CompressOptions::default());
        assert!(out.lossless);
    }

    #[test]
    fn detects_command() {
        let p = GhPrParser;
        assert!(p.can_handle("gh pr view 42", ""));
        assert!(p.can_handle("gh pr list", ""));
        assert!(p.can_handle("gh pr checks", ""));
        assert!(!p.can_handle("gh issue view 42", ""));
    }

    #[test]
    fn lossless_every_pr_in_list() {
        let p = GhPrParser;
        let out = p.parse(LIST_INPUT, &CompressOptions::default());
        for n in ["#42", "#41", "#40"] {
            assert!(out.content.contains(n), "missing {n}");
        }
    }
}
