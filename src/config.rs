use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::actions::{Action, Binding};
use crate::zones::{ZoneDefinition, built_in_zone_from_name};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub bindings: Vec<Binding>,
    #[serde(default)]
    pub zones: BTreeMap<String, ZoneDefinition>,
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
    #[error("empty zone name")]
    EmptyZoneName,
    #[error("zone name {zone} conflicts with built-in zone name")]
    BuiltInZoneConflict { zone: String },
    #[error("duplicate zone name {zone}")]
    DuplicateZoneName { zone: String },
    #[error("invalid zone definition for {zone}: {reason}")]
    InvalidZoneDefinition { zone: String, reason: String },
    #[error("unknown zone {zone}")]
    UnknownZone { zone: String },
}

pub fn parse_config(input: &str) -> Result<AppConfig, ConfigError> {
    Ok(toml::from_str(input)?)
}

pub fn validate_and_normalize_app_config(
    mut config: AppConfig,
) -> Result<AppConfig, BindingValidationError> {
    let mut normalized_zones = BTreeMap::new();
    for (name, zone) in std::mem::take(&mut config.zones) {
        let normalized_name = normalize_zone_name_name(&name)?;
        if built_in_zone_from_name(&normalized_name).is_some() {
            return Err(BindingValidationError::BuiltInZoneConflict {
                zone: normalized_name,
            });
        }

        validate_zone_definition(&normalized_name, &zone)?;

        if normalized_zones
            .insert(normalized_name.clone(), zone)
            .is_some()
        {
            return Err(BindingValidationError::DuplicateZoneName {
                zone: normalized_name,
            });
        }
    }

    let mut bindings = validate_and_normalize_bindings(std::mem::take(&mut config.bindings))?;

    for binding in bindings.iter_mut() {
        if let Action::MoveToZone { zone } = &mut binding.action {
            let normalized_zone = normalize_zone_name_name(zone)?;

            if built_in_zone_from_name(&normalized_zone).is_none()
                && !normalized_zones.contains_key(&normalized_zone)
            {
                return Err(BindingValidationError::UnknownZone {
                    zone: normalized_zone,
                });
            }

            *zone = normalized_zone;
        }
    }

    Ok(AppConfig {
        bindings,
        zones: normalized_zones,
    })
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

fn normalize_zone_name_name(raw: &str) -> Result<String, BindingValidationError> {
    let normalized = raw.trim().to_ascii_lowercase();

    if normalized.is_empty() {
        return Err(BindingValidationError::EmptyZoneName);
    }

    Ok(normalized)
}

fn validate_zone_definition(
    name: &str,
    zone: &ZoneDefinition,
) -> Result<(), BindingValidationError> {
    if zone.width == 0 {
        return Err(BindingValidationError::InvalidZoneDefinition {
            zone: name.to_string(),
            reason: "width must be greater than 0".to_string(),
        });
    }

    if zone.height == 0 {
        return Err(BindingValidationError::InvalidZoneDefinition {
            zone: name.to_string(),
            reason: "height must be greater than 0".to_string(),
        });
    }

    if zone.x > 100 {
        return Err(BindingValidationError::InvalidZoneDefinition {
            zone: name.to_string(),
            reason: "x must be in 0..=100".to_string(),
        });
    }

    if zone.y > 100 {
        return Err(BindingValidationError::InvalidZoneDefinition {
            zone: name.to_string(),
            reason: "y must be in 0..=100".to_string(),
        });
    }

    if zone.width > 100 {
        return Err(BindingValidationError::InvalidZoneDefinition {
            zone: name.to_string(),
            reason: "width must be in 1..=100".to_string(),
        });
    }

    if zone.height > 100 {
        return Err(BindingValidationError::InvalidZoneDefinition {
            zone: name.to_string(),
            reason: "height must be in 1..=100".to_string(),
        });
    }

    if zone.x.saturating_add(zone.width) > 100 {
        return Err(BindingValidationError::InvalidZoneDefinition {
            zone: name.to_string(),
            reason: "x + width must be <= 100".to_string(),
        });
    }

    if zone.y.saturating_add(zone.height) > 100 {
        return Err(BindingValidationError::InvalidZoneDefinition {
            zone: name.to_string(),
            reason: "y + height must be <= 100".to_string(),
        });
    }

    Ok(())
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
    use crate::{Action, Binding};

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
                zone: "left-half".to_string()
            }
        );
        assert_eq!(config.bindings[1].action, Action::MoveToNextDisplay);
    }

    #[test]
    fn parses_unknown_zone_names_as_raw_values() {
        let config = parse_config(
            r#"
[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-quarter" }
"#,
        )
        .unwrap();

        assert_eq!(
            config.bindings[0].action,
            Action::MoveToZone {
                zone: "left-quarter".to_string()
            }
        );
    }

    #[test]
    fn parses_and_validates_custom_zones() {
        let config = parse_config(
            r#"
[zones]

a = { x = 10, y = 0, width = 40, height = 100 }

[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "a" }
"#,
        )
        .unwrap();
        let config = validate_and_normalize_app_config(config).unwrap();

        assert_eq!(config.zones["a"].height, 100);
        assert_eq!(
            config.bindings[0].action,
            Action::MoveToZone {
                zone: "a".to_string()
            }
        );
    }

    #[test]
    fn rejects_unknown_zone_names_on_validation() {
        let config = parse_config(
            r#"
[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-quarter" }
"#,
        )
        .unwrap();

        let err = validate_and_normalize_app_config(config).unwrap_err();
        assert!(matches!(err, BindingValidationError::UnknownZone { .. }));
    }

    #[test]
    fn rejects_duplicate_zone_names_with_builtin_collisions() {
        let config = parse_config(
            r#"
[zones]
left-half = { x = 0, y = 0, width = 50, height = 100 }

[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-half" }
"#,
        )
        .unwrap();

        let err = validate_and_normalize_app_config(config).unwrap_err();
        assert!(matches!(
            err,
            BindingValidationError::BuiltInZoneConflict { zone } if zone == "left-half"
        ));
    }

    #[test]
    fn rejects_invalid_custom_zone_geometry() {
        let config = parse_config(
            r#"
[zones]
a = { x = 90, y = 0, width = 20, height = 100 }

[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "a" }
"#,
        )
        .unwrap();

        let err = validate_and_normalize_app_config(config).unwrap_err();
        assert!(matches!(
            err,
            BindingValidationError::InvalidZoneDefinition { zone, .. } if zone == "a"
        ));
    }

    #[test]
    fn defaults_missing_bindings_to_empty_config() {
        let config = parse_config("").unwrap();
        assert!(config.bindings.is_empty());
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
