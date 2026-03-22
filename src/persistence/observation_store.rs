use std::fs;
use std::path::{Path, PathBuf};

use crate::domain::errors::{DomainError, ErrorCode};
use crate::domain::observation::TerminalObservation;

#[derive(Debug, Clone)]
pub struct ObservationStore {
    path: PathBuf,
}

impl ObservationStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn load(&self) -> Result<Vec<TerminalObservation>, DomainError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(&self.path).map_err(|error| {
            DomainError::new(
                ErrorCode::PersistenceError,
                format!("failed to read observation store: {error}"),
                false,
            )
        })?;

        serde_json::from_str(&content).map_err(|error| {
            DomainError::new(
                ErrorCode::PersistenceError,
                format!("failed to parse observation store: {error}"),
                false,
            )
        })
    }

    pub fn save(&self, observations: &[TerminalObservation]) -> Result<(), DomainError> {
        ensure_parent(&self.path)?;
        let content = serde_json::to_string_pretty(observations).map_err(|error| {
            DomainError::new(
                ErrorCode::PersistenceError,
                format!("failed to serialize observation store: {error}"),
                false,
            )
        })?;

        fs::write(&self.path, content).map_err(|error| {
            DomainError::new(
                ErrorCode::PersistenceError,
                format!("failed to write observation store: {error}"),
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
