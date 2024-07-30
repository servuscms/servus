use chrono::{NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    fs::File,
    io::BufReader,
    path::PathBuf,
    str,
    sync::{Arc, RwLock},
};
use tide::log;
use walkdir::WalkDir;

const DEFAULT_THEME: &str = "hyde";

use crate::{
    content, nostr,
    resource::{ContentSource, Resource, ResourceKind},
    template,
};

#[derive(Clone, Serialize, Deserialize)]
pub struct ServusMetadata {
    pub version: String,
}

#[derive(Clone)]
pub struct Site {
    pub path: String,
    pub config: SiteConfig,
    pub data: Arc<RwLock<HashMap<String, serde_yaml::Value>>>,
    pub events: Arc<RwLock<HashMap<String, EventRef>>>,
    pub resources: Arc<RwLock<HashMap<String, Resource>>>,
    pub tera: Arc<RwLock<tera::Tera>>, // TODO: try to move this to Theme
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SiteConfig {
    pub base_url: String,
    pub pubkey: Option<String>,

    pub theme: Option<String>,
    pub title: Option<String>,

    pub extra: HashMap<String, toml::Value>,
}

impl SiteConfig {
    // https://github.com/getzola/zola/blob/master/components/config/src/config/mod.rs

    /// Makes a url, taking into account that the base url might have a trailing slash
    pub fn make_permalink(&self, path: &str) -> String {
        let trailing_bit = if path.ends_with('/') || path.ends_with("atom.xml") || path.is_empty() {
            ""
        } else {
            "/"
        };

        // Index section with a base url that has a trailing slash
        if self.base_url.ends_with('/') && path == "/" {
            self.base_url.to_string()
        } else if path == "/" {
            // index section with a base url that doesn't have a trailing slash
            format!("{}/", self.base_url)
        } else if self.base_url.ends_with('/') && path.starts_with('/') {
            format!("{}{}{}", self.base_url, &path[1..], trailing_bit)
        } else if self.base_url.ends_with('/') || path.starts_with('/') {
            format!("{}{}{}", self.base_url, path, trailing_bit)
        } else {
            format!("{}/{}{}", self.base_url, path, trailing_bit)
        }
    }
}

fn load_templates(site_config: &SiteConfig) -> tera::Tera {
    println!("Loading templates...");

    let theme_path = format!("./themes/{}", site_config.theme.as_ref().unwrap());

    let mut tera = tera::Tera::new(&format!("{}/templates/**/*", theme_path)).unwrap();
    tera.autoescape_on(vec![]);
    tera.register_function("get_url", template::GetUrl::new(site_config.clone()));

    println!("Loaded {} templates!", tera.get_template_names().count());

    tera
}

impl Site {
    pub fn load_resources(&self) {
        let mut root = PathBuf::from(&self.path);
        root.push("_content/");
        if !root.as_path().exists() {
            return;
        }
        for entry in WalkDir::new(&root) {
            let path = entry.unwrap().into_path();
            if !path.is_file() {
                continue;
            }
            let relative_path = path.strip_prefix(&root).unwrap();
            if relative_path.starts_with("files/") {
                continue;
            }
            println!("Scanning file {}...", path.display());
            let file = File::open(&path).unwrap();
            let mut reader = BufReader::new(file);
            let filename = path.to_str().unwrap().to_string();
            let (front_matter, content) = content::read(&mut reader).unwrap();
            let mut kind: Option<ResourceKind> = None;
            let mut title: Option<String> = None;
            let mut date: Option<NaiveDateTime> = None;
            let mut slug: Option<String> = None;
            let content_source: ContentSource;
            if let Some(event) = nostr::parse_event(&front_matter, &content) {
                println!("Event: id={}.", &event.id);
                let event_ref = EventRef {
                    id: event.id.to_owned(),
                    kind: event.kind,
                    d_tag: event.get_d_tag(),
                    filename,
                };
                let mut events = self.events.write().unwrap();
                events.insert(event.id.to_owned(), event_ref.clone());

                kind = get_resource_kind(&event);
                if kind.is_some() {
                    title = event.get_tags_hash().get("title").cloned();
                    if title.is_none() && front_matter.contains_key("title") {
                        title = Some(
                            front_matter
                                .get("title")
                                .unwrap()
                                .as_str()
                                .unwrap()
                                .to_string(),
                        );
                    };
                    date = Some(event.get_date());
                    if let Some(long_form_slug) = event.get_d_tag() {
                        slug = Some(long_form_slug);
                    } else {
                        slug = Some(event.id);
                    }
                }

                content_source = ContentSource::Event(event_ref.id.to_owned());
            } else {
                let file_stem = relative_path.file_stem().unwrap().to_str().unwrap();
                // TODO: extract path patterns from config
                if relative_path.starts_with("data") {
                    println!("Data: id={}.", file_stem);
                    let data: serde_yaml::Value = serde_yaml::from_str(&content).unwrap();
                    let mut site_data = self.data.write().unwrap();
                    site_data.insert(file_stem.to_string(), data);
                } else if relative_path.starts_with("posts") {
                    let date_part = &file_stem[0..10];
                    if let Ok(d) = NaiveDate::parse_from_str(date_part, "%Y-%m-%d") {
                        if front_matter.contains_key("title") {
                            kind = Some(ResourceKind::Post);
                            let midnight = NaiveTime::from_hms_opt(0, 0, 0).unwrap();
                            title = Some(
                                front_matter
                                    .get("title")
                                    .unwrap()
                                    .as_str()
                                    .unwrap()
                                    .to_string(),
                            );
                            date = Some(NaiveDateTime::new(d, midnight));
                            slug = Some(file_stem[11..].to_owned());
                        } else {
                            println!("Post missing title: {}", file_stem);
                        }
                    } else {
                        println!("Cannot parse post date from filename: {}", file_stem);
                    };
                } else if relative_path.starts_with("pages") {
                    if front_matter.contains_key("title") {
                        kind = Some(ResourceKind::Page);
                        slug = Some(file_stem.to_owned());
                        title = Some(
                            front_matter
                                .get("title")
                                .unwrap()
                                .as_str()
                                .unwrap()
                                .to_string(),
                        );
                    } else {
                        println!("Page missing title: {}", file_stem);
                    }
                } else if relative_path.starts_with("notes") {
                    kind = Some(ResourceKind::Note);
                    date = front_matter.get("created_at").map(|c| {
                        Utc.timestamp_opt(c.as_i64().unwrap(), 0)
                            .unwrap()
                            .naive_utc()
                    });
                    slug = Some(file_stem.to_owned());
                }

                content_source = ContentSource::File(filename);
            }
            if let (Some(kind), Some(slug)) = (kind, slug) {
                let resource = Resource {
                    kind,
                    title,
                    date,
                    slug,
                    content_source,
                };
                if let Some(url) = resource.get_resource_url() {
                    println!("Resource: url={}.", &url);
                    let mut resources = self.resources.write().unwrap();
                    resources.insert(url, resource);
                }
            }
        }
    }

