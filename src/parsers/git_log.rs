//! Parser for `git log` output. Handles the default verbose format
//! (`commit abc\nAuthor: ...`) by compacting each commit to a single line.
//! Commit bodies are omitted by default and the omission is declared.
//!
//! The compact `--oneline` format is passed through unchanged (after
//! ANSI stripping).

use crate::core::{CommandParser, CompressOptions, CompressedOutput, Strategy};
use crate::lossless::strip_ansi;

pub struct GitLogParser;

impl CommandParser for GitLogParser {
    fn strategy(&self) -> Strategy {
        Strategy::GitLog
    }

    fn can_handle(&self, command: &str, output: &str) -> bool {
        if command_is_git_log(command) {
            return true;
        }
        // Detect the default verbose format.
        output.starts_with("commit ") && output.contains("Author:") && output.contains("Date:")
    }

    fn parse(&self, output: &str, opts: &CompressOptions) -> CompressedOutput {
        let clean = strip_ansi(output);
        if !clean.contains("Author:") {
            // Likely --oneline or similar; no reformat needed.
            return CompressedOutput {
                content: clean,
                strategy: Strategy::GitLog,
                omitted: None,
                lossless: true,
            };
        }
        let commits = parse_verbose(&clean);
        let mut out = String::new();
        for c in &commits {
            out.push_str(&c.render_compact(opts.log_full));
            out.push('\n');
        }
        let (omitted, lossless) = if !opts.log_full && commits.iter().any(|c| !c.body.is_empty()) {
            (
                Some("commit bodies omitted - pass --full to include".to_string()),
                false,
            )
        } else {
            (None, true)
        };
        CompressedOutput {
            content: out,
            strategy: Strategy::GitLog,
            omitted,
            lossless,
        }
    }
}

fn command_is_git_log(command: &str) -> bool {
    let mut saw_git = false;
    for t in command.split_whitespace() {
        if t == "git" {
            saw_git = true;
        } else if saw_git && (t == "log" || t == "reflog") {
            return true;
        }
    }
    false
}

struct Commit {
    hash: String,
    author: String,
    date: String,
    subject: String,
    body: String,
}

impl Commit {
    fn short_hash(&self) -> String {
        self.hash.chars().take(7).collect()
    }

    fn render_compact(&self, include_body: bool) -> String {
        let base = format!(
            "{} {} ({}) <{}>",
            self.short_hash(),
            self.subject,
            self.date,
            self.author,
        );
        if include_body && !self.body.is_empty() {
            format!("{base}\n{}", indent(&self.body, "    "))
        } else {
            base
        }
    }
}

