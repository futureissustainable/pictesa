//! Presets: reusable prompt additions and parameter sets (camera moves, looks, effects).
//! Built-ins ship in `presets/presets.json`; users add their own `*.json` files to a preset folder.

use std::path::PathBuf;

use pictesa_core::MediaKind;

use crate::{Engine, Preset};

const BUILTIN: &str = include_str!("../../../presets/presets.json");

pub fn load(dirs: &[PathBuf]) -> Vec<Preset> {
    let mut out: Vec<Preset> = serde_json::from_str(BUILTIN).unwrap_or_else(|e| {
        tracing::error!("built-in presets are invalid: {e}");
        Vec::new()
    });
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str::<Vec<Preset>>(&t).map_err(|e| e.to_string())) {
                Ok(list) => {
                    for p in list {
                        out.retain(|q| q.id != p.id);
                        out.push(p);
                    }
                }
                Err(err) => tracing::warn!("skipping preset file {}: {err}", path.display()),
            }
        }
    }
    out
}

impl Engine {
    pub fn presets(&self, output: Option<MediaKind>) -> Vec<Preset> {
        self.inner.presets.iter().filter(|p| output.is_none_or(|o| p.output == o)).cloned().collect()
    }
    pub fn preset(&self, id: &str) -> Option<Preset> {
        self.inner.presets.iter().find(|p| p.id == id).cloned()
    }
}
