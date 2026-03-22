use crate::adapters::zjctl::AdapterError;

#[cfg_attr(not(test), allow(dead_code))]
pub fn parse_single_selector(output: &str) -> Result<String, AdapterError> {
    let selector = output.trim();
    if selector.is_empty() {
        return Err(AdapterError::Unimplemented);
    }

    Ok(selector.to_string())
}

#[cfg(test)]
mod tests {
    use super::parse_single_selector;

    #[test]
    fn trims_selector_output() {
        let selector = parse_single_selector("  id:terminal:7\n").expect("selector should parse");
        assert_eq!(selector, "id:terminal:7");
    }
}
