#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZjctlCommand {
    Availability,
    List,
    Spawn {
        cwd: Option<String>,
        title: Option<String>,
        command: Vec<String>,
    },
    Capture {
        selector: String,
        full: bool,
    },
    Send {
        selector: String,
        text: String,
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
            Self::Availability => vec!["--help".to_string()],
            Self::List => vec!["panes".to_string(), "ls".to_string(), "--json".to_string()],
            Self::Spawn {
                cwd,
                title,
                command,
            } => {
                let mut args = vec!["pane".to_string(), "launch".to_string()];

                if let Some(cwd) = cwd {
                    args.push("--cwd".to_string());
                    args.push(cwd.clone());
                }

                if let Some(title) = title {
                    args.push("--name".to_string());
                    args.push(title.clone());
                }

                if !command.is_empty() {
                    args.push("--".to_string());
                    args.extend(command.clone());
                }

                args
            }
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
            Self::Send { selector, text } => vec![
                "pane".to_string(),
                "send".to_string(),
                "--pane".to_string(),
                selector.clone(),
                "--".to_string(),
                text.clone(),
            ],
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
    fn builds_spawn_args() {
        assert_eq!(
            ZjctlCommand::Spawn {
                cwd: Some("/tmp".to_string()),
                title: Some("editor".to_string()),
                command: vec!["lazygit".to_string()],
            }
            .args(),
            vec![
                "pane", "launch", "--cwd", "/tmp", "--name", "editor", "--", "lazygit"
            ]
        );
    }

    #[test]
    fn builds_send_args() {
        assert_eq!(
            ZjctlCommand::Send {
                selector: "id:terminal:7".to_string(),
                text: "printf 'ok\\n'\n".to_string(),
            }
            .args(),
            vec![
                "pane",
                "send",
                "--pane",
                "id:terminal:7",
                "--",
                "printf 'ok\\n'\n"
            ]
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