    fn get_path(
        &self,
        event_kind: i64,
        resource_kind: &Option<ResourceKind>,
        event_id: &str,
        event_d_tag: Option<String>,
    ) -> Option<String> {
        // TODO: read all this from config
        let mut path = PathBuf::from(&self.path);
        path.push("_content/");
        path.push(match (event_kind, resource_kind) {
            (nostr::EVENT_KIND_CUSTOM_DATA, _) => format!("data/{}.md", event_d_tag.unwrap()),
            (_, Some(ResourceKind::Post)) => format!("posts/{}.md", event_d_tag.unwrap()),
            (_, Some(ResourceKind::Page)) => format!("pages/{}.md", event_d_tag.unwrap()),
            (_, Some(ResourceKind::Note)) => format!("notes/{}.md", event_id),
            _ => return None,
        });

        Some(path.display().to_string())
    }

    pub fn add_content(&self, event: &nostr::Event) {
        let event_d_tag = event.get_d_tag();
        let kind = get_resource_kind(event);
        let slug = if event.is_long_form() {
            event_d_tag.to_owned().unwrap()
        } else {
            event.id.to_owned()
        };

        let filename = self
            .get_path(event.kind, &kind, &event.id, event_d_tag.clone())
            .unwrap();
        event.write(&filename).unwrap();
        let event_ref = EventRef {
            id: event.id.to_owned(),
            kind: event.kind,
            d_tag: event_d_tag.to_owned(),
            filename,
        };

        let mut events = self.events.write().unwrap();

        let mut matched_event_id: Option<String> = None;
        {
            for event_ref in events.values() {
                if event_d_tag.is_some() {
                    if event_ref.d_tag == event_d_tag {
                        matched_event_id = Some(event_ref.id.to_owned());
                    }
                }
            }
        }

        if let Some(matched_event_id) = matched_event_id {
            log::info!("Removing (outdated) event: {}!", &matched_event_id);
            events.remove(&matched_event_id);
        }

        events.insert(event.id.to_owned(), event_ref.clone());

        if let Some(kind) = kind {
            let resource = Resource {
                kind,
                title: event.get_tags_hash().get("title").cloned(),
                date: event.get_long_form_published_at(),
                slug,
                content_source: ContentSource::Event(event.id.to_owned()),
            };

            if let Some(url) = resource.get_resource_url() {
                // but not all posts have an URL (drafts don't)
                let mut resources = self.resources.write().unwrap();
                resources.insert(url.to_owned(), resource);
            }
        }
    }

