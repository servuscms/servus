use chrono::NaiveDate;
use http_types::mime;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    str,
    str::FromStr,
    sync::{Arc, RwLock},
};
use tide::{Request, Response};
use tide_acme::rustls_acme::caches::DirCache;
use tide_acme::{AcmeConfig, TideRustlsExt};
use walkdir::{DirEntry, WalkDir};
use yaml_front_matter::YamlFrontMatter;

mod default_site;

#[derive(Clone, Serialize, Deserialize)]
struct ServusMetadata {
    version: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct PageMetadata {
    title: String,
    permalink: Option<String>,
    description: Option<String>,
    lang: Option<String>,
}

#[derive(Clone, Serialize)]
struct Resource {
    url: String,
    mime: String,
    content: Vec<u8>,
    excerpt: Option<String>,    // only used for posts and pages
    meta: Option<PageMetadata>, // only used for posts and pages
    text: Option<String>,       // only used for posts and pages
    slug: Option<String>,       // only used for posts
    date: Option<NaiveDate>,    // only used for posts
}

#[derive(Clone)]
struct SiteState {
    site: toml::Value,
    resources: Arc<RwLock<HashMap<String, Resource>>>,
}

#[derive(Clone)]
struct State {
    sites: HashMap<String, SiteState>,
}

fn get_site_for_request(request: &Request<State>) -> &SiteState {
    let state = &request.state();
    let host = match request.host() {
        Some(host) => {
            if state.sites.contains_key(host) {
                host.to_string()
            } else if host.contains(':') {
                let re = Regex::new(r":\d+").unwrap();
                let portless = re.replace(host, "").to_string();
                if state.sites.contains_key(&portless) {
                    portless
                } else {
                    "default".to_string()
                }
            } else {
                "default".to_string()
            }
        }
        _ => "default".to_string(),
    };

    &state.sites[&host]
}

#[async_std::main]
async fn main() -> Result<(), std::io::Error> {
    femme::start();

    let sites = get_sites();

    let mut app = tide::with_state(State {
        sites: sites.clone(),
    });
    app.with(tide::log::LogMiddleware::new());

    fn render(resource: &Resource) -> Response {
        let mime = mime::Mime::from_str(resource.mime.as_str()).unwrap();

        Response::builder(200)
            .content_type(mime)
            .header("Access-Control-Allow-Origin", "*")
            .body(&*resource.content)
            .build()
    }

    app.at("/").get(|request: Request<State>| async move {
        let site = &get_site_for_request(&request);
        match site.resources.read().unwrap().get("/index") {
            Some(index) => Ok(render(index)),
            None => Ok(Response::new(404)),
        }
    });
    app.at("*path").get(|request: Request<State>| async move {
        let site = &get_site_for_request(&request);
        let mut path = request.param("path").unwrap();
        if path.ends_with('/') {
            path = path.strip_suffix('/').unwrap();
        }
        let resources = site.resources.read().unwrap();
        match resources.get(&format!("/{}", &path)) {
            Some(resource) => Ok(render(resource)),
            None => match resources.get(&format!("/{}/index", &path)) {
                Some(resource) => Ok(render(resource)),
                None => Ok(Response::new(404)),
            },
        }
    });

    let addr = "0.0.0.0";
    let mut port = 443;
    let mut ssl = true;
    let mut dev_mode = false;
    let mut production_cert = false; // default to staging

    let args: Vec<_> = env::args().collect();
    if args.len() > 1 {
        let mode = args[1].as_str();
        match mode {
            "dev" => {
                port = 4884;
                ssl = false;
                dev_mode = true;
            }
            "live" => {
                production_cert = true;
            }
            _ => {
                panic!("Valid modes are 'dev' and 'live'.");
            }
        }
    }

    if dev_mode {
        println!("Open http://localhost:{} in your browser!", port);
    } else {
        println!("Running on port {} using SSL={}", port, ssl);
        if !production_cert {
            println!("Using Let's Encrypt staging environment. Great for testing, but browsers will complain about the certificate.");
        }
    }

    let bind_to = format!("{addr}:{port}");

    if ssl {
        let domains: Vec<String> = sites.keys().filter(|&x| x != "default").cloned().collect();
        let cache = DirCache::new("./cache");
        let mut acme_config = AcmeConfig::new(domains)
            .cache(cache)
            .directory_lets_encrypt(production_cert);
        for (domain, site) in sites {
            if domain != "default" {
                let mut contact: String = "mailto:".to_owned();
                contact.push_str(site.site.get("contact_email").unwrap().as_str().unwrap());
                acme_config = acme_config.contact_push(contact);
            }
        }

        app.listen(
            tide_rustls::TlsListener::build()
                .addrs(bind_to)
                .acme(acme_config),
        )
        .await?;
    } else {
        app.listen(bind_to).await?;
    }

    Ok(())
}

fn get_sites() -> HashMap<String, SiteState> {
    let mut paths = match fs::read_dir("./sites") {
        Ok(paths) => paths.map(|r| r.unwrap()).collect(),
        _ => vec![],
    };

    if paths.is_empty() {
        println!("No sites found! Generating default site...");

        default_site::generate("./sites/default");

        paths = fs::read_dir("./sites")
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
    }

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

        let config: HashMap<String, toml::Value> = toml::from_str(&config_content).unwrap();
        let site_config = config.get("site").unwrap();

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
            let f = std::fs::File::open(data_path.path()).unwrap();
            let data: serde_yaml::Value = serde_yaml::from_reader(f).unwrap();
            site_data.insert(data_name, data);
        }