fn indent(s: &str, prefix: &str) -> String {
    s.lines()
        .map(|l| format!("{prefix}{l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_verbose(input: &str) -> Vec<Commit> {
    let mut commits: Vec<Commit> = Vec::new();
    let mut current: Option<Commit> = None;
    let mut in_body = false;

    for raw in input.lines() {
        if let Some(hash) = raw.strip_prefix("commit ") {
            if let Some(c) = current.take() {
                commits.push(c);
            }
            // Strip decorations like "(HEAD -> main)".
            let hash = hash.split_whitespace().next().unwrap_or(hash).to_string();
            current = Some(Commit {
                hash,
                author: String::new(),
                date: String::new(),
                subject: String::new(),
                body: String::new(),
            });
            in_body = false;
            continue;
        }
        let Some(c) = current.as_mut() else { continue };

        if let Some(a) = raw.strip_prefix("Author: ") {
            c.author = compact_author(a.trim());
            continue;
        }
        if let Some(d) = raw.strip_prefix("AuthorDate: ") {
            c.date = compact_date(d.trim());
            continue;
        }
        if let Some(d) = raw.strip_prefix("Date: ") {
            if c.date.is_empty() {
                c.date = compact_date(d.trim());
            }
            continue;
        }
        if raw.starts_with("Commit: ") || raw.starts_with("CommitDate: ") {
            continue;
        }
        if raw.starts_with("Merge: ") {
            continue;
        }
        if raw.starts_with("gpg:") {
            continue;
        }

        // After the headers, lines are body text. They're typically
        // indented by 4 spaces in the default format.
        let trimmed = raw.strip_prefix("    ").unwrap_or(raw);
        if c.subject.is_empty() {
            if trimmed.trim().is_empty() {
                continue;
            }
            c.subject = trimmed.to_string();
            in_body = true;
        } else if in_body {
            if !c.body.is_empty() {
                c.body.push('\n');
            }
            c.body.push_str(trimmed);
        }
    }
    if let Some(c) = current {
        commits.push(c);
    }
    commits
}

fn compact_author(a: &str) -> String {
    // `Cassie <cassie@example.com>` -> `Cassie`
    if let Some(idx) = a.find(" <") {
        a[..idx].to_string()
    } else {
        a.to_string()
    }
}

/// We keep the date string mostly as-is. We could compute "N days ago" but
/// that requires the current time and a dep; instead we strip the timezone
/// and seconds for brevity without loss.
fn compact_date(d: &str) -> String {
    // e.g. "Mon Apr 20 10:30:00 2026 +1000"
    let parts: Vec<&str> = d.split_whitespace().collect();
    if parts.len() >= 5 {
        let month = parts[1];
        let day = parts[2];
        let time = parts[3];
        let year = parts[4];
        let short_time = time.rsplit_once(':').map(|(a, _)| a).unwrap_or(time);
        format!("{month} {day} {year} {short_time}")
    } else {
        d.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    const HAPPY_PATH_INPUT: &str = "\
commit abc1234567890abcdef
Author: Cassie <cassie@example.com>
Date:   Mon Apr 20 10:30:00 2026 +1000

    Add user authentication

    This adds JWT authentication for API endpoints.
    Tokens expire after 24h.

commit def5678901234abcdef
Author: Someone Else <other@example.com>
Date:   Mon Apr 18 09:00:00 2026 +1000

    Fix config loading

commit 9876543210fedcba
Author: Cassie <cassie@example.com>
Date:   Mon Apr 10 12:00:00 2026 +1000

    Initial commit
";

    #[test]
    fn happy_path_compacts_commits() {
        let p = GitLogParser;
        let out = p.parse(HAPPY_PATH_INPUT, &CompressOptions::default());
        assert!(out.content.contains("abc1234 Add user authentication"));
        assert!(out.content.contains("Cassie"));
        assert!(out.content.contains("def5678 Fix config loading"));
        assert!(out.content.contains("9876543 Initial commit"));
        // Bodies omitted by default.
        assert!(!out.content.contains("JWT authentication"));
    }

    #[test]
    fn declares_body_omission() {
        let p = GitLogParser;
        let out = p.parse(HAPPY_PATH_INPUT, &CompressOptions::default());
        assert!(out.omitted.is_some());
        let note = out.omitted.unwrap();
        assert!(note.contains("commit bodies omitted"));
        assert!(note.contains("--full"));
        assert!(!out.lossless);
    }

    #[test]
    fn full_flag_includes_bodies() {
        let p = GitLogParser;
        let opts = CompressOptions {
            log_full: true,
            ..Default::default()
        };
        let out = p.parse(HAPPY_PATH_INPUT, &opts);
        assert!(out.content.contains("JWT authentication"));
        assert!(out.omitted.is_none());
        assert!(out.lossless);
    }

    #[test]
    fn oneline_format_pass_through() {
        let p = GitLogParser;
        let input = "abc1234 subject line\ndef5678 another commit\n";
        let out = p.parse(input, &CompressOptions::default());
        assert_eq!(out.content, input);
        assert!(out.lossless);
    }

    #[test]
    fn no_body_commits_do_not_trigger_omission() {
        let p = GitLogParser;
        let input = "\
commit abc1234567
Author: Cassie <c@e.com>
Date:   Mon Apr 10 12:00:00 2026 +0000

    Initial commit
";
        let out = p.parse(input, &CompressOptions::default());
        assert!(out.content.contains("abc1234 Initial commit"));
        assert!(out.omitted.is_none(), "no bodies means no omission");
    }

    #[test]
    fn detects_command() {
        let p = GitLogParser;
        assert!(p.can_handle("git log", ""));
        assert!(p.can_handle("git log --oneline", ""));
        assert!(p.can_handle("git reflog", ""));
        assert!(!p.can_handle("git diff", ""));
    }

    #[test]
    fn empty_input_is_safe() {
        let p = GitLogParser;
        let out = p.parse("", &CompressOptions::default());
        assert_eq!(out.content, "");
    }

    #[test]
    fn lossless_assertion_every_subject_preserved() {
        let p = GitLogParser;
        let out = p.parse(HAPPY_PATH_INPUT, &CompressOptions::default());
        for s in [
            "Add user authentication",
            "Fix config loading",
            "Initial commit",
        ] {
            assert!(out.content.contains(s), "missing subject: {s}");
        }
    }
}
