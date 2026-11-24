//! GNU Make database reader (`make -p -R -r -q :`).
//!
//! Uses GNU Make's own rule evaluator so complex Makefiles (`include *.mk`,
//! `$(wildcard ...)`, `$(foreach ...)`, `$(eval $(call ...))`, `ifeq` branches,
//! computed target names) are resolved accurately without executing recipes.
//!
//! Falls back to the static parser when `make` is unavailable or fails.

use crate::parser::{self, classify_target, MakeTarget, ParsedMakefile};
use crate::paths::Instance;
use crate::process::{self, Invocation};
use crate::request::Request;
use lithe_plugin_sdk::{Cancellation, FunctionError, PluginError};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::Path;
use std::time::{Duration, Instant};

/// Upper bound on bytes collected from `make -p -R -r -q` (16 MiB).
const MAX_DB_BYTES: usize = 16 * 1024 * 1024;
/// Upper bound on wall time for `make -p -R -r -q`.
const DB_TIMEOUT: Duration = Duration::from_secs(10);

/// Result of resolving targets for a Makefile instance.
#[derive(Debug, Clone)]
pub struct ResolvedMakefile {
    pub parsed: ParsedMakefile,
    /// Set when `make -p -R -r -q` could not be used and static parsing took over.
    pub fallback_reason: Option<&'static str>,
}

/// Resolves targets using GNU Make's database first, merging descriptions from
/// the static parser, and falling back to static parsing if `make` fails.
pub fn resolve(
    instance: &Instance,
    request: &Request,
    cancellation: &Cancellation,
) -> Result<ResolvedMakefile, FunctionError> {
    cancellation.check()?;
    let entry_path = instance.makefile(request.explicit_file())?;
    let entry_file = instance
        .relative(&entry_path)
        .unwrap_or_else(|| "Makefile".to_string());

    // Static parse is fast, never spawns processes, and supplies doc comments
    // (which `make -p` discards) as well as a safe fallback.
    let static_parsed = parser::parse_instance(instance, request.explicit_file())?;

    match query_make_database(instance, request, &entry_file, cancellation) {
        Ok(db) if !db.targets.is_empty() => {
            let merged = merge_with_static(db, &static_parsed);
            Ok(ResolvedMakefile {
                parsed: merged,
                fallback_reason: None,
            })
        }
        Ok(_) => Ok(ResolvedMakefile {
            parsed: static_parsed,
            fallback_reason: Some("makefile.databaseEmpty"),
        }),
        Err(FunctionError::Cancelled) => Err(FunctionError::Cancelled),
        Err(_) => Ok(ResolvedMakefile {
            parsed: static_parsed,
            fallback_reason: Some("makefile.databaseFallback"),
        }),
    }
}

fn query_make_database(
    instance: &Instance,
    request: &Request,
    entry_file: &str,
    cancellation: &Cancellation,
) -> Result<ParsedMakefile, FunctionError> {
    let mut args: Vec<OsString> = vec![
        "-p".into(),
        "-R".into(),
        "-r".into(),
        "-q".into(),
    ];
    if let Some(file) = request.explicit_file() {
        args.push("-f".into());
        args.push(OsString::from(file));
    }
    // Dummy goal `:` prevents GNU Make from running the default goal while
    // still dumping the complete evaluated `# Files` database.
    args.push(":".into());

    let invocation = Invocation {
        program: request.make_program(),
        arguments: args,
        directory: instance.root.clone(),
        environment: request.env_pairs(),
    };

    let started = Instant::now();
    let mut bytes_out = Vec::new();
    let mut overflow = false;

    // `make -p -R -r -q :` typically exits with code 2 ("No rule to make target `:'"),
    // which is expected; what matters is whether stdout contains `# Files`.
    let _code = process::run(&invocation, cancellation, |chunk| {
        if started.elapsed() > DB_TIMEOUT {
            return Err(PluginError::new(
                "makefile.databaseTimeout",
                "Reading the Makefile database timed out.",
            )
            .into());
        }
        if bytes_out.len() + chunk.len() <= MAX_DB_BYTES {
            bytes_out.extend_from_slice(chunk);
        } else {
            overflow = true;
            return Err(PluginError::new(
                "makefile.databaseTooLarge",
                "The Makefile database output exceeded 16 MiB.",
            )
            .into());
        }
        Ok(())
    })?;

    if overflow {
        return Err(PluginError::new(
            "makefile.databaseTooLarge",
            "The Makefile database output exceeded 16 MiB.",
        )
        .into());
    }

    let text = String::from_utf8_lossy(&bytes_out);
    parse_database_text(&text, instance, entry_file)
}

