//! Static Makefile parser for discovering runnable targets, `.PHONY` rules,
//! descriptions, and workspace-contained `include` files.
//!
//! Parsing is purely static (never executes `$(shell ...)` or any recipe), so
//! loading the build model is safe, deterministic, and bounded.

use crate::paths::Instance;
use lithe_plugin_sdk::PluginError;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Maximum recursion depth for `include` / `-include` / `sinclude`.
const MAX_INCLUDE_DEPTH: usize = 8;
/// Maximum number of Makefile files read in a single model load.
const MAX_FILES: usize = 64;
/// Maximum bytes read from any single Makefile (4 MiB).
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// Maximum variable expansion iterations to prevent self-referential loops.
const MAX_EXPAND_PASSES: usize = 4;

/// Parsed Makefile target ready for the Lithe build model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MakeTarget {
    pub name: String,
    pub phony: bool,
    pub is_default: bool,
    pub kind: TargetKind,
    pub source_file: String,
    pub line: usize,
    pub description: Option<String>,
}

/// Classification used to tag targets for Run Configurations (`target` vs `internal`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// Public runnable Makefile target exposed as a generated Makefile Target run configuration.
    Target,
    /// Leading-underscore helper targets omitted from auto-generated Run Configurations.
    Internal,
}

impl TargetKind {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Target => "target",
            Self::Internal => "internal",
        }
    }
}

/// Complete parsed result for one Makefile instance.
#[derive(Debug, Clone)]
pub struct ParsedMakefile {
    pub entry_file: String,
    pub targets: Vec<MakeTarget>,
    /// Number of `include` directives that were skipped (outside workspace,
    /// unreadable, or hitting depth/file limits).
    pub skipped_includes: usize,
}

/// Parses the active Makefile of `instance`.
pub fn parse_instance(
    instance: &Instance,
    explicit_file: Option<&str>,
) -> Result<ParsedMakefile, PluginError> {
    let entry_path = instance.makefile(explicit_file)?;
    let entry_file = instance
        .relative(&entry_path)
        .unwrap_or_else(|| "Makefile".to_string());

    let mut state = ParseState::default();
    state.parse_file(instance, &entry_path, 0)?;

    let default_goal = state
        .variables
        .get(".DEFAULT_GOAL")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let mut targets = Vec::new();
    let mut first_public_idx: Option<usize> = None;

    for raw in state.targets {
        let phony = state.phony.contains(&raw.name);
        let kind = classify_target(&raw.name);
        if first_public_idx.is_none() && kind != TargetKind::Internal {
            first_public_idx = Some(targets.len());
        }
        targets.push(MakeTarget {
            name: raw.name,
            phony,
            is_default: false,
            kind,
            source_file: raw.source_file,
            line: raw.line,
            description: raw.description,
        });
    }

    if let Some(goal) = default_goal {
        if let Some(target) = targets.iter_mut().find(|t| t.name == goal) {
            target.is_default = true;
        }
    } else if let Some(idx) = first_public_idx {
        targets[idx].is_default = true;
    }

    Ok(ParsedMakefile {
        entry_file,
        targets,
        skipped_includes: state.skipped_includes,
    })
}

#[derive(Debug, Clone)]
struct RawTarget {
    name: String,
    source_file: String,
    line: usize,
    description: Option<String>,
}

#[derive(Default)]
struct ParseState {
    visited: BTreeSet<PathBuf>,
    variables: BTreeMap<String, String>,
    phony: BTreeSet<String>,
    targets: Vec<RawTarget>,
    seen_targets: BTreeMap<String, usize>,
    skipped_includes: usize,
}

