use std::fs;
use std::path::{Path, PathBuf};

use crate::domain::binding::TerminalBinding;
use crate::domain::errors::{DomainError, ErrorCode};

#[derive(Debug, Clone)]
pub struct RegistryStore {
    path: PathBuf,
}

impl RegistryStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn load(&self) -> Result<Vec<TerminalBinding>, DomainError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(&self.path).map_err(|error| {
            DomainError::new(
                ErrorCode::PersistenceError,
                format!("failed to read registry store: {error}"),
                false,
            )
        })?;

        serde_json::from_str(&content).map_err(|error| {
            DomainError::new(
                ErrorCode::PersistenceError,
                format!("failed to parse registry store: {error}"),
                false,
            )
        })
    }

    pub fn save(&self, bindings: &[TerminalBinding]) -> Result<(), DomainError> {
        ensure_parent(&self.path)?;
        let content = serde_json::to_string_pretty(bindings).map_err(|error| {
            DomainError::new(
                ErrorCode::PersistenceError,
                format!("failed to serialize registry store: {error}"),
                false,
            )
        })?;

        fs::write(&self.path, content).map_err(|error| {
            DomainError::new(
                ErrorCode::PersistenceError,
                format!("failed to write registry store: {error}"),
                false,
            )
        })
    }
}

fn ensure_parent(path: &Path) -> Result<(), DomainError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            DomainError::new(
                ErrorCode::PersistenceError,
                format!("failed to create persistence directory: {error}"),
                false,
            )
        })?;
    }

    Ok(())
}
