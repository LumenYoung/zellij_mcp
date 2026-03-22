#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZjctlCommand {
    Spawn,
    ResolveSelector,
    SendInput,
    WaitIdle,
    Capture,
    Close,
    List,
}
