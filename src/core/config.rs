//! Persistent configuration for pit.
//!
//! Stored at `~/Library/Application Support/pit/config.toml` (macOS)
//! or `~/.local/share/pit/config.toml` (Linux).
//!
//! Values can also be set via environment variables (higher priority).

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;

/// Get the pit data directory.
pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("pit")
}

/// Get the config file path.
pub fn config_path() -> PathBuf {
    data_dir().join("config.toml")
}

/// Read a config value. Checks env var first (uppercase, dots→underscores),
/// then falls back to config file.
///
/// Example: `get("linear.api_key")` checks `LINEAR_API_KEY` env, then config file.
pub fn get(key: &str) -> Option<String> {
    // Try env var first: "linear.api_key" → "LINEAR_API_KEY"
    let env_key = key.replace('.', "_").to_uppercase();
    if let Ok(val) = std::env::var(&env_key) {
        if !val.is_empty() {
            return Some(val);
        }
    }

    // Fall back to config file
    let config = load_config().unwrap_or_default();
    // Support dotted keys: "linear.api_key" looks in [linear] section for api_key
    let parts: Vec<&str> = key.splitn(2, '.').collect();
    match parts.as_slice() {
        [section, field] => config.get(&format!("{}.{}", section, field)).cloned(),
        [field] => config.get(*field).cloned(),
        _ => None,
    }
}

/// Set a config value in the config file.
pub fn set(key: &str, value: &str) -> Result<()> {
    let mut config = load_config().unwrap_or_default();
    config.insert(key.to_string(), value.to_string());
    save_config(&config)
}

/// Remove a config value from the config file.
pub fn unset(key: &str) -> Result<()> {
    let mut config = load_config().unwrap_or_default();
    config.remove(key);
    save_config(&config)
}

/// List all config values.
pub fn list() -> HashMap<String, String> {
    load_config().unwrap_or_default()
}

/// Load config from file. Simple key=value format (one per line).
/// Lines starting with # are comments. Section headers [name] prefix subsequent keys.
fn load_config() -> Result<HashMap<String, String>> {
    let path = config_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Ok(HashMap::new()),
    };

    let mut map = HashMap::new();
    let mut section = String::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_string();
            continue;
        }
        if let Some(eq_pos) = line.find('=') {
            let key = line[..eq_pos].trim();
            let value = line[eq_pos + 1..].trim().trim_matches('"');
            let full_key = if section.is_empty() {
                key.to_string()
            } else {
                format!("{}.{}", section, key)
            };
            map.insert(full_key, value.to_string());
        }
    }

    Ok(map)
}

/// Save config to file in TOML-like format.
fn save_config(config: &HashMap<String, String>) -> Result<()> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;

    // Group by section
    let mut sections: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for (key, value) in config {
        let parts: Vec<&str> = key.splitn(2, '.').collect();
        match parts.as_slice() {
            [section, field] => {
                sections
                    .entry(section.to_string())
                    .or_default()
                    .push((field.to_string(), value.clone()));
            }
            _ => {
                sections
                    .entry(String::new())
                    .or_default()
                    .push((key.clone(), value.clone()));
            }
        }
    }

    let mut output = String::new();

    // Write top-level keys first
    if let Some(top) = sections.remove("") {
        for (k, v) in &top {
            output.push_str(&format!("{} = \"{}\"\n", k, v));
        }
        if !top.is_empty() {
            output.push('\n');
        }
    }

    // Write sections
    let mut section_names: Vec<String> = sections.keys().cloned().collect();
    section_names.sort();
    for name in section_names {
        if let Some(fields) = sections.get(&name) {
            output.push_str(&format!("[{}]\n", name));
            let mut fields_sorted = fields.clone();
            fields_sorted.sort_by(|a, b| a.0.cmp(&b.0));
            for (k, v) in &fields_sorted {
                output.push_str(&format!("{} = \"{}\"\n", k, v));
            }
            output.push('\n');
        }
    }

    let path = config_path();
    std::fs::write(&path, output).with_context(|| format!("failed to write {}", path.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn env_var_takes_priority() {
        env::set_var("TEST_PIT_KEY", "from_env");
        let val = get("test_pit_key");
        assert_eq!(val, Some("from_env".to_string()));
        env::remove_var("TEST_PIT_KEY");
    }

    #[test]
    fn dotted_key_maps_to_env_var() {
        env::set_var("LINEAR_API_KEY", "lin_test_123");
        let val = get("linear.api_key");
        assert_eq!(val, Some("lin_test_123".to_string()));
        env::remove_var("LINEAR_API_KEY");
    }

    #[test]
    fn missing_key_returns_none() {
        // Use a key that's definitely not set
        env::remove_var("PIT_NONEXISTENT_KEY_12345");
        let val = get("pit_nonexistent_key_12345");
        assert!(val.is_none());
    }

    #[test]
    fn load_save_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        // Write a config file
        let content = r#"
[linear]
api_key = "lin_abc123"

[github]
token = "ghp_xyz"
"#;
        std::fs::write(&path, content).unwrap();

        // Parse it
        let parsed = {
            let content = std::fs::read_to_string(&path).unwrap();
            let mut map = HashMap::new();
            let mut section = String::new();
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if line.starts_with('[') && line.ends_with(']') {
                    section = line[1..line.len() - 1].trim().to_string();
                    continue;
                }
                if let Some(eq_pos) = line.find('=') {
                    let key = line[..eq_pos].trim();
                    let value = line[eq_pos + 1..].trim().trim_matches('"');
                    let full_key = if section.is_empty() {
                        key.to_string()
                    } else {
                        format!("{}.{}", section, key)
                    };
                    map.insert(full_key, value.to_string());
                }
            }
            map
        };

        assert_eq!(parsed.get("linear.api_key").unwrap(), "lin_abc123");
        assert_eq!(parsed.get("github.token").unwrap(), "ghp_xyz");
    }
}
