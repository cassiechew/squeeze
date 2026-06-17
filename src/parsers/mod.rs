//! Command-family parsers. Each module implements one or more
//! [`crate::core::CommandParser`] types.

pub mod cargo_test;
pub mod gh_issue;
pub mod gh_pr;
pub mod git_diff;
pub mod git_log;
pub mod git_status;
pub mod go_test;
pub mod grep;
pub mod jest;
pub mod lint;
pub mod passthrough;
pub mod pytest;
pub mod test_common;
pub mod vitest;

use crate::core::CommandParser;

/// Return every parser in dispatch order. Specific parsers come first;
/// the passthrough fallback is last.
pub fn all() -> Vec<Box<dyn CommandParser>> {
    vec![
        Box::new(git_status::GitStatusParser),
        Box::new(git_diff::GitDiffParser),
        Box::new(git_log::GitLogParser),
        Box::new(cargo_test::CargoTestParser),
        Box::new(go_test::GoTestParser),
        Box::new(pytest::PytestParser),
        Box::new(vitest::VitestParser),
        Box::new(jest::JestParser),
        Box::new(gh_pr::GhPrParser),
        Box::new(gh_issue::GhIssueParser),
        Box::new(lint::LintParser),
        Box::new(grep::GrepParser),
        Box::new(passthrough::AnsiStripPassthrough),
    ]
}
