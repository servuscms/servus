use chrono::NaiveDateTime;
use http_types::mime;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    env, fs,
    path::PathBuf,
    str,
    sync::{Arc, RwLock},
};
use walkdir::WalkDir;

use crate::nostr::{self, EVENT_KIND_LONG_FORM};

#[derive(Clone, Serialize, Deserialize)]
struct ServusMetadata {
    version: String,
}

#[derive(Clone, Serialize)]
struct Post {
    slug: String,
    url: String,
    summary: Option<String>,
    inner_html: String,
    date: Option<NaiveDateTime>,
    #[serde(flatten)]
    tags: HashMap<String, String>,
}

#[derive(Clone)]
pub struct Site {
    pub path: String,
    pub config: toml::Value,
    pub data: HashMap<String, serde_yaml::Value>,
    pub resources: Arc<RwLock<HashMap<String, Resource>>>,
    pub tera: Arc<RwLock<tera::Tera>>,
}

impl Site {
    pub fn load_resources(&self) {
        let mut root = PathBuf::from(&self.path);
        root.push("_events/");
        if !root.as_path().exists() {
            return;
        }
        for entry in WalkDir::new(&root) {
            let path = entry.unwrap().into_path();
            if !path.is_file() {
                continue;
            }
            println!("Scanning file {}...", path.display());
            if let Some(event) = nostr::read_event(&path.display().to_string()) {
                let slug = event.get_long_form_slug();
                let date = event.get_long_form_published_at();
                if let Some(kind) = get_resource_kind(&event) {
                    let resource = Resource {
                        kind,
                        event_kind: event.kind,
                        event_id: event.id.to_owned(),
                        date,
                        slug: slug.to_owned(),
                    };
                    if let Some(url) = resource.get_resource_url(&self.config) {
                        println!("Resource url={} -> {}.", &url, &resource.event_id);
                        let mut resources = self.resources.write().unwrap();
                        resources.insert(url, resource);
                    }
                }
            }
        }
    }

    pub fn add_post(&self, event: &nostr::Event) {
        let slug = event.get_long_form_slug().unwrap(); // all posts have a slug!

        let post = Resource {
            kind: get_resource_kind(event).unwrap(),
            event_kind: event.kind,
            event_id: event.id.to_owned(),
            date: event.get_long_form_published_at(),
            slug: Some(slug.to_owned()),
        };

        let mut file = fs::File::create(post.get_path(self).unwrap()).unwrap();
        event.write(&mut file).unwrap();

        if let Some(url) = post.get_resource_url(&self.config) {
            // but not all posts have an URL (drafts don't)
            let mut resources = self.resources.write().unwrap();
            resources.insert(url.to_owned(), post);
        }
    }

    pub fn remove_post(&self, deletion_event: &nostr::Event) -> bool {
        let mut deleted_event_ref: Option<String> = None;
        for tag in &deletion_event.tags {
            if tag[0] == "a" {
                // TODO: should we also support "e" tags?
                deleted_event_ref = Some(tag[1].to_owned());
            }
        }

        if deleted_event_ref.is_none() {
            return false;
        }

        let deleted_event_ref = deleted_event_ref.unwrap();

        let parts = deleted_event_ref.split(':').collect::<Vec<_>>();
        if parts.len() != 3 {
            return false;
        }

        if parts[1] != deletion_event.pubkey {
            return false;
        }

        let deleted_event_kind = parts[0].parse::<i64>().unwrap();
        let deleted_event_slug = Some(parts[2].to_owned());

        let mut resource_url: Option<String> = None;
        let mut path: Option<String> = None;
        {
            let resources = self.resources.read().unwrap();
            for (url, resource) in &*resources {
                if resource.event_kind == deleted_event_kind && resource.slug == deleted_event_slug
                {
                    resource_url = Some(url.to_owned());
                    path = resource.get_path(self);
                }
            }
        }

        if let Some(resource_url) = resource_url {
            self.resources.write().unwrap().remove(&resource_url);
        }

        if let Some(path) = path {
            fs::remove_file(path).is_ok()
        } else {
            false
        }
    }
}

#[derive(Clone, Serialize)]
pub enum ResourceKind {
    Post,
    Page,
}