    pub fn remove_content(&self, deletion_event: &nostr::Event) -> bool {
        let mut deleted_event_id: Option<String> = None;
        let mut deleted_event_kind: Option<i64> = None;
        let mut deleted_event_d_tag: Option<String> = None;
        for tag in &deletion_event.tags {
            if tag[0] == "e" {
                deleted_event_id = Some(tag[1].to_owned());
                log::debug!("DELETE 'e' {}", tag[1]);
            }
            if tag[0] == "a" {
                let deleted_event_ref = tag[1].to_owned();
                let parts = deleted_event_ref.split(':').collect::<Vec<_>>();
                if parts.len() == 3 {
                    if parts[1] != deletion_event.pubkey {
                        // TODO: do we need to check the site owner here?
                        return false;
                    }
                    deleted_event_kind = Some(parts[0].parse::<i64>().unwrap());
                    deleted_event_d_tag = Some(parts[2].to_owned());
                    log::debug!("DELETE 'a' {}", deleted_event_ref);
                }
            }
        }

        let mut resource_url: Option<String> = None;
        let mut resource_kind: Option<ResourceKind> = None;
        {
            let resources = self.resources.read().unwrap();
            for (url, resource) in &*resources {
                if let ContentSource::Event(event_id) = resource.content_source.clone() {
                    let mut matched_resource = false;

                    if deleted_event_kind.is_some() && deleted_event_d_tag.is_some() {
                        let events = self.events.read().unwrap();
                        let event_ref = events.get(&event_id).unwrap();
                        if event_ref.kind == deleted_event_kind.unwrap()
                            && event_ref.d_tag == deleted_event_d_tag
                        {
                            matched_resource = true;
                        }
                    } else if deleted_event_id.is_some() {
                        if Some(event_id) == deleted_event_id {
                            matched_resource = true;
                        }
                    }

                    if matched_resource {
                        resource_url = Some(url.to_owned());
                        resource_kind = Some(resource.kind);
                    }
                }
            }
        }

        let mut matched_event_id: Option<String> = None;
        let mut path: Option<String> = None;
        {
            let events = self.events.read().unwrap();
            for (event_id, event_ref) in &*events {
                let mut matched_event = false;
                if deleted_event_kind.is_some() && deleted_event_d_tag.is_some() {
                    if event_ref.kind == deleted_event_kind.unwrap()
                        && event_ref.d_tag == deleted_event_d_tag
                    {
                        matched_event = true;
                    }
                } else if deleted_event_id.is_some() {
                    if event_id == &deleted_event_id.clone().unwrap() {
                        matched_event = true;
                    }
                }

                if matched_event {
                    matched_event_id = Some(event_ref.id.to_owned());
                    path = self.get_path(
                        event_ref.kind,
                        &resource_kind,
                        event_id,
                        event_ref.d_tag.clone(),
                    );
                }
            }
        }

        if let Some(resource_url) = resource_url {
            log::info!("Removing resource: {}!", &resource_url);
            self.resources.write().unwrap().remove(&resource_url);
        }

        if let Some(matched_event_id) = matched_event_id {
            log::info!("Removing event: {}!", &matched_event_id);
            self.events.write().unwrap().remove(&matched_event_id);
        }

        if let Some(path) = path {
            log::info!("Removing file: {}!", &path);
            fs::remove_file(path).is_ok()
        } else {
            log::info!("No file for this resource!");
            false
        }
    }
}

#[derive(Clone, Serialize)]
pub struct EventRef {
    pub id: String,
    pub kind: i64,
    pub d_tag: Option<String>,

