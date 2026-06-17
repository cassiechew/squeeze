//! End-to-end tests. Each test spawns the compiled `squeeze` binary and
//! pipes a fixture through it on stdin. The key assertion is *lossless*:
//! every semantically meaningful token from the input must appear in the
//! output.

use std::io::Write;
use std::process::{Command, Stdio};

fn run_squeeze(args: &[&str], stdin_text: &str) -> (String, String, i32) {
    let exe = env!("CARGO_BIN_EXE_squeeze");
    let mut child = Command::new(exe)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn squeeze");

    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin_text.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait");
    (
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
        output.status.code().unwrap_or(-1),
    )
}

#[test]
fn exit_code_is_mirrored() {
    let (_, _, code) = run_squeeze(&["--exit-code", "7", "--", "echo", "hi"], "hi\n");
    assert_eq!(code, 7);
}

#[test]
fn exit_code_default_is_zero() {
    let (_, _, code) = run_squeeze(&["--", "echo"], "hi\n");
    assert_eq!(code, 0);
}

#[test]
fn dry_run_reports_strategy() {
    let input = "\
On branch main
Changes to be committed:
\tmodified:   src/foo.rs
";
    let (stdout, _, _) = run_squeeze(&["--dry-run", "--", "git", "status"], input);
    assert!(stdout.contains("strategy: git-status"), "stdout: {stdout}");
}

#[test]
fn git_status_preserves_every_file() {
    let input = "\
On branch main
Changes to be committed:
\tmodified:   src/foo.rs
\tnew file:   src/bar.rs

Changes not staged for commit:
\tmodified:   src/baz.rs

Untracked files:
\t.env.local
";
    let (stdout, _, _) = run_squeeze(&["--", "git", "status"], input);
    for f in ["src/foo.rs", "src/bar.rs", "src/baz.rs", ".env.local"] {
        assert!(stdout.contains(f), "missing {f}; stdout:\n{stdout}");
    }
}

#[test]
fn git_diff_preserves_every_added_line() {
    let input = "\
diff --git a/src/main.rs b/src/main.rs
index abc..def 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!(\"one\");
+    println!(\"two\");
 }
";
    let (stdout, _, _) = run_squeeze(&["--", "git", "diff"], input);
    for l in ["+    println!(\"one\");", "+    println!(\"two\");"] {
        assert!(stdout.contains(l), "missing {l}; stdout:\n{stdout}");
    }
    // index line should be stripped.
    assert!(!stdout.contains("index abc..def"));
}

#[test]
fn cargo_test_preserves_failure_assertion() {
    let input = "\
running 2 tests
test foo::passing ... ok
test foo::failing ... FAILED

failures:

---- foo::failing stdout ----
thread 'foo::failing' panicked at src/foo.rs:10:5:
assertion `left == right` failed
  left: 42
 right: 7
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

failures:
    foo::failing

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
";
    let (stdout, _, _) = run_squeeze(&["--", "cargo", "test"], input);
    for s in [
        "foo::failing",
        "assertion `left == right` failed",
        "left: 42",
        "right: 7",
    ] {
        assert!(stdout.contains(s), "missing {s}; stdout:\n{stdout}");
    }
    // Declared omission for passing tests.
    assert!(stdout.contains("passing test names omitted"));
}

#[test]
fn stack_traces_flag_surfaces_frames() {
    let input = "\
running 1 tests
test foo::failing ... FAILED

failures:

---- foo::failing stdout ----
thread 'foo::failing' panicked at src/foo.rs:10:5:
oops
   0: frame_zero
   1: frame_one

failures:
    foo::failing

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
";
    let (stdout_no, _, _) = run_squeeze(&["--", "cargo", "test"], input);
    assert!(!stdout_no.contains("frame_zero"));
    let (stdout_yes, _, _) =
        run_squeeze(&["--stack-traces", "--", "cargo", "test"], input);
    assert!(stdout_yes.contains("frame_zero"));
    assert!(stdout_yes.contains("frame_one"));
}

#[test]
fn git_log_body_omission_is_declared() {
    let input = "\
commit abc1234567
Author: Cassie <c@e.com>
Date:   Mon Apr 20 10:30:00 2026 +0000

    Add user authentication

    This is the body content.
";
    let (stdout, _, _) = run_squeeze(&["--", "git", "log"], input);
    // Body omitted by default.
    assert!(!stdout.contains("This is the body content"));
    assert!(stdout.contains("commit bodies omitted"));

    let (stdout_full, _, _) = run_squeeze(&["--full", "--", "git", "log"], input);
    assert!(stdout_full.contains("This is the body content"));
}

#[test]
fn grep_preserves_every_match_line() {
    let input = "\
src/main.rs:12:pub fn alpha() {}
src/main.rs:14:pub fn beta() {}
src/main.rs:16:pub fn gamma() {}
src/lib.rs:3:pub fn delta() {}
";
    let (stdout, _, _) = run_squeeze(&["--", "grep", "-rn", "fn", "."], input);
    for s in ["alpha", "beta", "gamma", "delta"] {
        assert!(stdout.contains(s), "missing {s}; stdout:\n{stdout}");
    }
    assert!(stdout.contains("src/main.rs (3 matches):"));
    assert!(stdout.contains("src/lib.rs (1 matches):"));
}

#[test]
fn grep_never_caps_large_result_sets() {
    // 250 matches in one file. Every single one must appear.
    let mut input = String::new();
    for i in 1..=250 {
        input.push_str(&format!("src/big.rs:{i}:match-{i}\n"));
    }
    let (stdout, _, _) = run_squeeze(&["--", "grep"], &input);
    assert!(stdout.contains("(250 matches):"));
    assert!(stdout.contains("match-250"));
    assert!(stdout.contains("match-1\n") || stdout.contains("match-1 "));
}

#[test]
fn passthrough_strips_ansi_but_preserves_content() {
    let input = "\x1b[31merror\x1b[0m: something broke\nfollow up: fix it\n";
    let (stdout, _, _) = run_squeeze(&["--", "some-unknown-tool"], input);
    assert!(stdout.contains("error: something broke"));
    assert!(stdout.contains("follow up: fix it"));
    assert!(!stdout.contains("\x1b["));
}

#[test]
fn show_strategy_flag_adds_footer() {
    let input = "hello\n";
    let (stdout, _, _) = run_squeeze(&["--strategy", "--", "echo"], input);
    assert!(stdout.contains("strategy: passthrough"), "stdout: {stdout}");
}

#[test]
fn gh_pr_list_preserves_every_number() {
    let input = "\
Showing 3 of 3 pull requests in owner/repo

#42\tAdd auth\tfeature/auth\tOPEN\tabout 2 days ago
#41\tFix config\tbugfix/config\tMERGED\tabout 3 days ago
#40\tInitial\tinit\tCLOSED\tabout 1 week ago
";
    let (stdout, _, _) = run_squeeze(&["--", "gh", "pr", "list"], input);
    for n in ["#42", "#41", "#40", "Add auth", "Fix config", "Initial"] {
        assert!(stdout.contains(n), "missing {n}; stdout:\n{stdout}");
    }
}
