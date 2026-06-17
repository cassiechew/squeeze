//! Parser for linter output. Group violations by rule, list all
//! file:line references under each. Never caps.
//!
//! Detection is command-based because linter text formats vary wildly.
//! Supported: ESLint (default stylish), Biome, eslint/biome JSON formats
//! are out of scope here (they'd warrant a separate parser).

use crate::core::{CommandParser, CompressOptions, CompressedOutput, Strategy};
use crate::lossless::strip_ansi;
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::OnceLock;

pub struct LintParser;

impl CommandParser for LintParser {
    fn strategy(&self) -> Strategy {
        Strategy::Lint
    }

    fn can_handle(&self, command: &str, output: &str) -> bool {
        if is_lint_command(command) {
            return true;
        }
        looks_like_eslint_stylish(output)
    }

    fn parse(&self, output: &str, _opts: &CompressOptions) -> CompressedOutput {
        let clean = strip_ansi(output);
        let violations = parse_violations(&clean);
        let mut by_rule: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for v in &violations {
            by_rule
                .entry(v.rule.clone())
                .or_default()
                .push(format!("{}:{}", v.file, v.line));
        }
        let total: usize = violations.len();
        let file_count = violations
            .iter()
            .map(|v| v.file.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len();

        let mut out = String::new();
        if total > 0 {
            out.push_str(&format!(
                "{} violation{} across {} file{}\n\n",
                total,
                if total == 1 { "" } else { "s" },
                file_count,
                if file_count == 1 { "" } else { "s" },
            ));
        }
        for (rule, refs) in &by_rule {
            out.push_str(&format!("{rule} ({}):\n", refs.len()));
            for r in refs {
                out.push_str(&format!("  {r}\n"));
            }
            out.push('\n');
        }
        CompressedOutput {
            content: out,
            strategy: Strategy::Lint,
            omitted: None,
            lossless: true,
        }
    }
}

fn is_lint_command(command: &str) -> bool {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    for t in &tokens {
        if matches!(*t, "eslint" | "biome" | "ruff") {
            return true;
        }
    }
    // `cargo clippy` and `npm run lint` are handled via command text sniffing.
    let joined = command.to_ascii_lowercase();
    joined.contains("clippy") || joined.contains(" lint") || joined.starts_with("lint")
}

fn looks_like_eslint_stylish(output: &str) -> bool {
    // The stylish format has blocks like:
    // /abs/path/file.tsx
    //   12:5  error   Missing semicolon  semi
    static RULE_LINE: OnceLock<Regex> = OnceLock::new();
    let rule_line =
        RULE_LINE.get_or_init(|| Regex::new(r"^\s+\d+:\d+\s+(error|warning)\s+").unwrap());
    let mut hits = 0;
    for l in output.lines().take(20) {
        if rule_line.is_match(l) {
            hits += 1;
            if hits >= 2 {
                return true;
            }
        }
    }
    false
}

struct Violation {
    file: String,
    line: usize,
    rule: String,
}

fn parse_violations(input: &str) -> Vec<Violation> {
    let mut current_file: Option<String> = None;
    let mut out: Vec<Violation> = Vec::new();

    static RULE_LINE: OnceLock<Regex> = OnceLock::new();
    let rule_line = RULE_LINE.get_or_init(|| {
        // 12:5  error  Missing semicolon  semi
        Regex::new(r"^\s*(\d+):\d+\s+(?:error|warning)\s+.*?\s{2,}(\S+)\s*$").unwrap()
    });
    static SUMMARY_LINE: OnceLock<Regex> = OnceLock::new();
    let summary = SUMMARY_LINE.get_or_init(|| {
        Regex::new(r"problems?\s+\(\d+\s+errors?,\s+\d+\s+warnings?\)").unwrap()
    });

    for raw in input.lines() {
        let line = raw.trim_end();

        if line.trim().is_empty() {
            continue;
        }
        if summary.is_match(line) {
            continue;
        }

        if let Some(caps) = rule_line.captures(line) {
            let ln: usize = caps.get(1).unwrap().as_str().parse().unwrap_or(0);
            let rule = caps.get(2).unwrap().as_str().to_string();
            if let Some(file) = &current_file {
                out.push(Violation {
                    file: file.clone(),
                    line: ln,
                    rule,
                });
            }
            continue;
        }

        // A line that doesn't match the rule shape and isn't indented is
        // likely a file header.
        if !line.starts_with(' ') && !line.starts_with('\t') {
            current_file = Some(line.to_string());
            continue;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const HAPPY_PATH: &str = "\
/abs/src/components/Button.tsx
  12:5   error    Missing semicolon                    semi
  24:3   error    'foo' is defined but never used      no-unused-vars

/abs/src/components/Card.tsx
  8:10   error    Missing semicolon                    semi

3 problems (3 errors, 0 warnings)
";

    #[test]
    fn happy_path_groups_by_rule() {
        let p = LintParser;
        let out = p.parse(HAPPY_PATH, &CompressOptions::default());
        assert!(out.content.contains("3 violations across 2 files"));
        assert!(out.content.contains("semi (2):"));
        assert!(out.content.contains("/abs/src/components/Button.tsx:12"));
        assert!(out.content.contains("/abs/src/components/Card.tsx:8"));
        assert!(out.content.contains("no-unused-vars (1):"));
        assert!(out.content.contains("/abs/src/components/Button.tsx:24"));
        assert!(out.lossless);
    }

    #[test]
    fn no_cap_applied() {
        let p = LintParser;
        let mut input = String::new();
        input.push_str("/abs/src/foo.ts\n");
        for i in 1..=100 {
            input.push_str(&format!("  {i}:1   error   X   rule-x\n"));
        }
        let out = p.parse(&input, &CompressOptions::default());
        assert!(out.content.contains("rule-x (100):"));
        assert!(out.content.contains("/abs/src/foo.ts:100"));
    }

    #[test]
    fn empty_input_safe() {
        let p = LintParser;
        let out = p.parse("", &CompressOptions::default());
        assert!(out.lossless);
    }

    #[test]
    fn detects_command() {
        let p = LintParser;
        assert!(p.can_handle("eslint src/", ""));
        assert!(p.can_handle("npx eslint --fix", ""));
        assert!(p.can_handle("biome check", ""));
        assert!(p.can_handle("cargo clippy", ""));
        assert!(p.can_handle("npm run lint", ""));
    }
}
