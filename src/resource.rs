use chrono::NaiveDateTime;
use http_types::mime;
use serde::Serialize;
use std::{collections::HashMap, env, fs::File, io::BufReader, str};

use crate::{
    content, nostr,
    site::{ServusMetadata, Site},
};

#[derive(Clone, Copy, PartialEq, Serialize)]
pub enum ResourceKind {
    Post,
    Page,
    Note,
}

#[derive(Clone, Serialize)]
pub enum ContentSource {
    Event(String),
    File(String),
}

#[derive(Clone, Serialize)]
struct PageTemplateContext<TagType> {
    url: String,
    slug: String,
    summary: Option<String>,
    content: String,
    date: Option<NaiveDateTime>,
    #[serde(flatten)]
    tags: HashMap<String, TagType>,
}

#[derive(Clone, Serialize)]
pub struct Resource {
    pub kind: ResourceKind,
    pub slug: String,

    pub title: Option<String>,
    pub date: Option<NaiveDateTime>,

    pub content_source: ContentSource,
}

impl Resource {
    fn read(&self, site: &Site) -> Option<(HashMap<String, serde_yaml::Value>, String)> {
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

    pub fn get_resource_url(&self) -> Option<String> {
        // TODO: extract all URL patterns from config!
        match self.kind {
            ResourceKind::Post => Some(format!("/posts/{}", &self.slug)),
            ResourceKind::Page => Some(format!("/{}", &self.clone().slug)),
            ResourceKind::Note => Some(format!("/notes/{}", &self.clone().slug)),
        }
    }

    pub fn render(&self, site: &Site) -> Vec<u8> {
        let (front_matter, content) = self.read(site).unwrap();

        let mut tera = site.tera.write().unwrap();
        let mut extra_context = tera::Context::new();

        // TODO: need real multilang support,
        // but for now, we just set this so that Zola themes don't complain
        extra_context.insert("lang", "en");

        extra_context.insert("config", &site.config);

        // TODO: how to refactor this?
        // Basically the if/else branches are the same,
        // but constructing PageTemplateContext with different type parameters.
        if let Some(event) = nostr::parse_event(&front_matter, &content) {
            let mut tags = event.get_tags_hash();
            if !tags.contains_key("title") {
                tags.insert("title".to_string(), "".to_string());
            }
            extra_context.insert(
                "page",
                &PageTemplateContext {
                    slug: self.slug.to_owned(),
                    date: self.date,
                    tags,
                    content: md_to_html(&content),
                    summary: event.get_long_form_summary(),
                    url: self.get_resource_url().unwrap(),
                },
            );
        } else {
            extra_context.insert(
                "page",
                &PageTemplateContext {
                    slug: self.slug.to_owned(),
                    date: self.date,
                    tags: front_matter,
                    content: md_to_html(&content),
                    summary: None,
                    url: self.get_resource_url().unwrap(),
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

        let rendered_text = render(&content, Some(extra_context.clone()), &mut tera);
        let html = md_to_html(&rendered_text);

        render_template("page.html", &mut tera, &html, extra_context)
            .as_bytes()
            .to_vec()
    }
}

fn render_template(
    template: &str,
    tera: &mut tera::Tera,
    content: &str,
    extra_context: tera::Context,
) -> String {
    let mut context = tera::Context::new();
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

fn render_robots_txt(site_url: &str) -> (mime::Mime, String) {
    let content = format!("User-agent: *\nSitemap: {}/sitemap.xml", site_url);
    (mime::PLAIN, content)
}

fn render_nostr_json(site: &Site) -> (mime::Mime, String) {
    let content = format!(
        "{{ \"names\": {{ \"_\": \"{}\" }} }}",
        site.config.pubkey.clone().unwrap_or("".to_string())
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
    let mut response: String = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n".to_owned();
    response.push_str("<feed xmlns=\"http://www.w3.org/2005/Atom\">\n");
    response.push_str(&format!(
        "<title>{}</title>\n",
        &site.config.title.clone().unwrap_or("".to_string())
    ));
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
    match resource_name {
        "robots.txt" => Some(render_robots_txt(&site.config.base_url)),
        ".well-known/nostr.json" => Some(render_nostr_json(site)),
        "sitemap.xml" => Some(render_sitemap_xml(&site.config.base_url, site)),
        "atom.xml" => Some(render_atom_xml(&site.config.base_url, site)),
        _ => None,
    }
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

fn render(content: &str, extra_context: Option<tera::Context>, tera: &mut tera::Tera) -> String {
    let mut context = tera::Context::new();
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
