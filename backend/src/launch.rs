//! `lithe/build/prepareRun`: answers a portable `make` launch plan for any
//! discovered Makefile target so it can be launched directly from Lithe's
//! Run Configurations toolbar or Build window.

use crate::database;
use crate::paths::Workspace;
use crate::request::{self, Request};
use lithe_plugin_sdk::{BuildContext, FunctionError, PluginError};
use serde_json::{json, Map, Value};

pub fn prepare_run(
    workspace: &Workspace,
    params: &Value,
    context: &BuildContext,
) -> Result<Value, FunctionError> {
    context.check_cancelled()?;
    let request = Request::parse(params)?;
    let instance = workspace.instance(&request.root)?;

    let target_name = params["target"]["name"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            PluginError::new(
                "makefile.noTarget",
                "Select a Makefile target to run.",
            )
        })?;

    // Verify the target still exists in the current Makefile rather than
    // silently running a stale target name.
    let resolved = database::resolve(&instance, &request, context.cancellation())?;
    if !resolved.parsed.targets.iter().any(|t| t.name == target_name) {
        return Err(PluginError::new(
            "makefile.staleTarget",
            format!("Makefile target '{target_name}' no longer exists. Reload the Makefile model."),
        )
        .into());
    }

    let launch_options = &params["launchOptions"];
    let working_directory = match launch_options["workingDirectory"].as_str() {
        Some(dir) if !dir.trim().is_empty() => {
            let resolved = instance.workspace_path(dir).ok_or_else(|| {
                PluginError::new(
                    "makefile.invalidWorkingDirectory",
                    "The working directory is not a folder of this workspace.",
                )
            })?;
            instance.in_workspace(&resolved).ok_or_else(|| {
                PluginError::new(
                    "makefile.invalidWorkingDirectory",
                    "The working directory is not a folder of this workspace.",
                )
            })?
        }
        _ => instance.relative.clone(),
    };

    let mut arguments: Vec<String> = Vec::new();
    arguments.extend(request.option_arguments.iter().cloned());
    arguments.extend(request::strings(launch_options, "runtimeArguments"));
    arguments.push(target_name.to_string());
    arguments.extend(request::strings(launch_options, "arguments"));
    arguments.extend(request::strings(params, "userArguments"));

    let mut environment: Map<String, Value> = request
        .environment
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect();
    for (k, v) in request::string_map(&launch_options["environment"]) {
        environment.insert(k, Value::String(v));
    }

    Ok(json!({
        "executable": {
            "kind": "command",
            "value": request.portable_make_command(),
        },
        "arguments": arguments,
        "workingDirectory": working_directory,
        "environment": environment,
    }))
}
