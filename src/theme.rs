use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{Arc, RwLock},
};

use crate::sass;
use crate::site::{load_config, SiteConfig};

#[derive(Clone)]
pub struct Theme {
    pub path: String,
    pub config: SiteConfig,
    pub resources: Arc<RwLock<HashMap<String, String>>>,
}

impl Theme {
    pub fn load_sass(&self) {
        let mut sass_path = PathBuf::from(&self.path);
        sass_path.push("sass/");
        if !sass_path.as_path().exists() {
            return;
        }

        let mut resources = self.resources.write().unwrap();

        for (k, v) in &sass::compile_sass(&sass_path) {
            println!("Loaded theme resource: {}", k);
            resources.insert(k.to_owned(), v.to_string());
        }
    }
}

pub fn load_themes() -> HashMap<String, Theme> {
    let paths = match fs::read_dir("./themes") {
        Ok(paths) => paths.map(|r| r.unwrap()).collect(),
        _ => vec![],
    };

    let mut themes = HashMap::new();
    for path in &paths {
        println!("Found theme: {}", path.file_name().to_str().unwrap());

        let theme_path = path.path().display().to_string();

        let config = load_config(&format!("{}/config.toml", theme_path));
        if config.is_none() {
            println!("No config for theme: {}. Skipping!", theme_path);
        }
        let config = config.unwrap();

        let theme = Theme {
            path: theme_path,
            config,
            resources: Arc::new(RwLock::new(HashMap::new())),
        };

        theme.load_sass();

        println!("Theme loaded!");

        themes.insert(path.file_name().to_str().unwrap().to_string(), theme);
    }

    println!("{} themes loaded!", themes.len());

    themes
}
