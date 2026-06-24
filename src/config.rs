//! Parser for snapRAID configuration files used to discover array paths.

use std::fs;
use std::path::Path;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("could not read config {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapraidConfig {
    pub data: Vec<DataDiskConfig>,
    pub parity: Vec<String>,
    pub content: Vec<String>,
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDiskConfig {
    pub name: String,
    pub path: String,
}

pub fn read_config(path: &Path) -> Result<SnapraidConfig, ConfigError> {
    let text = fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.display().to_string(),
        source,
    })?;
    Ok(parse_config(&text))
}

pub fn parse_config(text: &str) -> SnapraidConfig {
    let mut config = SnapraidConfig::default();

    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        let mut parts = line.split_whitespace();
        let Some(key) = parts.next() else {
            continue;
        };

        match key {
            "data" => {
                if let (Some(name), Some(path)) = (parts.next(), parts.next()) {
                    config.data.push(DataDiskConfig {
                        name: name.to_owned(),
                        path: path.to_owned(),
                    });
                }
            }
            "content" => {
                if let Some(path) = parts.next() {
                    config.content.push(path.to_owned());
                }
            }
            "parity" | "q-parity" | "2-parity" | "3-parity" | "4-parity" | "5-parity"
            | "6-parity" => {
                if let Some(path) = parts.next() {
                    config.parity.push(path.to_owned());
                }
            }
            "exclude" => {
                let pattern = parts.collect::<Vec<_>>().join(" ");
                if !pattern.is_empty() {
                    config.exclude.push(pattern);
                }
            }
            _ => {}
        }
    }

    config
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_relevant_snapraid_config_lines() {
        let config = parse_config(
            r#"
                parity /mnt/parity/snapraid.parity
                2-parity /mnt/parity2/snapraid.2-parity
                content /var/snapraid.content
                data d1 /srv/disk1
                data d2 /srv/disk2 # inline comment
                exclude *.tmp
            "#,
        );

        assert_eq!(config.parity.len(), 2);
        assert_eq!(config.content, vec!["/var/snapraid.content"]);
        assert_eq!(config.exclude, vec!["*.tmp"]);
        assert_eq!(config.data[0].name, "d1");
        assert_eq!(config.data[1].path, "/srv/disk2");
    }
}
