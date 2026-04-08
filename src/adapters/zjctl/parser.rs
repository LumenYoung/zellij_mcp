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

#[cfg_attr(not(test), allow(dead_code))]
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
            title: pane.title,
            command: pane.command,
            focused: pane.focused,
        })
        .collect())
}

pub fn parse_capture_output(output: &[u8]) -> String {
    String::from_utf8_lossy(output).into_owned()
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn parse_spawn_output(
    output: &str,
    session_name: &str,
    tab_name: Option<&str>,
    title: Option<&str>,
) -> Result<ResolvedTarget, AdapterError> {
    let raw_selector = parse_single_selector(output)?;
    let pane_id = raw_selector
        .strip_prefix("id:")
        .unwrap_or(&raw_selector)
        .replace("terminal_", "terminal:")
        .replace("plugin_", "plugin:");
    let selector = format!("id:{pane_id}");

    Ok(ResolvedTarget {
        selector,
        pane_id: Some(pane_id),
        session_name: session_name.to_string(),
        tab_name: tab_name.map(ToOwned::to_owned),
        title: title.map(ToOwned::to_owned),
        command: None,
        focused: false,
    })
}

#[derive(Debug, Deserialize)]
#[cfg_attr(not(test), allow(dead_code))]
struct PaneRecord {
    id: String,
    tab_name: Option<String>,
    title: Option<String>,
    command: Option<String>,
    focused: bool,
}

#[cfg(test)]
mod tests {
    use super::{
        parse_capture_output, parse_list_output, parse_single_selector, parse_spawn_output,
    };

    #[test]
    fn trims_selector_output() {
        let selector = parse_single_selector("  id:terminal:7\n").expect("selector should parse");
        assert_eq!(selector, "id:terminal:7");
    }

    #[test]
    fn parses_list_output() {
        let targets = parse_list_output(
            r#"[
                {"id":"terminal:3","tab_name":"editor","title":"shell","command":"fish","focused":false},
                {"id":"terminal:4","tab_name":"logs","title":"logs","command":"tail -f app.log","focused":true}
            ]"#,
            Some("gpu"),
        )
        .expect("list output should parse");

        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].selector, "id:terminal:3");
        assert_eq!(targets[0].session_name, "gpu");
        assert!(!targets[0].focused);
        assert_eq!(targets[1].tab_name.as_deref(), Some("logs"));
    }

    #[test]
    fn decodes_capture_output_lossily() {
        let content = parse_capture_output(b"hello\nworld");
        assert_eq!(content, "hello\nworld");
    }

    #[test]
    fn parses_spawn_output() {
        let target = parse_spawn_output("id:terminal:9\n", "gpu", Some("editor"), Some("lg"))
            .expect("spawn output should parse");

        assert_eq!(target.selector, "id:terminal:9");
        assert_eq!(target.pane_id.as_deref(), Some("terminal:9"));
        assert_eq!(target.session_name, "gpu");
        assert_eq!(target.tab_name.as_deref(), Some("editor"));
        assert_eq!(target.title.as_deref(), Some("lg"));
        assert_eq!(target.command, None);
    }

    #[test]
    fn canonicalizes_raw_spawn_output_to_id_selector() {
        let target = parse_spawn_output("terminal:9\n", "gpu", Some("editor"), Some("lg"))
            .expect("spawn output should parse");

        assert_eq!(target.selector, "id:terminal:9");
        assert_eq!(target.pane_id.as_deref(), Some("terminal:9"));
    }

    #[test]
    fn canonicalizes_underscore_spawn_output_to_id_selector() {
        let target = parse_spawn_output("terminal_9\n", "gpu", Some("editor"), Some("lg"))
            .expect("spawn output should parse");

        assert_eq!(target.selector, "id:terminal:9");
        assert_eq!(target.pane_id.as_deref(), Some("terminal:9"));
    }
}
