//! The Makefile plugin's build provider backend.
//!
//! Answers the host's `lithe/build/*` requests for the `makefile` build system
//! declared in `plugin.json`:
//!
//! | Request | What it does |
//! | --- | --- |
//! | `model` | Statically parses the project's Makefile (and workspace-contained `include` files) into targets, `.PHONY` metadata, and descriptions. |
//! | `execute` | Runs a selected Makefile target or the default goal and streams output back to the host. |
//! | `prepareRun` | Returns the portable `make` launch plan for any discovered target so it runs directly from Run Configurations. |
//!
//! The backend keeps no persistent state on disk and never writes into the
//! plugin package or workspace.

mod database;
mod execute;
mod launch;
mod model;
mod parser;
mod paths;
mod process;
mod request;
mod view;

use lithe_plugin_sdk::Backend;
use paths::Workspace;
use std::sync::{Arc, OnceLock};

fn main() {
    let workspace: Arc<OnceLock<Workspace>> = Arc::new(OnceLock::new());
    let mut backend = Backend::new("makefile-build").version(env!("CARGO_PKG_VERSION"));

    let root = Arc::clone(&workspace);
    backend.on_initialize(move |initialize| {
        let path = initialize
            .root_path()
            .or_else(|| std::env::current_dir().ok());
        if let Some(path) = path.and_then(|path| Workspace::new(&path)) {
            let _ = root.set(path);
        }
        Ok(())
    });

    view::register(&mut backend).expect("the view ID is valid");

    let handlers = Arc::clone(&workspace);
    let current = move || paths::current(&handlers);

    let model_ws = current.clone();
    backend.build_model(move |params, context| model::answer(&model_ws()?, &params, context));

    let execute_ws = current.clone();
    backend.build_execute(move |params, context| execute::answer(&execute_ws()?, &params, context));

    let run_ws = current;
    backend.build_prepare_run(move |params, context| {
        launch::prepare_run(&run_ws()?, &params, context)
    });

    std::process::exit(backend.listen());
}
