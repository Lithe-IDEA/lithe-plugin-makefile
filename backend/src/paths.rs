//! Workspace and Makefile instance path containment.
//!
//! Every path accepted from the host or discovered through `include` directives
//! is canonicalized and verified to stay inside the workspace root so symlinks
//! or `..` segments cannot escape it.

use lithe_plugin_sdk::{FunctionError, PluginError};
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

/// GNU Make's standard default Makefile search order.
pub const DEFAULT_MAKEFILES: &[&str] = &["GNUmakefile", "Makefile", "makefile"];

/// The open workspace folder, canonicalized.
#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
}

/// One detected Makefile build instance inside the workspace.
#[derive(Debug, Clone)]
pub struct Instance {
    workspace: PathBuf,
    /// Canonicalized absolute folder containing the Makefile.
    pub root: PathBuf,
    /// Workspace-relative folder with `/`, or `.` for the workspace root.
    pub relative: String,
}

impl Workspace {
    /// Creates a workspace rooted at `path`, or `None` when it is not a folder.
    pub fn new(path: &Path) -> Option<Self> {
        let root = resolve(path)?;
        root.is_dir().then_some(Self { root })
    }

    /// Resolves the Makefile instance at workspace-relative `root`.
    pub fn instance(&self, root: &str) -> Result<Instance, PluginError> {
        let invalid = || {
            PluginError::new(
                "makefile.invalidRoot",
                "The Makefile folder is not inside this workspace.",
            )
        };
        let joined = self.join(root).ok_or_else(invalid)?;
        let resolved = resolve(&joined).ok_or_else(invalid)?;
        if !resolved.starts_with(&self.root) || !resolved.is_dir() {
            return Err(invalid());
        }
        let relative = relative_to(&self.root, &resolved).ok_or_else(invalid)?;
        Ok(Instance {
            workspace: self.root.clone(),
            root: resolved,
            relative,
        })
    }

    /// Joins a plain relative path to the workspace root without escaping it.
    pub fn join(&self, path: &str) -> Option<PathBuf> {
        join(&self.root, path)
    }
}

impl Instance {
    /// Finds the active Makefile for this instance: an explicit `file` option
    /// if provided, otherwise GNU Make's standard `GNUmakefile` / `Makefile` /
    /// `makefile` order.
    pub fn makefile(&self, explicit_file: Option<&str>) -> Result<PathBuf, PluginError> {
        if let Some(file) = explicit_file.map(str::trim).filter(|s| !s.is_empty()) {
            let candidate = self
                .resolve_inside_workspace(file)
                .ok_or_else(|| {
                    PluginError::new(
                        "makefile.invalidFile",
                        "The selected Makefile is not inside this workspace.",
                    )
                })?;
            if candidate.is_file() {
                return Ok(candidate);
            }
            return Err(PluginError::new(
                "makefile.missingFile",
                "The selected Makefile does not exist.",
            ));
        }

        for name in DEFAULT_MAKEFILES {
            let candidate = self.root.join(name);
            if candidate.is_file() {
                if let Some(resolved) = resolve(&candidate) {
                    if resolved.starts_with(&self.workspace) {
                        return Ok(resolved);
                    }
                }
            }
        }
        Err(PluginError::new(
            "makefile.missingFile",
            "No GNUmakefile, Makefile, or makefile was found in this folder.",
        ))
    }

    /// Resolves `path` (relative to the instance folder, or absolute if inside
    /// the workspace) and verifies that its canonical target stays inside the
    /// workspace.
    pub fn resolve_inside_workspace(&self, path: &str) -> Option<PathBuf> {
        let candidate = Path::new(path);
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.root.join(candidate)
        };
        let resolved = resolve(&joined)?;
        resolved.starts_with(&self.workspace).then_some(resolved)
    }

    /// Resolves a workspace-relative path and ensures it lies inside the workspace.
    pub fn workspace_path(&self, path: &str) -> Option<PathBuf> {
        let joined = join(&self.workspace, path)?;
        let resolved = resolve(&joined)?;
        resolved.starts_with(&self.workspace).then_some(resolved)
    }

    /// Returns `path` relative to the workspace root with `/` separators.
    pub fn in_workspace(&self, path: &Path) -> Option<String> {
        let resolved = resolve(path)?;
        relative_to(&self.workspace, &resolved)
    }

    /// Returns `path` relative to the instance root with `/` separators.
    pub fn relative(&self, path: &Path) -> Option<String> {
        let resolved = resolve(path)?;
        relative_to(&self.root, &resolved)
    }

    /// Display name for the root project of this instance.
    pub fn project_name(&self) -> String {
        self.root
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("Makefile")
            .to_string()
    }
}

/// Returns the workspace initialized by the host.
pub fn current(workspace: &OnceLock<Workspace>) -> Result<Workspace, FunctionError> {
    workspace.get().cloned().ok_or_else(|| {
        PluginError::new(
            "makefile.noWorkspace",
            "Makefile plugin has no workspace folder to read. Reopen the folder.",
        )
        .into()
    })
}

fn resolve(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}

fn join(base: &Path, relative: &str) -> Option<PathBuf> {
    let trimmed = relative.trim();
    if trimmed.is_empty() || trimmed == "." {
        return Some(base.to_path_buf());
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        return None;
    }
    let mut out = base.to_path_buf();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => out.push(part),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(out)
}

fn relative_to(base: &Path, target: &Path) -> Option<String> {
    let stripped = target.strip_prefix(base).ok()?;
    let mut parts = Vec::new();
    for component in stripped.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => parts.push(part.to_str()?.to_string()),
            _ => return None,
        }
    }
    if parts.is_empty() {
        Some(".".to_string())
    } else {
        Some(parts.join("/"))
    }
}
