use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use include_dir::{Dir, include_dir};
use serde::Deserialize;
use thiserror::Error;

const EMBEDDED_MANIFEST: &str = include_str!("../catalog/catalog.toml");
static EMBEDDED_DIRECTORY: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/catalog");

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub schema_version: u32,
    #[serde(default)]
    pub categories: Vec<Category>,
    #[serde(default)]
    pub actions: Vec<ActionDefinition>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Category {
    pub id: CategoryId,
    pub label: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ActionDefinition {
    pub id: ActionId,
    pub label: String,
    pub description: String,
    pub category: CategoryId,
    pub script: PathBuf,
    #[serde(default)]
    pub interpreter: Interpreter,
    #[serde(default)]
    pub requirements: Requirements,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Interpreter {
    #[serde(default = "default_interpreter")]
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
}

impl Default for Interpreter {
    fn default() -> Self {
        Self {
            program: default_interpreter(),
            args: Vec::new(),
        }
    }
}

fn default_interpreter() -> String {
    "sh".to_owned()
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Requirements {
    #[serde(default)]
    pub os: Vec<String>,
    #[serde(default)]
    pub arch: Vec<String>,
    #[serde(default)]
    pub executables: Vec<String>,
}

macro_rules! identifier {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = String;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                let valid = !value.is_empty()
                    && value.len() <= 64
                    && value.bytes().all(|byte| {
                        byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || matches!(byte, b'.' | b'-' | b'_')
                    });
                valid
                    .then(|| Self(value.to_owned()))
                    .ok_or_else(|| format!("invalid identifier `{value}`"))
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                value.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

identifier!(ActionId);
identifier!(CategoryId);

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("could not read catalog manifest {path}: {source}")]
    ReadManifest {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("catalog manifest is invalid: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("unsupported catalog schema version {found}; this build supports version 1")]
    SchemaVersion { found: u32 },
    #[error("duplicate category ID `{0}`")]
    DuplicateCategory(CategoryId),
    #[error("duplicate action ID `{0}`")]
    DuplicateAction(ActionId),
    #[error("action `{action}` refers to unknown category `{category}`")]
    UnknownCategory {
        action: ActionId,
        category: CategoryId,
    },
    #[error("action `{action}` has unsafe script path `{path}`")]
    UnsafeScriptPath { action: ActionId, path: PathBuf },
    #[error("action `{action}` refers to missing script `{path}`")]
    MissingScript { action: ActionId, path: PathBuf },
    #[error(
        "action `{action}` declares interpreter `{declared}` but script shebang uses `{shebang}`"
    )]
    InterpreterMismatch {
        action: ActionId,
        declared: String,
        shebang: String,
    },
    #[error("action `{action}` has an invalid executable requirement `{executable}`")]
    InvalidExecutable {
        action: ActionId,
        executable: String,
    },
}

pub trait CatalogFiles {
    fn read(&self, relative_path: &Path) -> Option<Vec<u8>>;
    fn paths(&self) -> Vec<PathBuf>;
}

#[derive(Clone, Debug)]
pub struct CatalogBundle<F> {
    pub catalog: Catalog,
    pub files: F,
    pub source: CatalogSource,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CatalogSource {
    Embedded,
    Local(PathBuf),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EmbeddedFiles;

impl CatalogFiles for EmbeddedFiles {
    fn read(&self, relative_path: &Path) -> Option<Vec<u8>> {
        EMBEDDED_DIRECTORY
            .get_file(relative_path)
            .map(|file| file.contents().to_vec())
    }

    fn paths(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        collect_embedded_paths(&EMBEDDED_DIRECTORY, &mut paths);
        paths
    }
}

fn collect_embedded_paths(directory: &Dir<'_>, paths: &mut Vec<PathBuf>) {
    paths.extend(
        directory
            .files()
            .filter(|file| file.path() != Path::new("catalog.toml"))
            .map(|file| file.path().to_owned()),
    );
    for child in directory.dirs() {
        collect_embedded_paths(child, paths);
    }
}

#[derive(Clone, Debug)]
pub struct LocalFiles {
    root: PathBuf,
}

impl LocalFiles {
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl CatalogFiles for LocalFiles {
    fn read(&self, relative_path: &Path) -> Option<Vec<u8>> {
        let path = self.root.join(relative_path).canonicalize().ok()?;
        path.starts_with(&self.root)
            .then(|| fs::read(path).ok())
            .flatten()
    }

    fn paths(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        collect_local_paths(&self.root, &self.root, &mut paths);
        paths
    }
}

fn collect_local_paths(root: &Path, directory: &Path, paths: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_local_paths(root, &path, paths);
        } else if file_type.is_file()
            && let Ok(relative) = path.strip_prefix(root)
            && relative != Path::new("catalog.toml")
        {
            paths.push(relative.to_owned());
        }
    }
}

pub fn load_embedded() -> Result<CatalogBundle<EmbeddedFiles>, CatalogError> {
    let files = EmbeddedFiles;
    let catalog = parse_and_validate(EMBEDDED_MANIFEST, &files)?;
    Ok(CatalogBundle {
        catalog,
        files,
        source: CatalogSource::Embedded,
    })
}

pub fn load_local(root: impl Into<PathBuf>) -> Result<CatalogBundle<LocalFiles>, CatalogError> {
    let requested_root = root.into();
    let root = requested_root
        .canonicalize()
        .map_err(|source| CatalogError::ReadManifest {
            path: requested_root.join("catalog.toml"),
            source,
        })?;
    let manifest_path = root.join("catalog.toml");
    let manifest =
        fs::read_to_string(&manifest_path).map_err(|source| CatalogError::ReadManifest {
            path: manifest_path,
            source,
        })?;
    let files = LocalFiles { root: root.clone() };
    let catalog = parse_and_validate(&manifest, &files)?;
    Ok(CatalogBundle {
        catalog,
        files,
        source: CatalogSource::Local(root),
    })
}

pub fn parse_and_validate(
    manifest: &str,
    files: &impl CatalogFiles,
) -> Result<Catalog, CatalogError> {
    let catalog: Catalog = toml::from_str(manifest)?;
    validate(&catalog, files)?;
    Ok(catalog)
}

pub fn validate(catalog: &Catalog, files: &impl CatalogFiles) -> Result<(), CatalogError> {
    if catalog.schema_version != 1 {
        return Err(CatalogError::SchemaVersion {
            found: catalog.schema_version,
        });
    }

    let mut category_ids = HashSet::new();
    for category in &catalog.categories {
        if !category_ids.insert(category.id.clone()) {
            return Err(CatalogError::DuplicateCategory(category.id.clone()));
        }
    }

    let mut action_ids = HashSet::new();
    for action in &catalog.actions {
        if !action_ids.insert(action.id.clone()) {
            return Err(CatalogError::DuplicateAction(action.id.clone()));
        }
        if !category_ids.contains(&action.category) {
            return Err(CatalogError::UnknownCategory {
                action: action.id.clone(),
                category: action.category.clone(),
            });
        }
        if !safe_relative_path(&action.script) {
            return Err(CatalogError::UnsafeScriptPath {
                action: action.id.clone(),
                path: action.script.clone(),
            });
        }
        for executable in &action.requirements.executables {
            if executable.is_empty() || executable.contains('/') {
                return Err(CatalogError::InvalidExecutable {
                    action: action.id.clone(),
                    executable: executable.clone(),
                });
            }
        }

        let script = files
            .read(&action.script)
            .ok_or_else(|| CatalogError::MissingScript {
                action: action.id.clone(),
                path: action.script.clone(),
            })?;
        validate_shebang(action, &script)?;
    }
    Ok(())
}

fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path.components().all(|component| {
            matches!(component, Component::Normal(_))
                && component.as_os_str() != "."
                && component.as_os_str() != ".."
        })
}

fn validate_shebang(action: &ActionDefinition, script: &[u8]) -> Result<(), CatalogError> {
    let Some(line) = script
        .strip_prefix(b"#!")
        .and_then(|rest| rest.split(|byte| *byte == b'\n').next())
    else {
        return Ok(());
    };
    let line = String::from_utf8_lossy(line);
    let shebang_program = line.split_ascii_whitespace().next().unwrap_or_default();
    let basename = Path::new(shebang_program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(shebang_program);
    let declared_basename = Path::new(&action.interpreter.program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&action.interpreter.program);
    if basename != declared_basename {
        return Err(CatalogError::InterpreterMismatch {
            action: action.id.clone(),
            declared: action.interpreter.program.clone(),
            shebang: line.into_owned(),
        });
    }
    Ok(())
}

pub fn actions_by_category(catalog: &Catalog) -> HashMap<&CategoryId, Vec<&ActionDefinition>> {
    let mut grouped = HashMap::new();
    for action in &catalog.actions {
        grouped
            .entry(&action.category)
            .or_insert_with(Vec::new)
            .push(action);
    }
    grouped
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestFiles(HashMap<PathBuf, Vec<u8>>);

    impl CatalogFiles for TestFiles {
        fn read(&self, relative_path: &Path) -> Option<Vec<u8>> {
            self.0.get(relative_path).cloned()
        }

        fn paths(&self) -> Vec<PathBuf> {
            self.0.keys().cloned().collect()
        }
    }

    fn files() -> TestFiles {
        TestFiles(HashMap::from([(
            PathBuf::from("scripts/test.sh"),
            b"#!/bin/sh\nprintf 'ok\\n'\n".to_vec(),
        )]))
    }

    fn manifest(extra: &str) -> String {
        format!(
            r#"
schema_version = 1

[[categories]]
id = "tests"
label = "Tests"

[[actions]]
id = "test.action"
label = "Test"
description = "A test"
category = "tests"
script = "scripts/test.sh"
{extra}
"#
        )
    }

    #[test]
    fn embedded_catalog_is_valid() {
        let bundle = load_embedded().unwrap();
        assert!(!bundle.catalog.actions.is_empty());
    }

    #[test]
    fn rejects_unknown_fields() {
        let error = parse_and_validate(&manifest("surprise = true"), &files()).unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn rejects_escaping_script_path() {
        let source = manifest("").replace("scripts/test.sh", "../test.sh");
        let error = parse_and_validate(&source, &files()).unwrap_err();
        assert!(matches!(error, CatalogError::UnsafeScriptPath { .. }));
    }

    #[test]
    fn rejects_unknown_category() {
        let source = manifest("").replace("category = \"tests\"", "category = \"missing\"");
        let error = parse_and_validate(&source, &files()).unwrap_err();
        assert!(matches!(error, CatalogError::UnknownCategory { .. }));
    }

    #[test]
    fn rejects_mismatched_shebang() {
        let source = manifest("[actions.interpreter]\nprogram = \"bash\"");
        let error = parse_and_validate(&source, &files()).unwrap_err();
        assert!(matches!(error, CatalogError::InterpreterMismatch { .. }));
    }
}
