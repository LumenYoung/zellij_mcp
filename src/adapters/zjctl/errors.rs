use thiserror::Error;

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("zjctl command execution is not implemented yet")]
    Unimplemented,
}
