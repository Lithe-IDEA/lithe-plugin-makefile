//! `lithe/build/model`: turns the parsed Makefile into a canonical
//! `litheBuildModel` document so Lithe populates both the Build tool window
//! and auto-generated Run Configurations (e.g. `run`, `dev`, `build`, `test`).

use crate::database;
use crate::paths::Workspace;
use crate::request::Request;
use lithe_plugin_sdk::{BuildContext, FunctionError};
use serde_json::{json, Map, Value};

const OPERATION: &str = "lithe/build/model";

pub fn answer(
    workspace: &Workspace,
    params: &Value,
    context: &BuildContext,
) -> Result<Value, FunctionError> {
    context.check_cancelled()?;
    let request = Request::parse(params)?;
    let instance = workspace.instance(&request.root)?;
    let resolved = database::resolve(&instance, &request, context.cancellation())?;
    let parsed = resolved.parsed;

    if let Some(reason) = resolved.fallback_reason {
        let _ = context.diagnostics().degraded(OPERATION, reason, None);
    }
    if parsed.skipped_includes > 0 {
        let _ = context.diagnostics().degraded(
            OPERATION,
            "makefile.includesSkipped",
            Some(parsed.skipped_includes as u64),
        );
    }

    let project_name = instance.project_name();
    let projects = vec![json!({
        "name": project_name,
        "properties": {
            "kind": "makefile",
            "makefile": parsed.entry_file,
        }
    })];

    let targets: Vec<Value> = parsed
        .targets
        .into_iter()
        .map(|target| {
            let mut props = Map::new();
            props.insert(
                "phony".into(),
                Value::String(if target.phony { "true" } else { "false" }.into()),
            );
            props.insert(
                "default".into(),
                Value::String(if target.is_default { "true" } else { "false" }.into()),
            );
            props.insert("sourceFile".into(), Value::String(target.source_file));
            props.insert("line".into(), Value::String(target.line.to_string()));
            if let Some(description) = target.description {
                props.insert("description".into(), Value::String(description));
            }
            json!({
                "project": ".",
                "path": ".",
                "name": target.name,
                "tags": [target.kind.tag()],
                "properties": props,
            })
        })
        .collect();

    Ok(json!({
        "projects": projects,
        "targets": targets,
    }))
}
