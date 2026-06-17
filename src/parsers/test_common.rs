//! Shared data types and rendering for test-runner parsers.
//!
//! Every test-runner parser builds a [`TestReport`] and renders it via
//! [`render`]. The report is a failure-first layout: assertion messages
//! always appear; passing tests collapse to per-file counts; stack traces
//! are gated behind the `--stack-traces` flag.

use crate::core::{CompressedOutput, Strategy, Verbosity};
use std::collections::BTreeMap;

#[derive(Debug, Default, Clone)]
pub struct TestReport {
    /// file or module -> passed count
    pub passed_per_file: BTreeMap<String, usize>,
    /// file or module -> total test count (passed + failed). Used to render
    /// "16/18" when a file has mixed results.
    pub total_per_file: BTreeMap<String, usize>,
    /// Failures in source order.
    pub failures: Vec<TestFailure>,
    pub total_passed: usize,
    pub total_failed: usize,
    pub total_skipped: usize,
    /// Free-form duration string, e.g. "4.2s".
    pub duration: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TestFailure {
    pub file: String,
    pub test: String,
    /// Assertion messages. Always preserved verbatim.
    pub messages: Vec<String>,
    /// Stack trace lines. Gated behind --stack-traces.
    pub stack: Vec<String>,
}

pub fn render(report: &TestReport, strategy: Strategy, stack_traces: bool, passing: Verbosity) -> CompressedOutput {
    let mut out = String::new();
    let summary = summary_line(report);
    out.push_str(&summary);
    out.push('\n');

    if !report.failures.is_empty() {
        out.push('\n');
        out.push_str("FAILED:\n");
        let mut by_file: BTreeMap<&str, Vec<&TestFailure>> = BTreeMap::new();
        for f in &report.failures {
            by_file.entry(f.file.as_str()).or_default().push(f);
        }
        for (file, fails) in by_file {
            out.push_str(&format!("  {file}\n"));
            for f in fails {
                out.push_str(&format!("    {}\n", f.test));
                for msg in &f.messages {
                    for line in msg.lines() {
                        out.push_str(&format!("      {line}\n"));
                    }
                }
                if stack_traces {
                    for s in &f.stack {
                        out.push_str(&format!("      {s}\n"));
                    }
                }
            }
        }
    }

    let stack_trace_present = report.failures.iter().any(|f| !f.stack.is_empty());
    let should_list_passing = !matches!(passing, Verbosity::Hidden)
        && report.total_passed > 0;
    if should_list_passing {
        out.push('\n');
        out.push_str("PASSED:\n");
        for (file, passed) in &report.passed_per_file {
            let total = report.total_per_file.get(file).copied().unwrap_or(*passed);
            match passing {
                Verbosity::Full => {
                    // "Full" at the report level still only shows counts here;
                    // individual test names aren't retained in the report.
                    // This is intentional: the lossy dimension is names, not
                    // presence, and we declare it when Collapsed.
                    if total == *passed {
                        out.push_str(&format!("  {file} ({passed})\n"));
                    } else {
                        out.push_str(&format!("  {file} ({passed}/{total})\n"));
                    }
                }
                Verbosity::Collapsed => {
                    if total == *passed {
                        out.push_str(&format!("  {file} ({passed})\n"));
                    } else {
                        out.push_str(&format!("  {file} ({passed}/{total})\n"));
                    }
                }
                Verbosity::Hidden => unreachable!(),
            }
        }
    }

    let mut omitted_notes: Vec<String> = Vec::new();
    if stack_trace_present && !stack_traces {
        omitted_notes.push("stack traces omitted - pass --stack-traces to include".to_string());
    }
    if matches!(passing, Verbosity::Collapsed) && report.total_passed > 0 {
        omitted_notes
            .push("individual passing test names omitted - pass --passing full to include".to_string());
    }
    if matches!(passing, Verbosity::Hidden) && report.total_passed > 0 {
        omitted_notes.push(
            "passing tests hidden - pass --passing collapsed|full to show".to_string(),
        );
    }

    let lossless = omitted_notes.is_empty();
    let omitted = if omitted_notes.is_empty() {
        None
    } else {
        Some(omitted_notes.join("; "))
    };

    CompressedOutput {
        content: out,
        strategy,
        omitted,
        lossless,
    }
}

fn summary_line(report: &TestReport) -> String {
    let mut parts: Vec<String> = Vec::new();
    if report.total_failed > 0 {
        parts.push(format!("{} failed", report.total_failed));
    }
    if report.total_passed > 0 {
        parts.push(format!("{} passed", report.total_passed));
    }
    if report.total_skipped > 0 {
        parts.push(format!("{} skipped", report.total_skipped));
    }
    if parts.is_empty() {
        parts.push("0 tests".to_string());
    }
    let mut s = parts.join(", ");
    if let Some(d) = &report.duration {
        s.push_str(&format!(" ({d})"));
    }
    s
}

/// Derive the file grouping key from a test identifier.
///
/// For "foo::bar::test_something" -> "foo::bar".
/// For "tests/test_auth.py::test_login" -> "tests/test_auth.py".
pub fn derive_group_key(full_path: &str) -> String {
    if let Some(idx) = full_path.rfind("::") {
        full_path[..idx].to_string()
    } else {
        full_path.to_string()
    }
}
