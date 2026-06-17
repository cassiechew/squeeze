//! Parser for `grep` / `ripgrep` / `ag` output.
//!
//! Groups matches by file. Per-line content is preserved verbatim up to
//! 200 chars, beyond which the line is truncated and the truncation is
//! declared inline.
//!
//! Result counts are never capped.

use crate::core::{CommandParser, CompressOptions, CompressedOutput, Strategy};
use crate::lossless::strip_ansi;
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::OnceLock;

pub struct GrepParser;

const MAX_LINE_LEN: usize = 200;

impl CommandParser for GrepParser {
    fn strategy(&self) -> Strategy {
        Strategy::Grep
    }

    fn can_handle(&self, command: &str, output: &str) -> bool {
        if is_grep_command(command) {
            return true;
        }
        looks_like_grep_output(output)
    }

    fn parse(&self, output: &str, _opts: &CompressOptions) -> CompressedOutput {
        let clean = strip_ansi(output);
        let (matches, mut truncated_any) = parse_matches(&clean);
        let mut out = String::new();
        for (file, entries) in &matches {
            out.push_str(&format!("{file} ({} matches):\n", entries.len()));
            for (line_no, content) in entries {
                let (display, was_truncated) = truncate_if_needed(content);
                if was_truncated {
                    truncated_any = true;
                }
                match line_no {
                    Some(n) => out.push_str(&format!("  line {n}: {display}\n")),
                    None => out.push_str(&format!("  {display}\n")),
                }
            }
            out.push('\n');
        }
        let (omitted, lossless) = if truncated_any {
            (
                Some(format!("one or more match lines truncated at {MAX_LINE_LEN} chars")),
                false,
            )
        } else {
            (None, true)
        };
        CompressedOutput {
            content: out,
            strategy: Strategy::Grep,
            omitted,
            lossless,
        }
    }
}

fn is_grep_command(command: &str) -> bool {
    let first = command.split_whitespace().next().unwrap_or("");
    matches!(first, "grep" | "egrep" | "fgrep" | "rg" | "ag" | "ack")
}

fn looks_like_grep_output(output: &str) -> bool {
    static SHAPE: OnceLock<Regex> = OnceLock::new();
    let shape = SHAPE.get_or_init(|| Regex::new(r"^[^\s:]+:\d+:").unwrap());
    let mut hits = 0;
    for l in output.lines().take(20) {
        if shape.is_match(l) {
            hits += 1;
            if hits >= 2 {
                return true;
            }
        }
    }
    false
}

type Matches = BTreeMap<String, Vec<(Option<usize>, String)>>;

fn parse_matches(input: &str) -> (Matches, bool) {
    static WITH_LINE: OnceLock<Regex> = OnceLock::new();
    let with_line = WITH_LINE.get_or_init(|| Regex::new(r"^([^\s:][^:]*):(\d+):(.*)$").unwrap());
    static WITHOUT_LINE: OnceLock<Regex> = OnceLock::new();
    let without_line = WITHOUT_LINE.get_or_init(|| Regex::new(r"^([^\s:][^:]*):(.*)$").unwrap());

    let mut matches: Matches = BTreeMap::new();
    let truncated_any = false;

    for raw in input.lines() {
        let line = raw.trim_end_matches('\r');
        if line.is_empty() || line == "--" {
            continue;
        }
        if let Some(caps) = with_line.captures(line) {
            let file = caps.get(1).unwrap().as_str().to_string();
            let n: usize = caps.get(2).unwrap().as_str().parse().unwrap_or(0);
            let content = caps.get(3).unwrap().as_str().to_string();
            matches.entry(file).or_default().push((Some(n), content));
            continue;
        }
        if let Some(caps) = without_line.captures(line) {
            let file = caps.get(1).unwrap().as_str().to_string();
            let content = caps.get(2).unwrap().as_str().to_string();
            matches.entry(file).or_default().push((None, content));
            continue;
        }
        // Unknown shape - drop into a catch-all bucket so no data is lost.
        matches
            .entry("(unknown)".to_string())
            .or_default()
            .push((None, line.to_string()));
    }
    (matches, truncated_any)
}

fn truncate_if_needed(content: &str) -> (String, bool) {
    if content.chars().count() <= MAX_LINE_LEN {
        return (content.to_string(), false);
    }
    let truncated: String = content.chars().take(MAX_LINE_LEN).collect();
    (
        format!("{truncated} (truncated at {MAX_LINE_LEN} chars)"),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const HAPPY_PATH: &str = "\
src/main.rs:12:pub fn authenticate(user: &str) -> bool {
src/main.rs:45:fn validate_token(token: &str) -> Result<Claims> {
src/main.rs:89:pub fn logout(session: &Session) -> Result<()> {
src/lib.rs:7:pub use auth::authenticate;
";

    #[test]
    fn happy_path_groups_by_file() {
        let p = GrepParser;
        let out = p.parse(HAPPY_PATH, &CompressOptions::default());
        assert!(out.content.contains("src/main.rs (3 matches):"));
        assert!(out.content.contains("line 12: pub fn authenticate"));
        assert!(out.content.contains("line 45: fn validate_token"));
        assert!(out.content.contains("line 89: pub fn logout"));
        assert!(out.content.contains("src/lib.rs (1 matches):"));
        assert!(out.content.contains("line 7: pub use auth::authenticate;"));
        assert!(out.lossless);
    }

    #[test]
    fn no_hard_cap_applied() {
        let p = GrepParser;
        let mut input = String::new();
        for i in 1..=500 {
            input.push_str(&format!("src/big.rs:{i}:line {i}\n"));
        }
        let out = p.parse(&input, &CompressOptions::default());
        assert!(out.content.contains("(500 matches):"));
        // Every line should be present.
        assert!(out.content.contains("line 250:"));
        assert!(out.content.contains("line 500:"));
    }

    #[test]
    fn long_line_is_truncated_and_declared() {
        let p = GrepParser;
        let long: String = "a".repeat(300);
        let input = format!("src/foo.rs:1:{long}\n");
        let out = p.parse(&input, &CompressOptions::default());
        assert!(out.content.contains(&format!("(truncated at {MAX_LINE_LEN} chars)")));
        assert!(out.omitted.is_some());
        assert!(!out.lossless);
    }

    #[test]
    fn empty_input_is_safe() {
        let p = GrepParser;
        let out = p.parse("", &CompressOptions::default());
        assert!(out.content.is_empty());
        assert!(out.lossless);
    }

    #[test]
    fn detects_command() {
        let p = GrepParser;
        assert!(p.can_handle("grep -rn foo src/", ""));
        assert!(p.can_handle("rg foo", ""));
        assert!(p.can_handle("ag --rust foo", ""));
    }

    #[test]
    fn detects_output_without_command() {
        let p = GrepParser;
        assert!(p.can_handle("", HAPPY_PATH));
    }

    #[test]
    fn lossless_every_file_and_line_preserved() {
        let p = GrepParser;
        let out = p.parse(HAPPY_PATH, &CompressOptions::default());
        for f in ["src/main.rs", "src/lib.rs"] {
            assert!(out.content.contains(f), "missing file: {f}");
        }
        for content in [
            "pub fn authenticate",
            "fn validate_token",
            "pub fn logout",
            "pub use auth::authenticate",
        ] {
            assert!(out.content.contains(content), "missing: {content}");
        }
    }
}
