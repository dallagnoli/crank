use std::env;
use std::path::{Path, PathBuf};

use crate::catalog::{ActionDefinition, Interpreter};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Host {
    pub os: String,
    pub arch: String,
    pub path: Vec<PathBuf>,
}

impl Host {
    pub fn detect() -> Self {
        Self {
            os: normalize_os(env::consts::OS).to_owned(),
            arch: normalize_arch(env::consts::ARCH).to_owned(),
            path: env::split_paths(&env::var_os("PATH").unwrap_or_default()).collect(),
        }
    }

    pub fn resolve_executable(&self, name: &str) -> Option<PathBuf> {
        if name.contains('/') {
            return executable(Path::new(name)).then(|| PathBuf::from(name));
        }
        self.path
            .iter()
            .map(|directory| directory.join(name))
            .find(|candidate| executable(candidate))
    }
}

fn executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Availability {
    Available,
    Unavailable { reasons: Vec<String> },
}

impl Availability {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }
}

pub fn availability(action: &ActionDefinition, host: &Host) -> Availability {
    let mut reasons = Vec::new();
    if !action.requirements.os.is_empty()
        && !action
            .requirements
            .os
            .iter()
            .any(|os| normalize_os(os) == host.os)
    {
        reasons.push(format!(
            "requires OS: {}",
            action.requirements.os.join(", ")
        ));
    }
    if !action.requirements.arch.is_empty()
        && !action
            .requirements
            .arch
            .iter()
            .any(|arch| normalize_arch(arch) == host.arch)
    {
        reasons.push(format!(
            "requires architecture: {}",
            action.requirements.arch.join(", ")
        ));
    }
    missing_program(&action.interpreter, host, &mut reasons, "interpreter");
    for executable in &action.requirements.executables {
        if host.resolve_executable(executable).is_none() {
            reasons.push(format!(
                "required executable `{executable}` was not found on PATH"
            ));
        }
    }
    if reasons.is_empty() {
        Availability::Available
    } else {
        Availability::Unavailable { reasons }
    }
}

fn missing_program(interpreter: &Interpreter, host: &Host, reasons: &mut Vec<String>, kind: &str) {
    if host.resolve_executable(&interpreter.program).is_none() {
        reasons.push(format!(
            "{kind} `{}` was not found on PATH",
            interpreter.program
        ));
    }
}

pub fn normalize_os(value: &str) -> &str {
    match value {
        "darwin" | "macos" => "macos",
        other => other,
    }
}

pub fn normalize_arch(value: &str) -> &str {
    match value {
        "amd64" | "x86-64" | "x64" => "x86_64",
        "arm64" => "aarch64",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::catalog::{ActionId, CategoryId, Requirements};

    fn action() -> ActionDefinition {
        ActionDefinition {
            id: ActionId::from_str("test").unwrap(),
            label: "Test".into(),
            description: "Test".into(),
            category: CategoryId::from_str("tests").unwrap(),
            script: "test.sh".into(),
            interpreter: Interpreter::default(),
            requirements: Requirements::default(),
        }
    }

    #[test]
    fn explains_every_missing_capability() {
        let mut action = action();
        action.requirements.os = vec!["freebsd".into()];
        action.requirements.executables = vec!["definitely-not-installed".into()];
        let host = Host {
            os: "linux".into(),
            arch: "x86_64".into(),
            path: Vec::new(),
        };
        let Availability::Unavailable { reasons } = availability(&action, &host) else {
            panic!("action should be unavailable");
        };
        assert_eq!(reasons.len(), 3);
    }
}
