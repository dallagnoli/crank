use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use thiserror::Error;

use crate::catalog::{ActionDefinition, ActionId, Catalog, CatalogFiles};
use crate::host::{Availability, Host, availability};

#[derive(Debug)]
pub struct ResolvedCommand {
    pub action_id: ActionId,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub working_directory: PathBuf,
    _assets: TempDir,
}

impl ResolvedCommand {
    pub fn assets_root(&self) -> &Path {
        self._assets.path()
    }
}

#[derive(Debug, Error)]
pub enum ResolveError {
    #[error("action `{0}` does not exist")]
    UnknownAction(ActionId),
    #[error("action `{action}` is unavailable: {reasons}")]
    Unavailable { action: ActionId, reasons: String },
    #[error("could not create a temporary directory for action `{action}`: {source}")]
    CreateTemp {
        action: ActionId,
        source: std::io::Error,
    },
    #[error("bundled asset `{path}` for action `{action}` is missing")]
    MissingAsset { action: ActionId, path: PathBuf },
    #[error("could not extract asset `{path}` for action `{action}`: {source}")]
    Extract {
        action: ActionId,
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("interpreter `{program}` for action `{action}` disappeared from PATH")]
    InterpreterDisappeared { action: ActionId, program: String },
}

pub fn resolve(
    catalog: &Catalog,
    files: &impl CatalogFiles,
    id: &ActionId,
    host: &Host,
) -> Result<ResolvedCommand, ResolveError> {
    let action = catalog
        .actions
        .iter()
        .find(|action| &action.id == id)
        .ok_or_else(|| ResolveError::UnknownAction(id.clone()))?;
    let Availability::Available = availability(action, host) else {
        let Availability::Unavailable { reasons } = availability(action, host) else {
            unreachable!()
        };
        return Err(ResolveError::Unavailable {
            action: id.clone(),
            reasons: reasons.join("; "),
        });
    };
    resolve_available(action, files, host)
}

fn resolve_available(
    action: &ActionDefinition,
    files: &impl CatalogFiles,
    host: &Host,
) -> Result<ResolvedCommand, ResolveError> {
    let temp = tempfile::Builder::new()
        .prefix("crank-")
        .tempdir()
        .map_err(|source| ResolveError::CreateTemp {
            action: action.id.clone(),
            source,
        })?;
    for relative_path in files.paths() {
        let bytes = files
            .read(&relative_path)
            .ok_or_else(|| ResolveError::MissingAsset {
                action: action.id.clone(),
                path: relative_path.clone(),
            })?;
        let extracted_path = temp.path().join(&relative_path);
        if let Some(parent) = extracted_path.parent() {
            fs::create_dir_all(parent).map_err(|source| ResolveError::Extract {
                action: action.id.clone(),
                path: parent.to_owned(),
                source,
            })?;
        }
        fs::write(&extracted_path, bytes).map_err(|source| ResolveError::Extract {
            action: action.id.clone(),
            path: relative_path,
            source,
        })?;
    }
    let extracted_script = temp.path().join(&action.script);

    let program = host
        .resolve_executable(&action.interpreter.program)
        .ok_or_else(|| ResolveError::InterpreterDisappeared {
            action: action.id.clone(),
            program: action.interpreter.program.clone(),
        })?;
    let mut args = action.interpreter.args.clone();
    args.push(extracted_script.to_string_lossy().into_owned());
    let working_directory = extracted_script
        .parent()
        .expect("validated script paths have a parent")
        .to_owned();
    Ok(ResolvedCommand {
        action_id: action.id.clone(),
        program,
        args,
        working_directory,
        _assets: temp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::load_embedded;

    #[test]
    fn command_keeps_extracted_script_alive() {
        let bundle = load_embedded().unwrap();
        let host = Host::detect();
        let id = bundle.catalog.actions[0].id.clone();
        let command = resolve(&bundle.catalog, &bundle.files, &id, &host).unwrap();
        let script = command.args.last().unwrap();
        assert!(Path::new(script).exists());
        assert!(command.working_directory.starts_with(command.assets_root()));
        assert!(command.assets_root().join("assets/message.txt").exists());
    }
}
