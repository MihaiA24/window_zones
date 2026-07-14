use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::actions::Binding;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub bindings: Vec<Binding>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("invalid TOML config: {0}")]
    Toml(#[from] toml::de::Error),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum BindingValidationError {
    #[error("malformed hotkey {raw}: {reason}")]
    MalformedHotkey { raw: String, reason: String },
    #[error("duplicate binding for hotkey {hotkey}")]
    DuplicateHotkey { hotkey: String },
}

pub fn parse_config(input: &str) -> Result<AppConfig, ConfigError> {
    Ok(toml::from_str(input)?)
}

pub fn validate_and_normalize_bindings(
    mut bindings: Vec<Binding>,
) -> Result<Vec<Binding>, BindingValidationError> {
    let mut seen = HashSet::new();

    for binding in bindings.iter_mut() {
        let normalized = normalize_hotkey(&binding.hotkey)?;
        if !seen.insert(normalized.clone()) {
            return Err(BindingValidationError::DuplicateHotkey {
                hotkey: normalized.clone(),
            });
        }
        binding.hotkey = normalized;
    }

    Ok(bindings)
}

pub fn normalize_hotkey(input: &str) -> Result<String, BindingValidationError> {
    let mut modifiers = Vec::new();
    let mut key = None;

    if input.trim().is_empty() {
        return Err(BindingValidationError::MalformedHotkey {
            raw: input.to_string(),
            reason: "empty hotkey".to_string(),
        });
    }

    for token in input.split('+') {
        let canonical = canonicalize_hotkey_token(token)?;
        if is_modifier_token(&canonical) {
            modifiers.push(canonical);
        } else if key.is_none() {
            key = Some(canonical);
        } else {
            return Err(BindingValidationError::MalformedHotkey {
                raw: input.to_string(),
                reason: "multiple non-modifier keys".to_string(),
            });
        }
    }

    let key = key.ok_or_else(|| BindingValidationError::MalformedHotkey {
        raw: input.to_string(),
        reason: "missing key".to_string(),
    })?;

    modifiers.sort_by_key(|token| modifier_sort_key(token.as_str()));
    modifiers.dedup();

    let mut normalized = String::new();
    if !modifiers.is_empty() {
        for modifier in modifiers {
            if !normalized.is_empty() {
                normalized.push('+');
            }
            normalized.push_str(&modifier);
        }
        normalized.push('+');
    }
    normalized.push_str(&key);
    Ok(normalized)
}

fn canonicalize_hotkey_token(raw: &str) -> Result<String, BindingValidationError> {
    let token = raw.trim();
    if token.is_empty() {
        return Err(BindingValidationError::MalformedHotkey {
            raw: raw.to_string(),
            reason: "empty hotkey token".to_string(),
        });
    }

    Ok(match token.to_ascii_lowercase().as_str() {
        "control" => "ctrl".to_string(),
        "option" => "alt".to_string(),
        "cmd" | "meta" | "super" | "windows" | "win" => "cmd".to_string(),
        token => token.to_string(),
    })
}

fn is_modifier_token(token: &str) -> bool {
    matches!(token, "alt" | "ctrl" | "shift" | "cmd")
}

fn modifier_sort_key(token: &str) -> u8 {
    match token {
        "alt" => 0,
        "ctrl" => 1,
        "shift" => 2,
        "cmd" => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Action, Binding, BuiltInZone};

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

    #[test]
    fn normalizes_hotkey_spacing_case_and_modifier_order() {
        let normalized = normalize_hotkey(" ctrl + Alt + Left ").unwrap();
        assert_eq!(normalized, "alt+ctrl+left");
    }

    #[test]
    fn validates_and_normalizes_bindings() {
        let config = vec![
            Binding {
                hotkey: " Ctrl + Alt + Left ".to_string(),
                action: Action::MoveToNextDisplay,
            },
            Binding {
                hotkey: "ctrl+alt+shift+right ".to_string(),
                action: Action::MoveToPreviousDisplay,
            },
        ];

        let validated = validate_and_normalize_bindings(config).unwrap();
        assert_eq!(validated[0].hotkey, "alt+ctrl+left");
        assert_eq!(validated[1].hotkey, "alt+ctrl+shift+right");
    }

    #[test]
    fn rejects_duplicate_hotkeys_after_normalization() {
        let err = validate_and_normalize_bindings(vec![
            Binding {
                hotkey: " Ctrl + Alt + Left ".to_string(),
                action: Action::MoveToNextDisplay,
            },
            Binding {
                hotkey: "ctrl+alt+left".to_string(),
                action: Action::MoveToPreviousDisplay,
            },
        ])
        .unwrap_err();

        assert_eq!(
            err,
            BindingValidationError::DuplicateHotkey {
                hotkey: "alt+ctrl+left".to_string()
            }
        );
    }

    #[test]
    fn rejects_malformed_hotkeys_on_validation() {
        let err = validate_and_normalize_bindings(vec![Binding {
            hotkey: "  +left ".to_string(),
            action: Action::MoveToNextDisplay,
        }])
        .unwrap_err();

        assert!(matches!(
            err,
            BindingValidationError::MalformedHotkey { .. }
        ));
    }
}