/// Parses GNU Make `-p -R -r -q` stdout into a `ParsedMakefile`.
pub fn parse_database_text(
    text: &str,
    instance: &Instance,
    entry_file: &str,
) -> Result<ParsedMakefile, FunctionError> {
    let mut in_variables = false;
    let mut in_files = false;
    let mut found_files_section = false;

    let mut default_goal: Option<String> = None;
    let mut phony_set: BTreeSet<String> = BTreeSet::new();

    #[derive(Debug, Default)]
    struct DbEntry {
        name: String,
        not_a_target: bool,
        phony: bool,
        has_recipe: bool,
        source_file: Option<String>,
        line: Option<usize>,
    }

    let mut entries: Vec<DbEntry> = Vec::new();
    let mut current: Option<DbEntry> = None;
    let mut next_not_a_target = false;

    for raw_line in text.lines() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);

        if line == "# Variables" {
            in_variables = true;
            in_files = false;
            continue;
        }
        if line == "# Files" {
            in_variables = false;
            in_files = true;
            found_files_section = true;
            continue;
        }
        if line.starts_with("# files hash-table stats:") || line == "# Directories" {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            in_files = false;
            in_variables = false;
            continue;
        }

        if in_variables {
            if let Some(val) = line
                .strip_prefix(".DEFAULT_GOAL :=")
                .or_else(|| line.strip_prefix(".DEFAULT_GOAL ="))
            {
                let goal = val.trim();
                if !goal.is_empty() {
                    default_goal = Some(goal.to_string());
                }
            }
            continue;
        }

        if !in_files {
            continue;
        }

        if line.is_empty() {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            next_not_a_target = false;
            continue;
        }

        if line == "# Not a target:" {
            next_not_a_target = true;
            continue;
        }

        // Metadata comments belonging to the current target entry.
        if line.starts_with('#') {
            if let Some(entry) = current.as_mut() {
                if line.contains("Phony target") {
                    entry.phony = true;
                }
                if let Some((src, ln)) = parse_recipe_location(line) {
                    entry.has_recipe = true;
                    if entry.source_file.is_none() {
                        entry.source_file = Some(normalize_source_file(instance, &src, entry_file));
                        entry.line = Some(ln);
                    }
                }
            }
            continue;
        }

        // Recipe lines start with a tab.
        if line.starts_with('\t') {
            if let Some(entry) = current.as_mut() {
                entry.has_recipe = true;
            }
            continue;
        }

        // Start of a new target header line: `<target>: <prereqs>`
        if current.is_none() {
            if let Some((lhs, rhs)) = split_db_target(line) {
                if lhs == ".PHONY" {
                    for name in rhs.split_whitespace() {
                        phony_set.insert(name.to_string());
                    }
                    // Consume `.PHONY` block without recording it as a user target.
                    current = Some(DbEntry {
                        name: ".PHONY".to_string(),
                        not_a_target: true,
                        ..DbEntry::default()
                    });
                    next_not_a_target = false;
                    continue;
                }
                current = Some(DbEntry {
                    name: lhs.to_string(),
                    not_a_target: next_not_a_target,
                    ..DbEntry::default()
                });
                next_not_a_target = false;
            }
        }
    }
    if let Some(entry) = current.take() {
        entries.push(entry);
    }

    if !found_files_section {
        return Err(PluginError::new(
            "makefile.databaseInvalid",
            "make did not produce a # Files database section.",
        )
        .into());
    }

    let mut targets = Vec::new();
    for entry in entries {
        if entry.not_a_target {
            continue;
        }
        let name = entry.name.trim();
        if name.is_empty()
            || name == ":"
            || name.starts_with('.')
            || name.contains('%')
            || name.contains(char::is_whitespace)
            || name == entry_file
        {
            continue;
        }
        let phony = entry.phony || phony_set.contains(name);
        // Skip bare file prerequisites that have neither `.PHONY` nor a recipe
        // (except when they appear as explicit phony targets).
        if !phony && !entry.has_recipe {
            continue;
        }
        let kind = classify_target(name);
        targets.push(MakeTarget {
            name: name.to_string(),
            phony,
            is_default: false,
            kind,
            source_file: entry.source_file.unwrap_or_else(|| entry_file.to_string()),
            line: entry.line.unwrap_or(1),
            description: None,
        });
    }

    // Sort deterministically: entry Makefile first, then by source file, line
    // number, and target name (GNU Make prints hash buckets in hash order).
    targets.sort_by(|a, b| {
        let a_entry = a.source_file == entry_file;
        let b_entry = b.source_file == entry_file;
        b_entry
            .cmp(&a_entry)
            .then_with(|| a.source_file.cmp(&b.source_file))
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| a.name.cmp(&b.name))
    });

    if let Some(goal) = default_goal {
        if let Some(t) = targets.iter_mut().find(|t| t.name == goal) {
            t.is_default = true;
        }
    } else if let Some(t) = targets.iter_mut().find(|t| t.kind != parser::TargetKind::Internal) {
        t.is_default = true;
    }

    Ok(ParsedMakefile {
        entry_file: entry_file.to_string(),
        targets,
        skipped_includes: 0,
    })
}

fn merge_with_static(mut db: ParsedMakefile, static_parsed: &ParsedMakefile) -> ParsedMakefile {
    let by_name: BTreeMap<&str, &MakeTarget> = static_parsed
        .targets
        .iter()
        .map(|t| (t.name.as_str(), t))
        .collect();

    for target in &mut db.targets {
        if let Some(st) = by_name.get(target.name.as_str()) {
            if target.description.is_none() {
                target.description = st.description.clone();
            }
            if target.line == 1 && st.line > 1 {
                target.line = st.line;
                target.source_file = st.source_file.clone();
            }
        }
    }
    db
}

