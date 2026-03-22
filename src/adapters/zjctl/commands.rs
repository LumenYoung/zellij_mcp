#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZjctlCommand {
    Availability,
    List,
    Capture { selector: String, full: bool },
    Send { selector: String, text: String },
}

impl ZjctlCommand {
    pub fn args(&self) -> Vec<String> {
        match self {
            Self::Availability => vec!["--help".to_string()],
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
            Self::Send { selector, text } => vec![
                "pane".to_string(),
                "send".to_string(),
                "--pane".to_string(),
                selector.clone(),
                "--".to_string(),
                text.clone(),
            ],
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
}
