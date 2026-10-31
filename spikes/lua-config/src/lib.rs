//! v0.0 spike 2: run `tileide.lua` in a fresh sandboxed Lua state and turn the
//! `tileide.*` calls into a typed `Config`. Throwaway code (see v0.0.md).

pub mod api;
pub mod fields;
pub mod model;
pub mod sandbox;

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

pub use model::Config;
pub use sandbox::{FLAVOUR, Limits};

#[derive(Debug)]
pub struct Loaded {
    pub config: Config,
    pub warnings: Vec<String>,
    pub elapsed: Duration,
}

/// Runs the config at `path`. On error the caller keeps its last good `Config`.
pub fn load(path: &Path, limits: Limits) -> Result<Loaded, String> {
    let started = Instant::now();
    let shared = Rc::new(RefCell::new(api::Builder::default()));
    run(path, limits, &shared).map_err(|e| describe(&e))?;
    let builder = Rc::try_unwrap(shared)
        .map_err(|_| "internal: config state still referenced".to_string())?
        .into_inner();
    Ok(Loaded { config: builder.config, warnings: builder.warnings, elapsed: started.elapsed() })
}

fn run(path: &Path, limits: Limits, shared: &api::Shared) -> mlua::Result<()> {
    let lua = sandbox::new_state(limits)?;
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    sandbox::install_require(&lua, dir)?;
    let api = api::install(&lua, shared)?;
    lua.globals().set("tileide", api)?;
    sandbox::seal(&lua)?;
    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    sandbox::exec_file(&lua, path, &name)?;
    Ok(())
}

/// The innermost, human-readable message (already `file:line:` prefixed where Lua knows it).
fn describe(error: &mlua::Error) -> String {
    match error {
        mlua::Error::CallbackError { cause, .. } => describe(cause),
        mlua::Error::RuntimeError(message) => message.clone(),
        mlua::Error::SyntaxError { message, .. } => message.clone(),
        mlua::Error::MemoryError(_) => "config exceeded the memory limit".into(),
        other => other.to_string(),
    }
}