fn split_db_target(line: &str) -> Option<(&str, &str)> {
    // Handle `::` (dummy target) and double-colon rules.
    if line.starts_with("::") {
        return Some((":", &line[2..]));
    }
    let idx = line.find(':')?;
    let lhs = line[..idx].trim();
    let rhs = line[idx + 1..].trim_start_matches(':').trim();
    (!lhs.is_empty()).then_some((lhs, rhs))
}

/// Parses GNU Make's `#  commands to execute (from \`Makefile', line 47):` (3.81)
/// and `#  recipe to execute (from 'Makefile', line 47):` (4.x).
fn parse_recipe_location(comment: &str) -> Option<(String, usize)> {
    let marker = "(from ";
    let start = comment.find(marker)? + marker.len();
    let rest = &comment[start..];
    let rest = rest
        .strip_prefix('`')
        .or_else(|| rest.strip_prefix('\''))?;
    let file_end = rest.find('\'')?;
    let file = rest[..file_end].to_string();
    let after_file = &rest[file_end + 1..];
    let line_marker = "line ";
    let line_start = after_file.find(line_marker)? + line_marker.len();
    let digits: String = after_file[line_start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let line = digits.parse::<usize>().ok()?;
    Some((file, line))
}

fn normalize_source_file(instance: &Instance, raw: &str, entry_file: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return entry_file.to_string();
    }
    if let Some(resolved) = instance.resolve_inside_workspace(trimmed) {
        if let Some(rel) = instance.relative(&resolved) {
            return rel;
        }
    }
    Path::new(trimmed)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Workspace;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;

    fn temp_workspace(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lithe-makefile-db-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn resolves_dynamic_foreach_eval_and_wildcard_includes_via_gnu_make() {
        let dir = temp_workspace("dynamic");
        fs::create_dir_all(dir.join("mk")).unwrap();
        fs::write(
            dir.join("mk/services.mk"),
            r#"
SERVICES := api worker

define SERVICE_RULES
.PHONY: run-$(1) build-$(1)
run-$(1):
	@echo running $(1)
build-$(1):
	@echo building $(1)
endef

$(foreach s,$(SERVICES),$(eval $(call SERVICE_RULES,$(s))))
"#,
        )
        .unwrap();
        fs::write(
            dir.join("Makefile"),
            r#"
.DEFAULT_GOAL := help
include $(wildcard mk/*.mk)
.PHONY: help run dev build

help: ## Show help
	@echo help

build: ## Build app
	@echo build

run dev: build
	@echo run-dev
"#,
        )
        .unwrap();

        let ws = Workspace::new(&dir).unwrap();
        let instance = ws.instance(".").unwrap();
        let req = Request::parse(&json!({ "root": "." })).unwrap();
        let cancel = Cancellation::new();
        let resolved = resolve(&instance, &req, &cancel).unwrap();

        assert_eq!(resolved.fallback_reason, None);
        let names: Vec<&str> = resolved
            .parsed
            .targets
            .iter()
            .map(|t| t.name.as_str())
            .collect();

        // Both static targets (`run`, `dev`, `build`, `help`) AND dynamically
        // evaluated targets from wildcard include (`run-api`, `build-api`,
        // `run-worker`, `build-worker`) are discovered!
        for expected in [
            "help",
            "build",
            "run",
            "dev",
            "run-api",
            "build-api",
            "run-worker",
            "build-worker",
        ] {
            assert!(
                names.contains(&expected),
                "missing target {expected} in {names:?}"
            );
        }

        let help = resolved.parsed.targets.iter().find(|t| t.name == "help").unwrap();
        assert!(help.is_default);
        assert_eq!(help.description.as_deref(), Some("Show help"));

        let run_api = resolved.parsed.targets.iter().find(|t| t.name == "run-api").unwrap();
        assert_eq!(run_api.kind, parser::TargetKind::Target);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn falls_back_cleanly_when_make_binary_is_missing() {
        let dir = temp_workspace("fallback");
        fs::write(dir.join("Makefile"), ".PHONY: dev\ndev: ## Start dev\n\t@echo dev\n").unwrap();

        let ws = Workspace::new(&dir).unwrap();
        let instance = ws.instance(".").unwrap();
        let req = Request::parse(&json!({
            "root": ".",
            "tool": { "executable": "/nonexistent/make-binary-does-not-exist" }
        }))
        .unwrap();
        let cancel = Cancellation::new();
        let resolved = resolve(&instance, &req, &cancel).unwrap();

        assert_eq!(resolved.fallback_reason, Some("makefile.databaseFallback"));
        assert_eq!(resolved.parsed.targets.len(), 1);
        assert_eq!(resolved.parsed.targets[0].name, "dev");

        let _ = fs::remove_dir_all(&dir);
    }
}
