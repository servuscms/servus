use chrono::{NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use http_types::mime;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    env, fs,
    fs::File,
    io::BufReader,
    path::PathBuf,
    str,
    sync::{Arc, RwLock},
};
use tide::log;
use walkdir::WalkDir;

use crate::{content, nostr};

mod default_theme {
    include!(concat!(env!("OUT_DIR"), "/default_theme.rs"));
}

#[derive(Clone, Serialize, Deserialize)]
struct ServusMetadata {
    version: String,
}

#[derive(Clone, Serialize)]
struct PageTemplateContext<TagType> {
    url: String,
    slug: String,
    summary: Option<String>,
    inner_html: String,
    date: Option<NaiveDateTime>,
    #[serde(flatten)]
    tags: HashMap<String, TagType>,
}

#[derive(Clone)]
pub struct Site {
    pub path: String,
    pub config: toml::Value,
    pub data: Arc<RwLock<HashMap<String, serde_yaml::Value>>>,
    pub events: Arc<RwLock<HashMap<String, EventRef>>>,
    pub resources: Arc<RwLock<HashMap<String, Resource>>>,
    pub tera: Arc<RwLock<tera::Tera>>,
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
                if let Some(url) = resource.get_resource_url(&self.config) {
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
        events.insert(event.id.to_owned(), event_ref.clone());

        if let Some(kind) = kind {
            let resource = Resource {
                kind,
                title: event.get_tags_hash().get("title").cloned(),
                date: event.get_long_form_published_at(),
                slug,
                content_source: ContentSource::Event(event.id.to_owned()),
            };

            if let Some(url) = resource.get_resource_url(&self.config) {
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

#[derive(Clone, Copy, PartialEq, Serialize)]
pub enum ResourceKind {
    Post,
    Page,
    Note,
}

#[derive(Clone, Serialize)]
pub struct EventRef {
    pub id: String,
    pub kind: i64,
    pub d_tag: Option<String>,

    pub filename: String,
}

#[derive(Clone, Serialize)]
pub enum ContentSource {
    Event(String),
    File(String),
}

#[derive(Clone, Serialize)]
pub struct Resource {
    pub kind: ResourceKind,
    pub slug: String,

    pub title: Option<String>,
    pub date: Option<NaiveDateTime>,

    pub content_source: ContentSource,
}

impl EventRef {
    pub fn read(&self) -> Option<(HashMap<String, serde_yaml::Value>, String)> {
        let file = File::open(&self.filename).unwrap();
        let mut reader = BufReader::new(file);

        content::read(&mut reader)
    }
}

impl Resource {
    pub fn read(&self, site: &Site) -> Option<(HashMap<String, serde_yaml::Value>, String)> {
        let filename = match self.content_source.clone() {
            ContentSource::File(f) => f,
            ContentSource::Event(e_id) => {
                let events = site.events.read().unwrap();
                let event_ref = events.get(&e_id).unwrap();
                event_ref.filename.to_owned()
            }
        };
        let file = File::open(filename).unwrap();
        let mut reader = BufReader::new(file);

        content::read(&mut reader)
    }

    pub fn get_resource_url(&self, site_config: &toml::Value) -> Option<String> {
        // TODO: extract all URL patterns from config!
        match self.kind {
            ResourceKind::Post => {
                return Some(site_config.get("post_permalink").map_or_else(
                    || format!("/posts/{}", &self.slug),
                    |p| p.as_str().unwrap().replace(":slug", &self.slug),
                ));
            }
            ResourceKind::Page => Some(format!("/{}", &self.clone().slug)),
            ResourceKind::Note => Some(format!("/notes/{}", &self.clone().slug)),
        }
    }

    pub fn render(&self, site: &Site) -> Vec<u8> {
        let (front_matter, content) = self.read(site).unwrap();

        match self.kind {
            ResourceKind::Page | ResourceKind::Post => {
                let mut tera = site.tera.write().unwrap();
                let mut extra_context = tera::Context::new();

                // TODO: how to refactor this?
                // Basically the if/else branches are the same,
                // but constructing PageTemplateContext with different type parameters.
                if let Some(event) = nostr::parse_event(&front_matter, &content) {
                    extra_context.insert(
                        "resource",
                        &PageTemplateContext {
                            slug: self.slug.to_owned(),
                            date: self.date,
                            tags: event.get_tags_hash(),
                            inner_html: md_to_html(&content),
                            summary: event.get_long_form_summary(),
                            url: self.get_resource_url(&site.config).unwrap(),
                        },
                    );
                } else {
                    extra_context.insert(
                        "resource",
                        &PageTemplateContext {
                            slug: self.slug.to_owned(),
                            date: self.date,
                            tags: front_matter,
                            inner_html: md_to_html(&content),
                            summary: None,
                            url: self.get_resource_url(&site.config).unwrap(),
                        },
                    );
                }
                extra_context.insert("data", &site.data);

                let resources = site.resources.read().unwrap();
                let mut posts_list = resources
                    .values()
                    .collect::<Vec<&Resource>>()
                    .into_iter()
                    .filter(|r| r.kind == ResourceKind::Post)
                    .collect::<Vec<&Resource>>();
                posts_list.sort_by(|a, b| b.date.cmp(&a.date));
                extra_context.insert("posts", &posts_list);

                let rendered_text = render(
                    &content,
                    &site.config,
                    Some(extra_context.clone()),
                    &mut tera,
                );
                let html = md_to_html(&rendered_text);
                let layout = match self.kind {
                    ResourceKind::Post => "post.html".to_string(),
                    _ => "page.html".to_string(),
                };

                render_template(&layout, &mut tera, &html, &site.config, extra_context)
                    .as_bytes()
                    .to_vec()
            }
            ResourceKind::Note => {
                let mut tera = site.tera.write().unwrap();
                let mut extra_context = tera::Context::new();
                let date = self.date;
                extra_context.insert(
                    "resource",
                    &PageTemplateContext {
                        slug: self.slug.to_owned(),
                        date,
                        tags: front_matter,
                        inner_html: content.to_owned(),
                        summary: None,
                        url: self.get_resource_url(&site.config).unwrap(),
                    },
                );
                render_template(
                    "note.html",
                    &mut tera,
                    &content,
                    &site.config,
                    extra_context,
                )
                .as_bytes()
                .to_vec()
            }
        }
    }
}

fn render_template(
    template: &str,
    tera: &mut tera::Tera,
    content: &str,
    site_config: &toml::Value,
    extra_context: tera::Context,
) -> String {
    let mut context = tera::Context::new();
    context.insert("site", &site_config);
    context.insert(
        "servus",
        &ServusMetadata {
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    );
    context.insert("content", &content);
    context.extend(extra_context);

    tera.render(template, &context).unwrap()
}

fn md_to_html(md_content: &str) -> String {
    let options = &markdown::Options {
        compile: markdown::CompileOptions {
            allow_dangerous_html: true,
            ..markdown::CompileOptions::default()
        },
        ..markdown::Options::default()
    };

    markdown::to_html_with_options(md_content, options).unwrap()
}

fn render(
    content: &str,
    site: &toml::Value,
    extra_context: Option<tera::Context>,
    tera: &mut tera::Tera,
) -> String {
    let mut context = tera::Context::new();
    context.insert("site", &site);
    context.insert(
        "servus",
        &ServusMetadata {
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    );
    if let Some(c) = extra_context {
        context.extend(c);
    }

    tera.render_str(content, &context).unwrap()
}

fn render_robots_txt(site_url: &str) -> (mime::Mime, String) {
    let content = format!("User-agent: *\nSitemap: {}/sitemap.xml", site_url);
    (mime::PLAIN, content)
}

fn render_nostr_json(site: &Site) -> (mime::Mime, String) {
    let content = format!(
        "{{ \"names\": {{ \"_\": \"{}\" }} }}",
        site.config.get("pubkey").unwrap().as_str().unwrap()
    );
    (mime::JSON, content)
}

fn render_sitemap_xml(site_url: &str, site: &Site) -> (mime::Mime, String) {
    let mut response: String = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n".to_owned();
    let resources = site.resources.read().unwrap();
    response.push_str("<urlset xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:schemaLocation=\"http://www.sitemaps.org/schemas/sitemap/0.9 http://www.sitemaps.org/schemas/sitemap/0.9/sitemap.xsd\" xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n");
    for url in resources.keys() {
        let mut url = url.trim_end_matches("/index").to_owned();
        if url == site_url && !url.ends_with('/') {
            url.push('/');
        }
        response.push_str(&format!("    <url><loc>{}</loc></url>\n", url));
    }
    response.push_str("</urlset>");

    (mime::XML, response)
}

fn render_atom_xml(site_url: &str, site: &Site) -> (mime::Mime, String) {
    let site_title = match site.config.get("title") {
        Some(t) => t.as_str().unwrap(),
        _ => "",
    };
    let mut response: String = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n".to_owned();
    response.push_str("<feed xmlns=\"http://www.w3.org/2005/Atom\">\n");
    response.push_str(&format!("<title>{}</title>\n", site_title));
    response.push_str(&format!(
        "<link href=\"{}/atom.xml\" rel=\"self\"/>\n",
        site_url
    ));
    response.push_str(&format!("<link href=\"{}/\"/>\n", site_url));
    response.push_str(&format!("<id>{}</id>\n", site_url));
    let resources = site.resources.read().unwrap();
    for (url, resource) in &*resources {
        if resource.date.is_some() {
            if let Some((_, content)) = resource.read(site) {
                response.push_str(
                    &format!(
                        "<entry>
<title>{}</title>
<link href=\"{}\"/>
<updated>{}</updated>
<id>{}/{}</id>
<content type=\"xhtml\"><div xmlns=\"http://www.w3.org/1999/xhtml\">{}</div></content>
</entry>
",
                        resource.title.clone().unwrap_or("".to_string()),
                        &url,
                        &resource.date.unwrap(),
                        site_url,
                        resource.slug.clone(),
                        &md_to_html(&content).to_owned()
                    )
                    .to_owned(),
                );
            }
        }
    }
    response.push_str("</feed>");

    (mime::XML, response)
}

pub fn render_standard_resource(resource_name: &str, site: &Site) -> Option<(mime::Mime, String)> {
    let site_url = site.config.get("url")?.as_str().unwrap();
    match resource_name {
        "robots.txt" => Some(render_robots_txt(site_url)),
        ".well-known/nostr.json" => Some(render_nostr_json(site)),
        "sitemap.xml" => Some(render_sitemap_xml(site_url, site)),
        "atom.xml" => Some(render_atom_xml(site_url, site)),
        _ => None,
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
        let config_content =
            match fs::read_to_string(&format!("{}/_config.toml", path.path().display())) {
                Ok(content) => content,
                _ => {
                    println!(
                        "No site config for site: {}. Skipping!",
                        path.file_name().to_str().unwrap()
                    );
                    continue;
                }
            };

        println!("Loading layouts...");

        let mut tera = tera::Tera::new(&format!(
            "{}/_layouts/**/*",
            fs::canonicalize(path.path()).unwrap().display()
        ))
        .unwrap();
        tera.autoescape_on(vec![]);

        println!("Loaded {} templates!", tera.get_template_names().count());

        let config: HashMap<String, toml::Value> = toml::from_str(&config_content).unwrap();
        let site_config = config.get("site").unwrap();

        let site = Site {
            config: site_config.clone(),
            path: path.path().display().to_string(),
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

    default_theme::generate(&format!("./sites/{}/", domain));

    let mut tera = tera::Tera::new(&format!("{}/_layouts/**/*", path)).unwrap();
    tera.autoescape_on(vec![]);

    let config_content = format!(
        "[site]\npubkey = \"{}\"\nurl = \"https://{}\"\ntitle = \"{}\"\ntagline = \"{}\"",
        admin_pubkey.unwrap_or("".to_string()),
        domain,
        "Untitled site", // TODO: get from the request?
        "Undefined tagline"
    );
    fs::write(format!("./sites/{}/_config.toml", domain), &config_content).unwrap();

    let site_config = toml::from_str::<HashMap<String, toml::Value>>(&config_content).unwrap();

    let site = Site {
        config: site_config.get("site").unwrap().clone(),
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
        nostr::EVENT_KIND_LONG_FORM => {
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
