use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::actions::Binding;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub bindings: Vec<Binding>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("invalid TOML config: {0}")]
    Toml(#[from] toml::de::Error),
}

pub fn parse_config(input: &str) -> Result<AppConfig, ConfigError> {
    Ok(toml::from_str(input)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Action, BuiltInZone};

    #[test]
    fn parses_bindings_with_opaque_hotkeys_and_kebab_case_actions() {
        let config = parse_config(
            r#"
[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-half" }

[[bindings]]
hotkey = "Ctrl+Alt+Shift+Right"
action = { type = "move-to-next-display" }
"#,
        )
        .unwrap();

        assert_eq!(config.bindings.len(), 2);
        assert_eq!(config.bindings[0].hotkey, "Ctrl+Alt+Left");
        assert_eq!(
            config.bindings[0].action,
            Action::MoveToZone {
                zone: BuiltInZone::LeftHalf
            }
        );
        assert_eq!(config.bindings[1].action, Action::MoveToNextDisplay);
    }

    #[test]
    fn defaults_missing_bindings_to_empty_config() {
        let config = parse_config("").unwrap();
        assert!(config.bindings.is_empty());
    }

    #[test]
    fn rejects_unknown_zone_names() {
        let err = parse_config(
            r#"
[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-quarter" }
"#,
        )
        .unwrap_err();

        assert!(err.to_string().contains("left-quarter"));
    }
}