        let resources = load_resources(&path.path(), site_config, &site_data, &mut tera);

        sites.insert(
            path.file_name().to_str().unwrap().to_string(),
            SiteState {
                site: site_config.clone(),
                resources: Arc::new(RwLock::new(resources)),
            },
        );
    }

    sites
}

fn get_post(
    path: &Path,
    site: &toml::Value,
    site_data: &HashMap<String, serde_yaml::Value>,
    tera: &tera::Tera,
) -> Option<Resource> {
    let filename = path.file_name().unwrap().to_str().unwrap();
    if filename.len() < 11 {
        println!("Invalid filename: {}", filename);
        return None;
    }

    let date_part = &filename[0..10];
    let date = match NaiveDate::parse_from_str(date_part, "%Y-%m-%d") {
        Ok(date) => date,
        _ => {
            println!("Invalid date: {}. Skipping!", date_part);
            return None;
        }
    };

    let content = fs::read_to_string(path.display().to_string()).unwrap();

    let (text, maybe_meta) = parse_meta(&content);
    if maybe_meta.is_none() {
        println!(
            "Cannot parse metadata for {}. Skipping post!",
            path.display()
        );
        return None;
    }

    let meta = maybe_meta.unwrap();
    let html_text = md_to_html(text);
    let slug = Path::new(&filename[11..])
        .file_stem()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let mut excerpt: Option<String> = None;
    let dom = tl::parse(&html_text, tl::ParserOptions::default()).unwrap();
    let parser = dom.parser();
    if let Some(p) = dom.query_selector("p").unwrap().next() {
        excerpt = Some(p.get(parser).unwrap().inner_text(parser).to_string());
    }

    let mut resource = Resource {
        url: format!("{}/posts/{}", site.get("url").unwrap(), &slug),
        mime: format!("{}", mime::HTML),
        content: vec![],
        excerpt,
        meta: Some(meta),
        text: Some(html_text.clone()),
        slug: Some(slug),
        date: Some(date),
    };

    let mut extra_context = tera::Context::new();
    extra_context.insert("page", &resource);
    extra_context.insert("data", &site_data);

    resource.content = render_template("post.html", tera, html_text.clone(), site, extra_context)
        .as_bytes()
        .to_vec();

    Some(resource)
}

fn skipped(entry: &DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(|s| (s.starts_with('.') || s.starts_with('_')) && s != ".well-known")
        .unwrap_or(false)
}

fn load_posts(
    site_path: &PathBuf,
    site: &toml::Value,
    site_data: &HashMap<String, serde_yaml::Value>,
    tera: &tera::Tera,
) -> HashMap<String, Resource> {
    let mut posts = HashMap::new();

    let mut posts_path = PathBuf::from(site_path);
    posts_path.push("_posts/");

    for entry in WalkDir::new(&posts_path) {
        let path = entry.unwrap().into_path();
        if !path.is_file() {
            continue;
        }
        let maybe_post = get_post(&path, site, site_data, tera);
        if maybe_post.is_none() {
            continue;
        }
        let post = maybe_post.unwrap();
        let resource_path = format!("/posts/{}", post.slug.as_ref().unwrap());

        println!("Loaded post {}", resource_path);
        posts.insert(resource_path, post);
    }

    posts
}