impl ParseState {
    fn parse_file(
        &mut self,
        instance: &Instance,
        path: &Path,
        depth: usize,
    ) -> Result<(), PluginError> {
        if depth > MAX_INCLUDE_DEPTH || self.visited.len() >= MAX_FILES {
            self.skipped_includes += 1;
            return Ok(());
        }
        if !self.visited.insert(path.to_path_buf()) {
            return Ok(());
        }

        let metadata = std::fs::metadata(path).map_err(|_| {
            PluginError::new(
                "makefile.readFailed",
                "Could not read the Makefile metadata.",
            )
        })?;
        if metadata.len() > MAX_FILE_BYTES {
            if depth == 0 {
                return Err(PluginError::new(
                    "makefile.tooLarge",
                    "The Makefile exceeds the 4 MiB static parsing limit.",
                ));
            }
            self.skipped_includes += 1;
            return Ok(());
        }

        let content = std::fs::read_to_string(path).map_err(|_| {
            PluginError::new(
                "makefile.readFailed",
                "Could not read the Makefile as UTF-8 text.",
            )
        })?;

        let source_file = instance
            .relative(path)
            .unwrap_or_else(|| "Makefile".to_string());

        let logical_lines = join_continuations(&content);
        let mut recipe_prefix = '\t';
        let mut in_define = false;
        let mut pending_comment: Option<String> = None;

        for (line_no, raw_line) in logical_lines {
            // Skip recipe lines.
            if raw_line.starts_with(recipe_prefix) {
                pending_comment = None;
                continue;
            }

            let trimmed = raw_line.trim();
            if trimmed.is_empty() {
                pending_comment = None;
                continue;
            }

            // Track multi-line `define ... endef` blocks.
            if in_define {
                if trimmed == "endef" || trimmed.starts_with("endef ") || trimmed.starts_with("endef\t") {
                    in_define = false;
                }
                pending_comment = None;
                continue;
            }
            if is_define_start(trimmed) {
                in_define = true;
                pending_comment = None;
                continue;
            }

            // Pure comment line: capture as potential doc comment for the next rule.
            if let Some(comment) = trimmed.strip_prefix('#') {
                let text = comment.trim_start_matches('#').trim();
                if !text.is_empty() && !text.starts_with('!') {
                    pending_comment = Some(text.to_string());
                } else {
                    pending_comment = None;
                }
                continue;
            }

            // Conditional directives (`ifeq`, `ifneq`, `ifdef`, `ifndef`, `else`, `endif`)
            // are skipped without clearing targets inside either branch so both branches'
            // targets can still be discovered statically.
            if is_conditional_directive(trimmed) {
                pending_comment = None;
                continue;
            }

            // Handle `include`, `-include`, `sinclude`.
            if let Some(rest) = strip_include_directive(trimmed) {
                pending_comment = None;
                let expanded = self.expand_vars(strip_inline_comment(rest).trim());
                for token in expanded.split_whitespace() {
                    if token.contains('*') || token.contains('?') || token.contains('[') {
                        // Simple glob expansion is skipped cleanly in static mode.
                        self.skipped_includes += 1;
                        continue;
                    }
                    let Some(resolved) = instance.resolve_inside_workspace(token) else {
                        self.skipped_includes += 1;
                        continue;
                    };
                    if resolved.is_file() {
                        let _ = self.parse_file(instance, &resolved, depth + 1);
                    } else {
                        self.skipped_includes += 1;
                    }
                }
                continue;
            }

            // Check variable assignment before rule parsing so `FOO := bar:baz` is
            // never mistaken for a target rule.
            if let Some((name, op, value)) = parse_variable_assignment(trimmed) {
                pending_comment = None;
                if name == ".RECIPEPREFIX" {
                    if let Some(ch) = value.chars().next() {
                        recipe_prefix = ch;
                    }
                }
                let cleaned_value = strip_inline_comment(value).trim().to_string();
                match op {
                    "?=" => {
                        self.variables
                            .entry(name.to_string())
                            .or_insert(cleaned_value);
                    }
                    "+=" => {
                        self.variables
                            .entry(name.to_string())
                            .and_modify(|existing| {
                                if !existing.is_empty() {
                                    existing.push(' ');
                                }
                                existing.push_str(&cleaned_value);
                            })
                            .or_insert(cleaned_value);
                    }
                    _ => {
                        self.variables.insert(name.to_string(), cleaned_value);
                    }
                }
                continue;
            }

            // Check rule line: `targets: prerequisites [# comment]`
            if let Some((lhs, rhs)) = split_rule_line(trimmed) {
                let inline_doc = extract_inline_doc(rhs);
                let doc = inline_doc.or_else(|| pending_comment.take());

                // Check for `.PHONY: target1 target2 ...`
                if lhs.trim() == ".PHONY" {
                    let expanded_rhs = self.expand_vars(strip_inline_comment(rhs));
                    for name in expanded_rhs.split_whitespace() {
                        if is_valid_target_name(name) {
                            self.phony.insert(name.to_string());
                        }
                    }
                    continue;
                }

                // Skip target-specific variable assignments like `target: VAR = val`.
                let rhs_no_comment = strip_inline_comment(rhs).trim();
                if parse_variable_assignment(rhs_no_comment).is_some() {
                    continue;
                }

                // Skip static pattern rules (`targets: pattern: prereqs`).
                if rhs_no_comment.contains(':') && !rhs_no_comment.starts_with(':') {
                    if let Some((mid, _)) = split_rule_line(rhs_no_comment) {
                        if mid.contains('%') {
                            continue;
                        }
                    }
                }

                let expanded_lhs = self.expand_vars(lhs);
                for token in expanded_lhs.split_whitespace() {
                    if !is_valid_target_name(token) {
                        continue;
                    }
                    if let Some(&existing_idx) = self.seen_targets.get(token) {
                        if self.targets[existing_idx].description.is_none() && doc.is_some() {
                            self.targets[existing_idx].description = doc.clone();
                        }
                    } else {
                        let idx = self.targets.len();
                        self.seen_targets.insert(token.to_string(), idx);
                        self.targets.push(RawTarget {
                            name: token.to_string(),
                            source_file: source_file.clone(),
                            line: line_no,
                            description: doc.clone(),
                        });
                    }
                }
                continue;
            }

            pending_comment = None;
        }

        Ok(())
    }

