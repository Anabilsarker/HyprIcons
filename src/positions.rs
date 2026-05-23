use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde_json::Value;
use tracing::{debug, warn};

fn positions_file() -> PathBuf {
    crate::config::config_dir().join("positions.json")
}

pub struct Positions {
    path: PathBuf,
    data: HashMap<String, (i32, i32)>,
}

impl Positions {
    pub fn new() -> Self {
        Self::with_path(positions_file())
    }

    pub fn with_path(path: PathBuf) -> Self {
        let mut p = Self {
            path,
            data: HashMap::new(),
        };
        p.load();
        p
    }

    pub fn load(&mut self) {
        if !self.path.exists() {
            return;
        }
        let txt = match fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) => {
                warn!("Failed to load positions: {}", e);
                return;
            }
        };
        let raw: Value = match serde_json::from_str(&txt) {
            Ok(v) => v,
            Err(e) => {
                warn!("Failed to load positions: {}", e);
                return;
            }
        };
        if let Value::Object(map) = raw {
            for (k, v) in map {
                if let Value::Array(arr) = v
                    && arr.len() == 2
                    && let (Some(x), Some(y)) = (arr[0].as_i64(), arr[1].as_i64())
                {
                    self.data.insert(k, (x as i32, y as i32));
                }
            }
            debug!(
                "Loaded {} positions from {}",
                self.data.len(),
                self.path.display()
            );
        }
    }

    pub fn save(&self) {
        if let Some(parent) = self.path.parent()
            && let Err(e) = fs::create_dir_all(parent)
        {
            warn!("Failed to save positions: {}", e);
            return;
        }
        let map: serde_json::Map<String, Value> = self
            .data
            .iter()
            .map(|(k, (x, y))| {
                (
                    k.clone(),
                    Value::Array(vec![Value::from(*x), Value::from(*y)]),
                )
            })
            .collect();
        match serde_json::to_string_pretty(&Value::Object(map)) {
            Ok(s) => {
                if let Err(e) = fs::write(&self.path, s) {
                    warn!("Failed to save positions: {}", e);
                }
            }
            Err(e) => warn!("Failed to save positions: {}", e),
        }
    }

    pub fn get(&self, name: &str) -> Option<(i32, i32)> {
        self.data.get(name).copied()
    }

    pub fn set(&mut self, name: &str, x: i32, y: i32) {
        self.data.insert(name.to_string(), (x, y));
    }

    pub fn remove(&mut self, name: &str) {
        self.data.remove(name);
    }

    pub fn clear(&mut self) {
        self.data.clear();
    }

    pub fn prune(&mut self, valid_names: &[String]) {
        let keep: std::collections::HashSet<&String> = valid_names.iter().collect();
        self.data.retain(|k, _| keep.contains(k));
    }
}

impl Default for Positions {
    fn default() -> Self {
        Self::new()
    }
}
