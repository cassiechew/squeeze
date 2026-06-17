//! Core types: options, compressed output, parser trait.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verbosity {
    Full,
    Collapsed,
    Hidden,
}

impl Default for Verbosity {
    fn default() -> Self {
        Verbosity::Collapsed
    }
}

impl std::str::FromStr for Verbosity {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "full" => Ok(Verbosity::Full),
            "collapsed" => Ok(Verbosity::Collapsed),
            "hidden" => Ok(Verbosity::Hidden),
            other => Err(format!(
                "invalid verbosity '{other}' (expected full|collapsed|hidden)"
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompressOptions {
    /// Include stack traces in test output. Default false (declared omission).
    pub stack_traces: bool,
    /// Verbosity for passing tests. Default Collapsed (counts only).
    pub passing_tests: Verbosity,
    /// Never truncate diff content. Default true.
    pub diff_full: bool,
    /// Hard cap on grep results. Default None (never cap).
    pub max_grep_results: Option<usize>,
    /// Include full commit bodies in git log. Default false (declared).
    pub log_full: bool,
}

impl Default for CompressOptions {
    fn default() -> Self {
        Self {
            stack_traces: false,
            passing_tests: Verbosity::Collapsed,
            diff_full: true,
            max_grep_results: None,
            log_full: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    GitStatus,
    GitDiff,
    GitLog,
    CargoTest,
    GoTest,
    Vitest,
    Jest,
    Pytest,
    GhPrView,
    GhPrList,
    GhIssueView,
    GhIssueList,
    Grep,
    Lint,
    Passthrough,
}

impl fmt::Display for Strategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Strategy::GitStatus => "git-status",
            Strategy::GitDiff => "git-diff",
            Strategy::GitLog => "git-log",
            Strategy::CargoTest => "cargo-test",
            Strategy::GoTest => "go-test",
            Strategy::Vitest => "vitest",
            Strategy::Jest => "jest",
            Strategy::Pytest => "pytest",
            Strategy::GhPrView => "gh-pr-view",
            Strategy::GhPrList => "gh-pr-list",
            Strategy::GhIssueView => "gh-issue-view",
            Strategy::GhIssueList => "gh-issue-list",
            Strategy::Grep => "grep",
            Strategy::Lint => "lint",
            Strategy::Passthrough => "passthrough",
        };
        f.write_str(s)
    }
}

/// The result of compressing a command's output.
#[derive(Debug, Clone)]
pub struct CompressedOutput {
    pub content: String,
    pub strategy: Strategy,
    /// Human-readable description of what was omitted and the flag to restore
    /// it. `None` means nothing was omitted.
    pub omitted: Option<String>,
    /// Whether the output is lossless. Must be true for default behaviours.
    pub lossless: bool,
}

impl CompressedOutput {
    /// Render the content with any declared-omission footer appended.
    pub fn rendered(&self) -> String {
        match &self.omitted {
            Some(note) => {
                let mut s = self.content.clone();
                if !s.ends_with('\n') {
                    s.push('\n');
                }
                s.push('\n');
                s.push('(');
                s.push_str(note);
                s.push(')');
                s.push('\n');
                s
            }
            None => {
                let mut s = self.content.clone();
                if !s.ends_with('\n') {
                    s.push('\n');
                }
                s
            }
        }
    }
}

/// A parser for a specific command family.
pub trait CommandParser: Send + Sync {
    /// Declared name of the strategy (used for diagnostics).
    fn strategy(&self) -> Strategy;

    /// Return true if this parser should handle the given command + output.
    fn can_handle(&self, command: &str, output: &str) -> bool;

    /// Transform the raw output into a [`CompressedOutput`].
    fn parse(&self, output: &str, opts: &CompressOptions) -> CompressedOutput;
}