#[derive(Clone, Serialize)]
pub struct Resource {
    pub kind: ResourceKind,
    pub event_kind: i64,
    pub event_id: String,
    pub date: Option<NaiveDateTime>,
    pub slug: Option<String>,
}

impl Resource {
    fn get_relative_path(&self) -> Option<String> {
        let slug = self.clone().slug.unwrap().to_owned();
        match self.kind {
            ResourceKind::Page => Some(format!("pages/{}.md", slug)),
            ResourceKind::Post => Some(format!("posts/{}.md", slug)),
        }
    }

    pub fn get_path(&self, site: &Site) -> Option<String> {
        let mut path = PathBuf::from(&site.path);
        path.push("_events/");
        path.push(&self.get_relative_path()?);

        Some(path.display().to_string())
    }

    pub fn read_event(&self, site: &Site) -> nostr::Event {
        nostr::read_event(&self.get_path(site).unwrap()).unwrap()
    }

    pub fn get_resource_url(&self, site_config: &toml::Value) -> Option<String> {
        match self.kind {
            ResourceKind::Post => {
                let slug = self.clone().slug.unwrap();
                return Some(site_config.get("post_permalink").map_or_else(
                    || format!("/posts/{}", &slug),
                    |p| p.as_str().unwrap().replace(":slug", &slug),
                ));
            }
            ResourceKind::Page => Some(format!("/{}", &self.clone().slug.unwrap())),
        }
    }

    pub fn render(&self, site: &Site) -> Vec<u8> {
        let event = self.read_event(site);

        match self.kind {
            ResourceKind::Page | ResourceKind::Post => {
                let mut tera = site.tera.write().unwrap();
                let slug = event.get_long_form_slug().unwrap();
                let date = event.get_long_form_published_at();
                let mut extra_context = tera::Context::new();
                extra_context.insert(
                    "page", // TODO: rename to "post"?
                    &Post {
                        slug: slug.to_owned(),
                        date,
                        tags: event.get_tags_hash(),
                        inner_html: md_to_html(&event.content),
                        summary: event.get_long_form_summary(),
                        url: self.get_resource_url(&site.config).unwrap(),
                    },
                );
                extra_context.insert("data", &site.data);

                let rendered_text = render(
                    &event.content,
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
        }
    }
}

fn render_template(
    template: &str,
    tera: &mut tera::Tera,
    content: &str,
    site: &toml::Value,
    extra_context: tera::Context,
) -> String {
    let mut context = tera::Context::new();
    context.insert("site", &site);
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
    for (url, post_ref) in &*resources {
        let event = post_ref.read_event(site);
        if event.get_long_form_published_at().is_some() {
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
                    &event.get_tags_hash().get("title").unwrap(),
                    &url,
                    &event.get_long_form_published_at().unwrap(),
                    site_url,
                    event.get_long_form_slug().unwrap().clone(),
                    &md_to_html(&event.content).to_owned()
                )
                .to_owned(),
            );
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

        let mut site_data: HashMap<String, serde_yaml::Value> = HashMap::new();

        let site_data_paths = match fs::read_dir(format!("{}/_data", path.path().display())) {
            Ok(paths) => paths.map(|r| r.unwrap()).collect(),
            _ => vec![],
        };
        for data_path in &site_data_paths {
            let data_name = data_path
                .path()
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            println!("Loading data: {}", &data_name);
            let f = fs::File::open(data_path.path()).unwrap();
            let data: serde_yaml::Value = serde_yaml::from_reader(f).unwrap();
            site_data.insert(data_name, data);
        }

        let config: HashMap<String, toml::Value> = toml::from_str(&config_content).unwrap();
        let site_config = config.get("site").unwrap();

        let site = Site {
            config: site_config.clone(),
            path: path.path().display().to_string(),
            data: site_data,
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

fn get_resource_kind(event: &nostr::Event) -> Option<ResourceKind> {
    let date = event.get_long_form_published_at();
    match event.kind {
        EVENT_KIND_LONG_FORM => {
            if date.is_some() {
                Some(ResourceKind::Post)
            } else {
                Some(ResourceKind::Page)
            }
        }
        _ => None,
    }
}