    /// Expands simple `$(VAR)` and `${VAR}` references using known static variables.
    fn expand_vars(&self, input: &str) -> String {
        let mut current = input.to_string();
        for _ in 0..MAX_EXPAND_PASSES {
            if !current.contains("$(") && !current.contains("${") {
                break;
            }
            let next = expand_once(&current, &self.variables);
            if next == current {
                break;
            }
            current = next;
        }
        current
    }
}

/// Joins backslash-continued lines while preserving the 1-based starting line number.
fn join_continuations(content: &str) -> Vec<(usize, String)> {
    let mut result = Vec::new();
    let mut lines = content.lines().enumerate().peekable();
    while let Some((idx, line)) = lines.next() {
        let start_line = idx + 1;
        let line = line.strip_suffix('\r').unwrap_or(line);
        // Never merge lines inside a recipe line (starts with tab).
        if line.starts_with('\t') {
            result.push((start_line, line.to_string()));
            continue;
        }
        let mut combined = line.to_string();
        while ends_with_unescaped_backslash(&combined) {
            combined.pop();
            if let Some((_, next_line)) = lines.next() {
                let next_line = next_line.strip_suffix('\r').unwrap_or(next_line);
                combined.push(' ');
                combined.push_str(next_line.trim_start());
            } else {
                break;
            }
        }
        result.push((start_line, combined));
    }
    result
}

fn ends_with_unescaped_backslash(s: &str) -> bool {
    let count = s.bytes().rev().take_while(|&b| b == b'\\').count();
    count % 2 == 1
}

fn is_define_start(trimmed: &str) -> bool {
    let rest = trimmed
        .strip_prefix("override ")
        .or_else(|| trimmed.strip_prefix("export "))
        .unwrap_or(trimmed);
    rest.starts_with("define ") || rest.starts_with("define\t")
}

