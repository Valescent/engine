//! Startup configuration from `config.toml`.

use std::fs;
use std::io;
use std::path::Path;

use thiserror::Error;

/// Default render distance in chunk columns.
pub(crate) const DEFAULT_RENDER_DISTANCE: i32 = 12;

/// Smallest accepted render distance.
pub(crate) const MIN_RENDER_DISTANCE: i32 = 8;

/// Largest accepted render distance.
pub(crate) const MAX_RENDER_DISTANCE: i32 = 32;

/// Engine settings read once at startup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Config {
    pub(crate) render_distance_chunks: i32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            render_distance_chunks: DEFAULT_RENDER_DISTANCE,
        }
    }
}

/// Configuration failures.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read the config file: {0}")]
    Io(#[from] io::Error),
    #[error("unsupported config section: [{0}]")]
    UnknownSection(String),
    #[error("unsupported config key: {0}")]
    UnknownKey(String),
    #[error("invalid value for {key}: {value}")]
    InvalidValue { key: String, value: String },
    #[error("render distance {0} is outside {MIN_RENDER_DISTANCE}..={MAX_RENDER_DISTANCE}")]
    RenderDistanceOutOfRange(i32),
}

/// Reads `config.toml`, or defaults when the file is missing.
///
/// Only a `[world]` table with `render_distance_chunks` is accepted.
pub(crate) fn load(path: &Path) -> Result<Config, ConfigError> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(error) => return Err(ConfigError::Io(error)),
    };

    let mut config = Config::default();
    let mut in_world = false;
    let mut seen_render_distance = false;

    for line in contents.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            let section = section.trim();
            if section != "world" {
                return Err(ConfigError::UnknownSection(section.to_owned()));
            }
            in_world = true;
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            return Err(ConfigError::InvalidValue {
                key: line.to_owned(),
                value: String::new(),
            });
        };
        let key = key.trim();
        let value = value.trim();
        if !in_world || key != "render_distance_chunks" {
            return Err(ConfigError::UnknownKey(key.to_owned()));
        }
        if seen_render_distance {
            return Err(ConfigError::UnknownKey(key.to_owned()));
        }
        seen_render_distance = true;

        let distance: i32 = value.parse().map_err(|_| ConfigError::InvalidValue {
            key: key.to_owned(),
            value: value.to_owned(),
        })?;
        if !(MIN_RENDER_DISTANCE..=MAX_RENDER_DISTANCE).contains(&distance) {
            return Err(ConfigError::RenderDistanceOutOfRange(distance));
        }
        config.render_distance_chunks = distance;
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let directory = std::env::temp_dir().join(format!(
            "valescent-config-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn load_text(text: &str) -> Result<Config, ConfigError> {
        let directory = temp_dir("read");
        let path = directory.join("config.toml");
        fs::write(&path, text).unwrap();
        let result = load(&path);
        fs::remove_dir_all(directory).unwrap();
        result
    }

    #[test]
    fn a_missing_file_uses_defaults() {
        let directory = temp_dir("missing");
        let config = load(&directory.join("config.toml")).unwrap();
        assert_eq!(config, Config::default());
        assert_eq!(config.render_distance_chunks, DEFAULT_RENDER_DISTANCE);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn parses_the_world_table() {
        let config = load_text("[world]\nrender_distance_chunks = 8\n").unwrap();
        assert_eq!(config.render_distance_chunks, 8);

        let config = load_text("[world]\nrender_distance_chunks = 32\n").unwrap();
        assert_eq!(config.render_distance_chunks, 32);
    }

    #[test]
    fn ignores_comments_and_blank_lines() {
        let config =
            load_text("# comment\n\n[world]\n# distance\nrender_distance_chunks = 16\n").unwrap();
        assert_eq!(config.render_distance_chunks, 16);
    }

    #[test]
    fn rejects_unknown_sections_and_keys() {
        assert!(matches!(
            load_text("[graphics]\nrender_distance_chunks = 12\n"),
            Err(ConfigError::UnknownSection(section)) if section == "graphics"
        ));
        assert!(matches!(
            load_text("[world]\nrender_distance_chunks = 12\nseed = 1\n"),
            Err(ConfigError::UnknownKey(key)) if key == "seed"
        ));
        assert!(matches!(
            load_text("render_distance_chunks = 12\n"),
            Err(ConfigError::UnknownKey(key)) if key == "render_distance_chunks"
        ));
    }

    #[test]
    fn rejects_out_of_range_and_malformed_values() {
        assert!(matches!(
            load_text("[world]\nrender_distance_chunks = 7\n"),
            Err(ConfigError::RenderDistanceOutOfRange(7))
        ));
        assert!(matches!(
            load_text("[world]\nrender_distance_chunks = 33\n"),
            Err(ConfigError::RenderDistanceOutOfRange(33))
        ));
        assert!(matches!(
            load_text("[world]\nrender_distance_chunks = twelve\n"),
            Err(ConfigError::InvalidValue { .. })
        ));
        assert!(matches!(
            load_text("[world]\nrender_distance_chunks\n"),
            Err(ConfigError::InvalidValue { .. })
        ));
    }

    #[test]
    fn rejects_duplicate_keys() {
        assert!(matches!(
            load_text("[world]\nrender_distance_chunks = 8\nrender_distance_chunks = 9\n"),
            Err(ConfigError::UnknownKey(_))
        ));
    }
}
