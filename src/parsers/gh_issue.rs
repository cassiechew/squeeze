//! Parser for `gh issue view` and `gh issue list`. Reuses the PR parser's
//! shared logic since the surface is nearly identical.

use crate::core::{CommandParser, CompressOptions, CompressedOutput, Strategy};
use crate::lossless::strip_ansi;
use crate::parsers::gh_pr;

pub struct GhIssueParser;

impl CommandParser for GhIssueParser {
    fn strategy(&self) -> Strategy {
        Strategy::GhIssueView
    }

    fn can_handle(&self, command: &str, _output: &str) -> bool {
        is_gh_issue(command)
    }

    fn parse(&self, output: &str, _opts: &CompressOptions) -> CompressedOutput {
        let clean = strip_ansi(output);
        if looks_like_list(&clean) {
            return gh_pr::parse_list(&clean, Strategy::GhIssueList);
        }
        gh_pr::parse_view(&clean, Strategy::GhIssueView)
    }
}

fn is_gh_issue(command: &str) -> bool {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    let mut seen_gh = false;
    let mut seen_issue = false;
    for t in tokens {
        if t == "gh" {
            seen_gh = true;
            continue;
        }
        if seen_gh && !t.starts_with('-') {
            if t == "issue" {
                seen_issue = true;
                continue;
            }
            if seen_issue {
                return matches!(t, "view" | "list" | "status" | "comment");
            }
        }
    }
    false
}

fn looks_like_list(output: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    const LIST_INPUT: &str = "\
Showing 2 of 2 issues in owner/repo

#17\tBug in login flow\topen\tabout 1 day ago
#12\tFlaky test\tclosed\tabout 1 week ago
";

    #[test]
    fn list_formats_issues() {
        let p = GhIssueParser;
        let out = p.parse(LIST_INPUT, &CompressOptions::default());
        assert!(out.content.contains("#17"));
        assert!(out.content.contains("Bug in login flow"));
        assert!(out.content.contains("#12"));
        assert!(out.content.contains("Flaky test"));
        assert!(!out.content.contains("Showing"));
    }

    #[test]
    fn detects_command() {
        let p = GhIssueParser;
        assert!(p.can_handle("gh issue view 17", ""));
        assert!(p.can_handle("gh issue list", ""));
        assert!(!p.can_handle("gh pr view 17", ""));
    }

    #[test]
    fn lossless_numbers_preserved() {
        let p = GhIssueParser;
        let out = p.parse(LIST_INPUT, &CompressOptions::default());
        assert!(out.content.contains("#17"));
        assert!(out.content.contains("#12"));
    }

    #[test]
    fn empty_input_is_safe() {
        let p = GhIssueParser;
        let out = p.parse("", &CompressOptions::default());
        assert!(out.lossless);
    }
}