fn is_conditional_directive(trimmed: &str) -> bool {
    let first = trimmed.split_whitespace().next().unwrap_or("");
    matches!(
        first,
        "ifeq" | "ifneq" | "ifdef" | "ifndef" | "else" | "endif" | "undefine" | "vpath" | "unexport"
    )
}

fn strip_include_directive(trimmed: &str) -> Option<&str> {
    for prefix in ["include ", "include\t", "-include ", "-include\t", "sinclude ", "sinclude\t"] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return Some(rest);
        }
    }
    None
}

/// Parses a variable assignment line like `FOO := bar`, `export FOO ?= baz`.
fn parse_variable_assignment(trimmed: &str) -> Option<(&str, &str, &str)> {
    let body = trimmed
        .strip_prefix("export ")
        .or_else(|| trimmed.strip_prefix("override "))
        .or_else(|| trimmed.strip_prefix("private "))
        .unwrap_or(trimmed)
        .trim_start();

    let bytes = body.as_bytes();
    let mut depth = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'$' if i + 1 < bytes.len() && (bytes[i + 1] == b'(' || bytes[i + 1] == b'{') => {
                depth += 1;
                i += 2;
                continue;
            }
            b')' | b'}' if depth > 0 => {
                depth -= 1;
            }
            b'#' if depth == 0 => return None,
            b':' if depth == 0 => {
                if body[i..].starts_with("::=") {
                    let name = body[..i].trim();
                    return is_var_name(name).then_some((name, "::=", &body[i + 3..]));
                }
                if body[i..].starts_with(":=") {
                    let name = body[..i].trim();
                    return is_var_name(name).then_some((name, ":=", &body[i + 2..]));
                }
                // A plain `:` before any `=` means this is a rule, not a variable assignment.
                return None;
            }
            b'?' | b'+' | b'!' if depth == 0 && i + 1 < bytes.len() && bytes[i + 1] == b'=' => {
                let name = body[..i].trim();
                let op = &body[i..i + 2];
                return is_var_name(name).then_some((name, op, &body[i + 2..]));
            }
            b'=' if depth == 0 => {
                let name = body[..i].trim();
                return is_var_name(name).then_some((name, "=", &body[i + 1..]));
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn is_var_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(char::is_whitespace)
        && !name.contains(':')
        && !name.contains('#')
}

/// Splits a Makefile rule line at the first top-level `:` (or `::`).
fn split_rule_line(trimmed: &str) -> Option<(&str, &str)> {
    let bytes = trimmed.as_bytes();
    let mut depth = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'$' if i + 1 < bytes.len() && (bytes[i + 1] == b'(' || bytes[i + 1] == b'{') => {
                depth += 1;
                i += 2;
                continue;
            }
            b')' | b'}' if depth > 0 => {
                depth -= 1;
            }
            b'#' if depth == 0 => return None,
            b':' if depth == 0 => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'=' {
                    return None;
                }
                let rhs_start = if i + 1 < bytes.len() && bytes[i + 1] == b':' {
                    i + 2
                } else {
                    i + 1
                };
                return Some((&trimmed[..i], &trimmed[rhs_start..]));
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn strip_inline_comment(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' && (i == 0 || bytes[i - 1] != b'\\') {
            return &s[..i];
        }
        i += 1;
    }
    s
}

/// Extracts an inline `## description` (or `# description`) comment from a rule's RHS.
fn extract_inline_doc(rhs: &str) -> Option<String> {
    let bytes = rhs.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' && (i == 0 || bytes[i - 1] != b'\\') {
            let comment = rhs[i..].trim_start_matches('#').trim();
            if !comment.is_empty() {
                return Some(comment.to_string());
            }
            return None;
        }
        i += 1;
    }
    None
}

