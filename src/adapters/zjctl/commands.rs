#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZjctlCommand {
    List,
    Capture {
        selector: String,
        full: bool,
    },
    WaitIdle {
        selector: String,
        idle_seconds: String,
        timeout_seconds: String,
    },
    Close {
        selector: String,
        force: bool,
    },
}

impl ZjctlCommand {
    pub fn args(&self) -> Vec<String> {
        match self {
            Self::List => vec!["panes".to_string(), "ls".to_string(), "--json".to_string()],
            Self::Capture { selector, full } => {
                let mut args = vec![
                    "pane".to_string(),
                    "capture".to_string(),
                    "--pane".to_string(),
                    selector.clone(),
                ];

                if *full {
                    args.push("--full".to_string());
                }

                args
            }
            Self::WaitIdle {
                selector,
                idle_seconds,
                timeout_seconds,
            } => vec![
                "pane".to_string(),
                "wait-idle".to_string(),
                "--pane".to_string(),
                selector.clone(),
                "--idle-time".to_string(),
                idle_seconds.clone(),
                "--timeout".to_string(),
                timeout_seconds.clone(),
                "--full".to_string(),
            ],
            Self::Close { selector, force } => {
                let mut args = vec![
                    "pane".to_string(),
                    "close".to_string(),
                    "--pane".to_string(),
                    selector.clone(),
                ];

                if *force {
                    args.push("--force".to_string());
                }

                args
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ZjctlCommand;

    #[test]
    fn builds_list_args() {
        assert_eq!(ZjctlCommand::List.args(), vec!["panes", "ls", "--json"]);
    }

    #[test]
    fn builds_capture_args() {
        assert_eq!(
            ZjctlCommand::Capture {
                selector: "id:terminal:7".to_string(),
                full: true,
            }
            .args(),
            vec!["pane", "capture", "--pane", "id:terminal:7", "--full"]
        );
    }

    #[test]
    fn builds_wait_idle_args() {
        assert_eq!(
            ZjctlCommand::WaitIdle {
                selector: "id:terminal:7".to_string(),
                idle_seconds: "1.2".to_string(),
                timeout_seconds: "30.0".to_string(),
            }
            .args(),
            vec![
                "pane",
                "wait-idle",
                "--pane",
                "id:terminal:7",
                "--idle-time",
                "1.2",
                "--timeout",
                "30.0",
                "--full"
            ]
        );
    }

    #[test]
    fn builds_close_args() {
        assert_eq!(
            ZjctlCommand::Close {
                selector: "id:terminal:7".to_string(),
                force: true,
            }
            .args(),
            vec!["pane", "close", "--pane", "id:terminal:7", "--force"]
        );
    }
}
