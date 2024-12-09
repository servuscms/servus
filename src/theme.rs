use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{Arc, RwLock},
};
use tide::log;

use crate::sass;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ThemeConfig {
    #[serde(flatten)]
    pub extra: HashMap<String, toml::Value>,
}

pub fn load_config(config_path: &str) -> Option<ThemeConfig> {
    if let Ok(content) = fs::read_to_string(config_path) {
        Some(toml::from_str(&content).unwrap())
    } else {
        None
    }
}

pub struct Theme {
    pub path: String,
    pub config: ThemeConfig,
    pub resources: Arc<RwLock<HashMap<String, String>>>,
}

impl Theme {
    pub fn load_sass(&self) -> Result<(), String> {
        let mut sass_path = PathBuf::from(&self.path);
        sass_path.push("sass/");
        if !sass_path.as_path().exists() {
            return Ok(());
        }

        let mut resources = self.resources.write().unwrap();

        for (k, v) in &sass::compile_sass(&sass_path)? {
            log::debug!("Loaded theme resource: {}", k);
            resources.insert(k.to_owned(), v.to_string());
        }

        Ok(())
    }
}

pub fn load_themes() -> HashMap<String, Theme> {
    let paths = match fs::read_dir("./themes") {
        Ok(paths) => paths.map(|r| r.unwrap()).collect(),
        _ => vec![],
    };

    let mut themes = HashMap::new();
    for path in &paths {
        log::info!("Found theme: {}", path.file_name().to_str().unwrap());

        let theme_path = path.path().display().to_string();

        let config = load_config(&format!("{}/config.toml", theme_path));
        if config.is_none() {
            log::warn!("No config for theme: {}. Skipping!", theme_path);
            continue;
        }
        let config = config.unwrap();

        let theme = Theme {
            path: theme_path.clone(),
            config,
            resources: Arc::new(RwLock::new(HashMap::new())),
        };

        if let Err(e) = theme.load_sass() {
            log::warn!(
                "Failed to load sass for theme: {}. Skipping! Error: {}",
                theme_path,
                e
            )
        }

        log::debug!("Theme loaded: {}!", path.file_name().to_str().unwrap());

        themes.insert(path.file_name().to_str().unwrap().to_string(), theme);
    }

    log::info!("{} themes loaded!", themes.len());

    themes
}
