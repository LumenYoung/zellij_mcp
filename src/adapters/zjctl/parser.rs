use serde::Deserialize;

use crate::adapters::zjctl::AdapterError;
use crate::adapters::zjctl::client::ResolvedTarget;

#[cfg_attr(not(test), allow(dead_code))]
pub fn parse_single_selector(output: &str) -> Result<String, AdapterError> {
    let selector = output.trim();
    if selector.is_empty() {
        return Err(AdapterError::ParseError(
            "selector output was empty".to_string(),
        ));
    }

    Ok(selector.to_string())
}

pub fn parse_list_output(
    output: &str,
    session_name: Option<&str>,
) -> Result<Vec<ResolvedTarget>, AdapterError> {
    let panes: Vec<PaneRecord> = serde_json::from_str(output)
        .map_err(|error| AdapterError::ParseError(error.to_string()))?;

    Ok(panes
        .into_iter()
        .map(|pane| ResolvedTarget {
            selector: format!("id:{}", pane.id),
            pane_id: Some(pane.id),
            session_name: session_name.unwrap_or_default().to_string(),
            tab_name: pane.tab_name,
        })
        .collect())
}

pub fn parse_capture_output(output: &[u8]) -> String {
    String::from_utf8_lossy(output).into_owned()
}

#[derive(Debug, Deserialize)]
struct PaneRecord {
    id: String,
    tab_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{parse_capture_output, parse_list_output, parse_single_selector};

    #[test]
    fn trims_selector_output() {
        let selector = parse_single_selector("  id:terminal:7\n").expect("selector should parse");
        assert_eq!(selector, "id:terminal:7");
    }

    #[test]
    fn parses_list_output() {
        let targets = parse_list_output(
            r#"[
                {"id":"terminal:3","tab_name":"editor"},
                {"id":"terminal:4","tab_name":"logs"}
            ]"#,
            Some("gpu"),
        )
        .expect("list output should parse");

        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].selector, "id:terminal:3");
        assert_eq!(targets[0].session_name, "gpu");
        assert_eq!(targets[1].tab_name.as_deref(), Some("logs"));
    }

    #[test]
    fn decodes_capture_output_lossily() {
        let content = parse_capture_output(b"hello\nworld");
        assert_eq!(content, "hello\nworld");
    }
}