    pub filename: String,
}

impl EventRef {
    pub fn read(&self) -> Option<(HashMap<String, serde_yaml::Value>, String)> {
        let file = File::open(&self.filename).unwrap();
        let mut reader = BufReader::new(file);

        content::read(&mut reader)
    }
}

pub fn load_config(config_path: &str) -> Option<SiteConfig> {
    if let Ok(content) = fs::read_to_string(config_path) {
        Some(toml::from_str(&content).unwrap())
    } else {
        None
    }
}

pub fn load_sites() -> HashMap<String, Site> {
    let paths = match fs::read_dir("./sites") {
        Ok(paths) => paths.map(|r| r.unwrap()).collect(),
        _ => vec![],
    };

    let mut sites = HashMap::new();
    for path in &paths {
        println!("Found site: {}", path.file_name().to_str().unwrap());

        let site_path = path.path().display().to_string();

        let config = load_config(&format!("{}/_config.toml", site_path));
        if config.is_none() {
            println!("No site config for site: {}. Skipping!", site_path);
        }

        let mut config = config.unwrap();

        let theme_path = format!("./themes/{}", config.theme.as_ref().unwrap());
        let theme_config = load_config(&&format!("{}/config.toml", theme_path));

        config.extra = theme_config.unwrap().extra; // TODO: merge rather than overwrite!

        let tera = load_templates(&config);

        let site = Site {
            config,
            path: site_path,
            data: Arc::new(RwLock::new(HashMap::new())),
            events: Arc::new(RwLock::new(HashMap::new())),
            resources: Arc::new(RwLock::new(HashMap::new())),
            tera: Arc::new(RwLock::new(tera)),
        };

        site.load_resources();

        println!("Site loaded!");

        sites.insert(path.file_name().to_str().unwrap().to_string(), site);
    }

    println!("{} sites loaded!", sites.len());

    sites
}

pub fn create_site(domain: &str, admin_pubkey: Option<String>) -> Site {
    let path = format!("./sites/{}", domain);
    fs::create_dir_all(&path).unwrap();

    let config_content = format!(
        "pubkey = \"{}\"\nbase_url = \"https://{}\"\ntitle = \"{}\"\ntheme = \"{}\"\n[extra]\n",
        admin_pubkey.unwrap_or("".to_string()),
        domain,
        "",
        DEFAULT_THEME
    );
    fs::write(format!("./sites/{}/_config.toml", domain), &config_content).unwrap();

    let config = load_config(&format!("{}/_config.toml", path)).unwrap();

    let tera = load_templates(&config);

    let site = Site {
        config,
        path,
        data: Arc::new(RwLock::new(HashMap::new())),
        events: Arc::new(RwLock::new(HashMap::new())),
        resources: Arc::new(RwLock::new(HashMap::new())),
        tera: Arc::new(RwLock::new(tera)),
    };

    site.load_resources();

    site
}

fn get_resource_kind(event: &nostr::Event) -> Option<ResourceKind> {
    let date = event.get_long_form_published_at();
    match event.kind {
        nostr::EVENT_KIND_LONG_FORM | nostr::EVENT_KIND_LONG_FORM_DRAFT => {
            if date.is_some() {
                Some(ResourceKind::Post)
            } else {
                Some(ResourceKind::Page)
            }
        }
        nostr::EVENT_KIND_NOTE => Some(ResourceKind::Note),
        _ => None,
    }
}
