//! Parser for `git diff` output. Emits a per-file stat header then the
//! full patch. ANSI is stripped and `index abc..def` lines are dropped
//! (they carry no information the agent needs), but hunks, context lines,
//! and additions/deletions are all preserved unconditionally.

use crate::core::{CommandParser, CompressOptions, CompressedOutput, Strategy};
use crate::lossless::strip_ansi;

pub struct GitDiffParser;

impl CommandParser for GitDiffParser {
    fn strategy(&self) -> Strategy {
        Strategy::GitDiff
    }

    fn can_handle(&self, command: &str, output: &str) -> bool {
        if command_is_git_diff(command) {
            return true;
        }
        output.contains("diff --git ")
    }

    fn parse(&self, output: &str, _opts: &CompressOptions) -> CompressedOutput {
        let clean = strip_ansi(output);
        let files = parse_diff(&clean);
        let content = render(&files);
        CompressedOutput {
            content,
            strategy: Strategy::GitDiff,
            omitted: None,
            lossless: true,
        }
    }
}

fn command_is_git_diff(command: &str) -> bool {
    let mut saw_git = false;
    for t in command.split_whitespace() {
        if t == "git" {
            saw_git = true;
        } else if saw_git && (t == "diff" || t == "show") {
            return true;
        }
    }
    false
}

struct FileDiff {
    path: String,
    additions: usize,
    deletions: usize,
    body: String,
}

fn parse_diff(input: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut current: Option<FileDiff> = None;
    let mut in_hunk = false;

    for line in input.lines() {
        if let Some(header_path) = line.strip_prefix("diff --git ") {
            if let Some(f) = current.take() {
                files.push(f);
            }
            let path = extract_path(header_path);
            current = Some(FileDiff {
                path,
                additions: 0,
                deletions: 0,
                body: String::new(),
            });
            in_hunk = false;
            continue;
        }

        let Some(f) = current.as_mut() else {
            continue;
        };

        // Drop noise lines that carry no semantic value.
        if line.starts_with("index ") && !in_hunk {
            continue;
        }
        if (line.starts_with("--- ") || line.starts_with("+++ ")) && !in_hunk {
            // Preserve them — they confirm the filename and +/- sides.
            f.body.push_str(line);
            f.body.push('\n');
            continue;
        }
        if line.starts_with("new file mode")
            || line.starts_with("deleted file mode")
            || line.starts_with("old mode")
            || line.starts_with("new mode")
            || line.starts_with("similarity index")
            || line.starts_with("rename from")
            || line.starts_with("rename to")
            || line.starts_with("copy from")
            || line.starts_with("copy to")
            || line.starts_with("Binary files ")
        {
            f.body.push_str(line);
            f.body.push('\n');
            continue;
        }
        if line.starts_with("@@") {
            in_hunk = true;
            f.body.push_str(line);
            f.body.push('\n');
            continue;
        }
        if in_hunk {
            if line.starts_with('+') && !line.starts_with("+++") {
                f.additions += 1;
            } else if line.starts_with('-') && !line.starts_with("---") {
                f.deletions += 1;
            }
            f.body.push_str(line);
            f.body.push('\n');
        }
    }
    if let Some(f) = current {
        files.push(f);
    }
    files
}

/// `diff --git a/foo b/foo` -> `foo`.
fn extract_path(header: &str) -> String {
    let parts: Vec<&str> = header.split_whitespace().collect();
    if let Some(a) = parts.first() {
        if let Some(stripped) = a.strip_prefix("a/") {
            return stripped.to_string();
        }
    }
    header.trim().to_string()
}

fn render(files: &[FileDiff]) -> String {
    if files.is_empty() {
        return String::new();
    }
    let total_add: usize = files.iter().map(|f| f.additions).sum();
    let total_del: usize = files.iter().map(|f| f.deletions).sum();

    let mut out = String::new();
    out.push_str(&format!(
        "{} file{} changed, +{} -{}\n\n",
        files.len(),
        if files.len() == 1 { "" } else { "s" },
        total_add,
        total_del
    ));

    let name_width = files.iter().map(|f| f.path.len()).max().unwrap_or(0);
    for f in files {
        out.push_str(&format!(
            "{:<width$} | +{} -{}\n",
            f.path,
            f.additions,
            f.deletions,
            width = name_width
        ));
    }
    out.push('\n');

    for f in files {
        out.push_str(&format!("--- {} ---\n", f.path));
        out.push_str(&f.body);
        if !f.body.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const HAPPY_PATH_INPUT: &str = "\
diff --git a/src/main.rs b/src/main.rs
index abc123..def456 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!(\"new feature\");
     println!(\"hello\");
 }
diff --git a/src/lib.rs b/src/lib.rs
index 111..222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -5,2 +5,3 @@
 pub fn x() {}
+pub fn y() {}
 pub fn z() {}
";

    #[test]
    fn happy_path_emits_stat_header_and_patch() {
        let p = GitDiffParser;
        let out = p.parse(HAPPY_PATH_INPUT, &CompressOptions::default());
        assert!(out.content.contains("2 files changed, +2 -0"));
        assert!(out.content.contains("src/main.rs | +1 -0"));
        assert!(out.content.contains("src/lib.rs  | +1 -0"));
        // Patch body preserved.
        assert!(out.content.contains("+    println!(\"new feature\");"));
        assert!(out.content.contains("+pub fn y() {}"));
        assert!(out.lossless);
    }

    #[test]
    fn strips_index_lines() {
        let p = GitDiffParser;
        let out = p.parse(HAPPY_PATH_INPUT, &CompressOptions::default());
        assert!(!out.content.contains("index abc123"));
        assert!(!out.content.contains("index 111"));
    }

    #[test]
    fn preserves_hunk_headers() {
        let p = GitDiffParser;
        let out = p.parse(HAPPY_PATH_INPUT, &CompressOptions::default());
        assert!(out.content.contains("@@ -1,3 +1,4 @@"));
        assert!(out.content.contains("@@ -5,2 +5,3 @@"));
    }

    #[test]
    fn empty_input_yields_empty_output() {
        let p = GitDiffParser;
        let out = p.parse("", &CompressOptions::default());
        assert_eq!(out.content, "");
        assert!(out.lossless);
    }

    #[test]
    fn binary_diff_preserved() {
        let p = GitDiffParser;
        let input = "\
diff --git a/img.png b/img.png
index abc..def
Binary files a/img.png and b/img.png differ
";
        let out = p.parse(input, &CompressOptions::default());
        assert!(out.content.contains("img.png"));
        assert!(out.content.contains("Binary files"));
    }

    #[test]
    fn detects_command() {
        let p = GitDiffParser;
        assert!(p.can_handle("git diff", ""));
        assert!(p.can_handle("git diff HEAD~1", ""));
        assert!(p.can_handle("git -c color.ui=always diff", ""));
        assert!(p.can_handle("git show HEAD", ""));
    }

    #[test]
    fn lossless_assertion_every_added_line_present() {
        let p = GitDiffParser;
        let out = p.parse(HAPPY_PATH_INPUT, &CompressOptions::default());
        for l in [
            "+    println!(\"new feature\");",
            "+pub fn y() {}",
            " pub fn z() {}",
        ] {
            assert!(out.content.contains(l), "missing: {l}");
        }
    }
}