fn expand_once(input: &str, vars: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && (bytes[i + 1] == b'(' || bytes[i + 1] == b'{')
        {
            let close = if bytes[i + 1] == b'(' { b')' } else { b'}' };
            if let Some(end) = bytes[i + 2..].iter().position(|&b| b == close) {
                let name = &input[i + 2..i + 2 + end];
                if let Some(val) = vars.get(name) {
                    out.push_str(val);
                    i = i + 3 + end;
                    continue;
                }
            }
        }
        let ch = input[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Rejects special dot targets, pattern rules, unexpanded expressions, and
/// non-target tokens.
fn is_valid_target_name(name: &str) -> bool {
    if name.is_empty()
        || name.starts_with('.')
        || name.contains('%')
        || name.contains('$')
        || name.contains('=')
        || name.contains(';')
    {
        return false;
    }
    true
}

/// Classifies a target name into public `Target` or `Internal` (leading `_`).
pub fn classify_target(name: &str) -> TargetKind {
    if name.starts_with('_') {
        TargetKind::Internal
    } else {
        TargetKind::Target
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Workspace;
    use std::fs;

    fn temp_workspace(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lithe-makefile-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_phony_descriptions_variables_and_classification() {
        let dir = temp_workspace("basic");
        fs::write(
            dir.join("Makefile"),
            r#"
APP := api
.PHONY: build run dev test _helper

# Build the Go binary
build:
	go build -o bin/$(APP) ./cmd/$(APP)

run: ## Run the application locally
	go run ./cmd/$(APP)

dev: ## Start hot-reload dev server
	air

test:
	go test ./...

%.o: %.c
	cc -c $< -o $@

_helper:
	@echo internal
"#,
        )
        .unwrap();

        let ws = Workspace::new(&dir).unwrap();
        let instance = ws.instance(".").unwrap();
        let parsed = parse_instance(&instance, None).unwrap();

        let names: Vec<&str> = parsed.targets.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["build", "run", "dev", "test", "_helper"]);

        assert!(parsed.targets[0].is_default);
        assert_eq!(parsed.targets[0].kind, TargetKind::Target);
        assert_eq!(
            parsed.targets[0].description.as_deref(),
            Some("Build the Go binary")
        );

        assert_eq!(parsed.targets[1].kind, TargetKind::Target);
        assert!(parsed.targets[1].phony);
        assert_eq!(
            parsed.targets[1].description.as_deref(),
            Some("Run the application locally")
        );

        assert_eq!(parsed.targets[2].kind, TargetKind::Target);
        assert_eq!(parsed.targets[4].kind, TargetKind::Internal);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn follows_workspace_includes_and_blocks_escape() {
        let dir = temp_workspace("includes");
        fs::create_dir_all(dir.join("mk")).unwrap();
        fs::write(
            dir.join("mk/dev.mk"),
            ".PHONY: dev-up\ndev-up: ## Boot dev stack\n\tdocker compose up\n",
        )
        .unwrap();
        fs::write(
            dir.join("Makefile"),
            "include mk/dev.mk\n-include ../outside.mk\nbuild:\n\techo ok\n",
        )
        .unwrap();

        let ws = Workspace::new(&dir).unwrap();
        let instance = ws.instance(".").unwrap();
        let parsed = parse_instance(&instance, None).unwrap();

        let names: Vec<&str> = parsed.targets.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["dev-up", "build"]);
        assert_eq!(parsed.targets[0].source_file, "mk/dev.mk");
        assert_eq!(parsed.targets[0].kind, TargetKind::Target);
        assert_eq!(parsed.skipped_includes, 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn skips_define_blocks_and_target_specific_variables() {
        let dir = temp_workspace("define");
        fs::write(
            dir.join("Makefile"),
            r#"
define HELP_TEXT
fake-target:
	echo not a target
endef

build: CGO_ENABLED = 0
build:
	go build ./...
"#,
        )
        .unwrap();

        let ws = Workspace::new(&dir).unwrap();
        let instance = ws.instance(".").unwrap();
        let parsed = parse_instance(&instance, None).unwrap();

        let names: Vec<&str> = parsed.targets.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["build"]);

        let _ = fs::remove_dir_all(&dir);
    }
}
