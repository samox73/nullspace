const IDENTITY_VERSION: u32 = 1;

pub fn equation_identity(latex: &str) -> String {
    match ratex_parser::parser::parse(latex) {
        Ok(nodes) => {
            let mut value = serde_json::to_value(&nodes).unwrap_or(serde_json::Value::Null);
            strip_keys(&mut value, "loc");
            format!(
                "ast:v{IDENTITY_VERSION}:{}",
                serde_json::to_string(&value).unwrap_or_default()
            )
        }
        Err(_) => format!(
            "raw:v{IDENTITY_VERSION}:{}",
            latex.split_whitespace().collect::<Vec<_>>().join(" ")
        ),
    }
}

fn strip_keys(value: &mut serde_json::Value, key: &str) {
    match value {
        serde_json::Value::Object(map) => {
            map.remove(key);
            for value in map.values_mut() {
                strip_keys(value, key);
            }
        }
        serde_json::Value::Array(items) => {
            for value in items.iter_mut() {
                strip_keys(value, key);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::equation_identity;

    #[test]
    fn spaces_do_not_affect_parsed_identity() {
        assert_eq!(equation_identity("E=mc^2"), equation_identity("E = mc^2"));
    }

    #[test]
    fn control_word_termination_affects_identity() {
        assert_ne!(
            equation_identity("\\alpha beta"),
            equation_identity("\\alphabeta")
        );
    }

    #[test]
    fn text_mode_spaces_affect_identity() {
        assert_ne!(
            equation_identity("\\text{a b}"),
            equation_identity("\\text{ab}")
        );
    }

    #[test]
    fn parsed_identity_is_case_sensitive() {
        assert_ne!(equation_identity("\\Pi"), equation_identity("\\pi"));
    }

    #[test]
    fn identity_is_deterministic() {
        assert_eq!(
            equation_identity("\\frac{a}{b} + c"),
            equation_identity("\\frac{a}{b} + c")
        );
    }

    #[test]
    fn unparseable_latex_uses_raw_fallback() {
        assert!(equation_identity("\\frac{").starts_with("raw:v1:"));
    }
}
