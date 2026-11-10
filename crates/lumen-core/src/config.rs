//! Persisted user preferences (`%APPDATA%\Lumen\config.json`).

use std::path::PathBuf;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// Where config and photos live when the platform has no `%APPDATA%`/`%USERPROFILE%` (Android sets it once at start-up).
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn set_data_dir(dir: PathBuf) {
    let _ = DATA_DIR.set(dir);
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Config {
    pub output_dir: PathBuf,
    pub prefix: String,
    pub bracket_shots: u32,
    pub bracket_step: u32,
    pub bracket_far_first: bool,
    pub settle_ms: u64,
    pub auto_stack: bool,
    pub stack_sharpest: bool,
    pub stack_smoothing: usize,
    pub focus_step_index: usize,
    pub show_grid: bool,
    pub show_peaking: bool,
    pub show_histogram: bool,
    pub show_osd: bool,
    pub audio_device: Option<String>,
    pub audio_listen: bool,
    pub audio_volume: f32,
    pub audio_auto_start: bool,
    pub peaking_threshold: f32,
    pub last_device: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        let output_dir = match DATA_DIR.get() {
            Some(dir) => dir.join("Pictures"),
            None => std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("Pictures").join("Lumen"),
        };
        Self {
            output_dir,
            prefix: "capture".into(),
            bracket_shots: 12,
            bracket_step: 60,
            bracket_far_first: false,
            settle_ms: 250,
            auto_stack: true,
            stack_sharpest: false,
            stack_smoothing: 3,
            focus_step_index: 1,
            show_grid: false,
            show_peaking: true,
            show_histogram: true,
            show_osd: true,
            audio_device: None,
            audio_listen: false,
            audio_volume: 0.8,
            audio_auto_start: false,
            peaking_threshold: 60.0,
            last_device: None,
        }
    }
}

fn path() -> Option<PathBuf> {
    if let Some(dir) = DATA_DIR.get() {
        return Some(dir.join("config.json"));
    }
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Lumen").join("config.json"))
}

impl Config {
    /// Load preferences, falling back to defaults on any problem.
    pub fn load() -> Self {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_config_falls_back_to_defaults() {
        let c: Config = serde_json::from_str(r#"{"prefix":"x","bracket_shots":3}"#).unwrap();
        assert_eq!(c.prefix, "x");
        assert_eq!(c.bracket_shots, 3);
        assert_eq!(c.bracket_step, Config::default().bracket_step);
    }
}