fn load_pages(
    site_path: &PathBuf,
    site: &toml::Value,
    site_data: &HashMap<String, serde_yaml::Value>,
    tera: &mut tera::Tera,
    posts: &Vec<&Resource>,
) -> HashMap<String, Resource> {
    let mut pages = HashMap::new();

    let mut posts_path = PathBuf::from(site_path);
    posts_path.push("posts/");

    let page_walker = WalkDir::new(site_path).into_iter();
    for entry in page_walker.filter_entry(|e| !skipped(e)) {
        let path = entry.unwrap().into_path();

        if !path.is_file() {
            continue;
        }

        let extension = path.extension().unwrap().to_str().unwrap();
        if extension != "md" && extension != "html" {
            continue;
        }

        if path
            .display()
            .to_string()
            .starts_with(posts_path.display().to_string().as_str())
        {
            continue;
        }

        let site_prefix = site_path.display().to_string();
        let path_str = path.display().to_string();

        let content = fs::read_to_string(&path).unwrap();
        let (text, maybe_meta) = parse_meta(&content);
        let meta = match maybe_meta {
            Some(m) => m,
            None => {
                println!(
                    "Cannot parse metadata for {}. Skipping page!",
                    path.display()
                );
                continue;
            }
        };

        let resource_path = if meta.permalink.is_some() {
            meta.clone()
                .permalink
                .unwrap()
                .strip_suffix('/')
                .unwrap()
                .to_string()
        } else {
            path_str
                .strip_prefix(site_prefix.as_str())
                .unwrap()
                .strip_suffix(&format!(".{}", extension))
                .unwrap()
                .to_string()
        };

        let mut resource = Resource {
            url: format!("{}{}", site.get("url").unwrap(), resource_path),
            mime: format!("{}", mime::HTML),
            content: vec![],
            excerpt: None,
            meta: Some(meta),
            text: None,
            slug: None,
            date: None,
        };

        let mut extra_context = tera::Context::new();
        extra_context.insert("page", &resource);
        extra_context.insert("posts", &posts);
        extra_context.insert("data", &site_data);

        let rendered_text = render(&text, site, Some(extra_context.clone()), tera);
        let html_text = if extension == "md" {
            md_to_html(rendered_text)
        } else {
            rendered_text
        };
        resource.text = Some(html_text.clone());
        resource.content = render_template("page.html", tera, html_text, site, extra_context)
            .as_bytes()
            .to_vec();

        println!("Loaded page {} ", resource_path);
        pages.insert(resource_path.to_string(), resource);
    }

    pages
}

fn load_extra_resources(
    site_path: &PathBuf,
    site: &toml::Value,
    tera: &mut tera::Tera,
    posts: &Vec<&Resource>,
    pages: &Vec<&Resource>,
) -> HashMap<String, Resource> {
    let mut resources = HashMap::new();

    let walker = WalkDir::new(site_path).into_iter();
    for entry in walker.filter_entry(|e| !skipped(e)) {
        let path = entry.unwrap().into_path();

        if !path.is_file() {
            continue;
        }
        let extension = path.extension().unwrap().to_str().unwrap();
        if extension == "md" || extension == "html" {
            continue;
        }

        let site_prefix = site_path.display().to_string();
        let path_str = path.display().to_string();

        let resource_path = path_str.strip_prefix(site_prefix.as_str()).unwrap();
        let mime;
        let content;

        let extension = path.extension().unwrap().to_str().unwrap();
        match extension {
            "xml" | "txt" => {
                let mut extra_context = tera::Context::new();
                extra_context.insert("posts", &posts);
                extra_context.insert("pages", &pages);

                content = render(
                    &fs::read_to_string(&path).unwrap(),
                    site,
                    Some(extra_context),
                    tera,
                )
                .as_bytes()
                .to_vec();

                mime = if extension == "xml" {
                    mime::XML
                } else {
                    mime::PLAIN
                };
            }
            _ => {
                content = fs::read(&path).unwrap();

                mime = match mime::Mime::sniff(&content) {
                    Ok(m) => m,
                    _ => match mime::Mime::from_extension(extension) {
                        Some(m) => m,
                        _ => mime::PLAIN,
                    },
                };
            }
        };

        let resource = Resource {
            url: format!("{}{}", site.get("url").unwrap(), resource_path),
            mime: format!("{}", mime),
            content,
            excerpt: None,
            meta: None,
            text: None,
            slug: None,
            date: None,
        };

        println!(
            "Loaded resource {} {} bytes={}",
            resource_path,
            resource.mime,
            resource.content.len()
        );
        resources.insert(resource_path.to_string(), resource);
    }

    resources
}

fn load_resources(
    site_path: &PathBuf,
    site: &toml::Value,
    site_data: &HashMap<String, serde_yaml::Value>,
    tera: &mut tera::Tera,
) -> HashMap<String, Resource> {
    let posts = load_posts(site_path, site, site_data, tera);

    let mut posts_list: Vec<&Resource> = posts.values().into_iter().collect();
    posts_list.sort_by(|a, b| b.date.cmp(&a.date));

    let pages = load_pages(site_path, site, site_data, tera, &posts_list);

    let pages_list: Vec<&Resource> = pages.values().into_iter().collect();

    let extra_resources = load_extra_resources(site_path, site, tera, &posts_list, &pages_list);

    let mut resources = HashMap::new();
    resources.extend(posts);
    resources.extend(pages);
    resources.extend(extra_resources);

    resources
}

fn parse_meta(content: &String) -> (String, Option<PageMetadata>) {
    if let Ok(document) = YamlFrontMatter::parse::<PageMetadata>(content) {
        (document.content, Some(document.metadata))
    } else {
        (content.to_string(), None)
    }
}

fn md_to_html(md_content: String) -> String {
    let options = &markdown::Options {
        compile: markdown::CompileOptions {
            allow_dangerous_html: true,
            ..markdown::CompileOptions::default()
        },
        ..markdown::Options::default()
    };

    markdown::to_html_with_options(&md_content, options).unwrap()
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

fn render_template(
    template: &str,
    tera: &tera::Tera,
    content: String,
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
