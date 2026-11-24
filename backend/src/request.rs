//! Common fields parsed from `lithe/build/*` JSON-RPC requests.

use lithe_plugin_sdk::PluginError;
use serde_json::Value;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

/// Common request parameters shared by model, execute, and prepareRun.
#[derive(Debug, Clone)]
pub struct Request {
    pub root: String,
    pub tool_executable: Option<PathBuf>,
    pub option_arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub options: Value,
}

impl Request {
    pub fn parse(params: &Value) -> Result<Self, PluginError> {
        let root = params["root"].as_str().unwrap_or(".").to_string();
        let tool_executable = params["tool"]["executable"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        let option_arguments = strings(params, "optionArguments");
        let environment = string_map(&params["environment"]);
        let options = params.get("options").cloned().unwrap_or(Value::Null);
        Ok(Self {
            root,
            tool_executable,
            option_arguments,
            environment,
            options,
        })
    }

    /// Returns the explicit `file` build option if configured.
    pub fn explicit_file(&self) -> Option<&str> {
        self.options
            .get("file")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }

    /// Resolves the `make` executable to invoke for `lithe/build/execute`.
    pub fn make_program(&self) -> PathBuf {
        if let Some(path) = &self.tool_executable {
            return path.clone();
        }
        PathBuf::from("make")
    }

    /// Returns the bare command name (`make`, `gmake`, or `mingw32-make`) for
    /// portable `lithe/build/prepareRun` launch plans.
    pub fn portable_make_command(&self) -> String {
        if let Some(path) = &self.tool_executable {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if stem.eq_ignore_ascii_case("gmake") {
                    return "gmake".to_string();
                }
                if stem.eq_ignore_ascii_case("mingw32-make") {
                    return "mingw32-make".to_string();
                }
            }
        }
        "make".to_string()
    }

    /// Environment pairs ready for `std::process::Command`.
    pub fn env_pairs(&self) -> Vec<(OsString, OsString)> {
        self.environment
            .iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v)))
            .collect()
    }
}

/// Reads an optional array of strings at `object[field]`.
pub fn strings(object: &Value, field: &str) -> Vec<String> {
    object
        .get(field)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Reads an optional JSON object of string values into an ordered map.
pub fn string_map(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}
