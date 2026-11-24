//! `lithe/build/execute`: runs a selected Makefile target (`run-target`) or
//! the Makefile's default goal (`default`) and streams output to the host.

use crate::database;
use crate::paths::Workspace;
use crate::process::{self, Invocation};
use crate::request::{self, Request};
use lithe_plugin_sdk::{BuildContext, FunctionError, PluginError};
use serde_json::{json, Value};
use std::ffi::OsString;

const OPERATION: &str = "lithe/build/execute";
/// Core accepts up to 32 MiB of final output; leave headroom for truncation notice.
const OUTPUT_LIMIT: usize = 32 * 1024 * 1024 - 4 * 1024;

pub fn answer(
    workspace: &Workspace,
    params: &Value,
    context: &BuildContext,
) -> Result<Value, FunctionError> {
    context.check_cancelled()?;
    let request = Request::parse(params)?;
    let instance = workspace.instance(&request.root)?;
    let task = params["task"].as_str().unwrap_or_default();

    let mut arguments: Vec<OsString> = request
        .option_arguments
        .iter()
        .map(OsString::from)
        .collect();

    match task {
        "run-target" => {
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
            let resolved = database::resolve(&instance, &request, context.cancellation())?;
            if !resolved.parsed.targets.iter().any(|t| t.name == target_name) {
                return Err(PluginError::new(
                    "makefile.staleTarget",
                    format!(
                        "Makefile target '{target_name}' no longer exists. Reload the Makefile model."
                    ),
                )
                .into());
            }
            arguments.push(OsString::from(target_name));
        }
        "default" => {
            // Ensure the Makefile still exists before running the default goal.
            let _ = instance.makefile(request.explicit_file())?;
        }
        _ => {
            return Err(PluginError::new(
                "makefile.unknownTask",
                "Makefile plugin does not recognize this task.",
            )
            .into());
        }
    }

    for arg in request::strings(params, "userArguments") {
        arguments.push(OsString::from(arg));
    }

    let invocation = Invocation {
        program: request.make_program(),
        arguments,
        directory: instance.root.clone(),
        environment: request.env_pairs(),
    };

    let mut output = String::new();
    let mut truncated = false;

    let code = process::run(&invocation, context.cancellation(), |bytes| {
        let text = String::from_utf8_lossy(bytes);
        context.output(&text);
        if !truncated {
            if output.len() + text.len() <= OUTPUT_LIMIT {
                output.push_str(&text);
            } else {
                truncated = true;
            }
        }
        Ok(())
    })?;

    if truncated {
        output.push_str("\n[output truncated at 32 MiB]\n");
        let _ = context
            .diagnostics()
            .degraded(OPERATION, "makefile.outputTruncated", None);
    }

    Ok(json!({
        "exitCode": code,
        "output": output,
    }))
}
