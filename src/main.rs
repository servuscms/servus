use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
    fs,
    sync::{Arc, RwLock},
    str,
    str::FromStr,
};
use chrono::NaiveDate;
use http_types::mime;
use markdown;
use regex::Regex;
use serde::{Serialize, Deserialize};
use tera;
use tide::{Request, Response};
use tide_acme::{AcmeConfig, TideRustlsExt};
use tide_acme::rustls_acme::caches::DirCache;
use tide_rustls;
use tl;
use toml;
use walkdir::{DirEntry, WalkDir};
use yaml_front_matter::YamlFrontMatter;

mod default_site;

#[derive(Clone)]
#[derive(Serialize)]
#[derive(Deserialize)]
struct ServusMetadata {
    version: String,
}

#[derive(Deserialize)]
struct SiteConfig {
    site: SiteMetadata,
}

#[derive(Clone)]
#[derive(Serialize)]
#[derive(Deserialize)]
struct SiteMetadata {
    title: String,
    tagline: String,
    contact_email: String,
    url: String,
}

#[derive(Clone)]
#[derive(Serialize)]
#[derive(Deserialize)]
struct PageMetadata {
    title: String,
    description: Option<String>,
}

#[derive(Clone)]
#[derive(Serialize)]
struct Resource {
    url: String,
    mime: String,
    content: Vec<u8>,
    meta: Option<PageMetadata>, // only used for posts and pages
    text: Option<String>, // only used for posts and pages
    slug: Option<String>, // only used for posts
    date: Option<NaiveDate>, // only used for posts
}

#[derive(Clone)]
struct SiteState {
    site: SiteMetadata,
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
            } else if host.contains(":") {
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

    let mut app = tide::with_state(State { sites: sites.clone() });
    app.with(tide::log::LogMiddleware::new());

    fn render(resource: &Resource) -> Response {
        let mime = mime::Mime::from_str(resource.mime.as_str()).unwrap();

        Response::builder(200).content_type(mime).body(&*resource.content).build()
    }

    app.at("/").get(|request: Request<State>| async move {
        let site = &get_site_for_request(&request);
        match site.resources.read().unwrap().get("/index") {
            Some(index) => {
                return Ok(render(index));
            },
            None => {
                return Ok(Response::new(404));
            }
        };
    });
    app.at("*path").get(|request: Request<State>| async move {
        let site = &get_site_for_request(&request);
        let mut path = request.param("path")?;
        if path.ends_with("/") {
            path = path.strip_suffix("/").unwrap();
        }
        match site.resources.read().unwrap().get(&format!("/{}", &path)) {
            Some(resource) => {
                return Ok(render(resource));
            },
            None => {
                return Ok(Response::new(404));
            },
        };
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
            },
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
        let mut acme_config = AcmeConfig::new(domains).cache(cache).directory_lets_encrypt(production_cert);
        for (domain, site) in sites {
            if domain != "default" {
                let mut contact: String = "mailto:".to_owned();
                contact.push_str(&site.site.contact_email);
                acme_config = acme_config.contact_push(contact);
            }
        }

        app.listen(tide_rustls::TlsListener::build().addrs(bind_to).acme(acme_config)).await?;
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

    if paths.len() == 0 {
        println!("No sites found! Generating default site...");

        default_site::generate("./sites/default");

        paths = fs::read_dir("./sites").unwrap().map(|r| r.unwrap()).collect();
    }

    let mut sites = HashMap::new();
    for path in &paths {
        println!("Found site: {}", path.file_name().to_str().unwrap());
        let mut site_config_path = PathBuf::from(&path.path());
        site_config_path.push(".servus/config.toml");
        let site_config_content = match fs::read_to_string(&site_config_path) {
            Ok(content) => content,
            _ => {
                println!("No site config for site: {}. Skipping!", path.file_name().to_str().unwrap());
                continue;
            },
        };

        let mut tera = tera::Tera::new(&format!("{}/.servus/templates/*.html", fs::canonicalize(path.path()).unwrap().display())).unwrap();
        tera.autoescape_on(vec![]);

        let site_config: SiteConfig = toml::from_str(&site_config_content).unwrap();

        let resources = get_resources(&path.path(), &site_config.site, &tera);

        sites.insert(path.file_name().to_str().unwrap().to_string(),
                     SiteState {
                         site: site_config.site,
                         resources: Arc::new(RwLock::new(resources)),
                     });
    }

    sites
}

fn get_post(path: &PathBuf, site: &SiteMetadata, tera: &tera::Tera) -> Option<Resource> {
    let filename = path.file_name().unwrap().to_str().unwrap();
    if filename.len() < 11 {
        println!("Invalid filename: {}", filename);
        return None;
    }

    let date_part = &filename[0..10];
    let date = match NaiveDate::parse_from_str(date_part, "%Y-%m-%d") {
        Ok(date) => date,
        _ => {
            println!("Invalid date: {}. Skipping!", date_part.to_string());
            return None;
        }
    };

    let content = fs::read_to_string(&path.display().to_string()).unwrap();

    let (text, maybe_meta) = parse_meta(&content);
    if maybe_meta.is_none() {
        println!("Cannot parse metadata for {}. Skipping post!", path.display());
        return None;
    }

    let mut meta = maybe_meta.unwrap();
    let html_text = md_to_html(text);
    let slug = Path::new(&filename[11..]).file_stem().unwrap().to_str().unwrap().to_string();

    if meta.description.is_none() {
        let dom = tl::parse(&html_text, tl::ParserOptions::default()).unwrap();
        let parser = dom.parser();
        match dom.query_selector("p").unwrap().next() {
            Some(p) => {
                meta.description = Some(p.get(&parser).unwrap().inner_text(&parser).to_string());
            }
            _ => {}
        }
    }

    let mut resource = Resource {
        url: format!("{}/posts/{}", site.url, &slug),
        mime: format!("{}", mime::HTML),
        content: vec![],
        meta: Some(meta.clone()),
        text: Some(html_text.clone()),
        slug: Some(slug.clone()),
        date: Some(date),
    };

    let mut extra_context = tera::Context::new();
    extra_context.insert("page", &resource);

    resource.content = render_template("post.html", tera, html_text, &site, extra_context).as_bytes().to_vec();

    Some(resource)
}

fn is_hidden(entry: &DirEntry) -> bool {
    entry.file_name().to_str().map(|s| s.starts_with(".")).unwrap_or(false)
}

fn get_resources(site_path: &PathBuf, site: &SiteMetadata, tera: &tera::Tera) -> HashMap<String, Resource> {
    let mut resources = HashMap::new();

    let mut posts_path = PathBuf::from(site_path);
    posts_path.push("posts/");

    let mut posts = HashMap::new();

    for entry in WalkDir::new(&posts_path) {
        let path = entry.unwrap().into_path();
        if !path.is_file() {
            continue;
        }
        let maybe_post = get_post(&path, &site, &tera);
        if maybe_post.is_none() {
            continue;
        }
        let post = maybe_post.unwrap();
        let resource_path = format!("/posts/{}", post.slug.as_ref().unwrap());
        println!("Loaded post {}", resource_path);
        posts.insert(resource_path, post);
    }

    let mut extra_context_posts = tera::Context::new();
    let mut posts_list: Vec<&Resource> = posts.values().into_iter().collect();
    posts_list.sort_by(|a, b| b.date.cmp(&a.date));
    extra_context_posts.insert("posts", &posts_list);

    let walker = WalkDir::new(site_path).into_iter();
    for entry in walker.filter_entry(|e| !is_hidden(e)) {
        let path = entry.unwrap().into_path();

        if !path.is_file() {
            continue;
        }

        if path.display().to_string().starts_with(posts_path.display().to_string().as_str()) {
            continue;
        }

        let site_prefix = site_path.display().to_string();
        let path_str = path.display().to_string();

        let mut resource_path = path_str.strip_prefix(site_prefix.as_str()).unwrap();
        let mut resource;

        match path.extension().unwrap().to_str().unwrap() {
            "md" => {
                let content = fs::read_to_string(&path).unwrap();
                let (text, maybe_meta) = parse_meta(&content);
                if maybe_meta.is_none() {
                    println!("Cannot parse metadata for {}. Skipping page!", path.display());
                    continue;
                }
                resource_path = resource_path.strip_suffix(".md").unwrap();
                let meta = maybe_meta.unwrap();
                resource = Resource {
                    url: format!("{}{}", site.url, resource_path),
                    mime: format!("{}", mime::HTML),
                    content: vec![],
                    meta: Some(meta.clone()),
                    text: None,
                    slug: None,
                    date: None,
                };
                let mut extra_context = tera::Context::new();
                extra_context.insert("page", &resource);
                extra_context.extend(extra_context_posts.clone());
                let rendered_text = render(&text, &site, Some(extra_context.clone()));
                let html_text = md_to_html(rendered_text);
                resource.text = Some(html_text.clone());
                resource.content = render_template("page.html", tera, html_text, &site, extra_context).as_bytes().to_vec();
            }
            "xml" => {
                let content = fs::read_to_string(&path).unwrap();
                resource = Resource {
                    url: format!("{}{}", site.url, resource_path),
                    mime: format!("{}", mime::XML),
                    content: render(&content, &site, Some(extra_context_posts.clone())).as_bytes().to_vec(),
                    meta: None,
                    text: None,
                    slug: None,
                    date: None,
                };
            }
            _ => {
                let content = fs::read(&path).unwrap();
                let mime = match mime::Mime::sniff(&content) {
                    Ok(m) => m,
                    _ => {
                        match mime::Mime::from_extension(&path.extension().unwrap().to_str().unwrap()) {
                            Some(m) => m,
                            _ => mime::PLAIN,
                        }
                    }
                };
                resource = Resource {
                    url: format!("{}{}", site.url, resource_path),
                    mime: format!("{}", mime),
                    content: content,
                    meta: None,
                    text: None,
                    slug: None,
                    date: None,
                };
            }
        }

        println!("Loaded resource {} {} bytes={}", resource_path, resource.mime, resource.content.len());
        resources.insert(resource_path.to_string(), resource);
    }

    resources.extend(posts);
    
    resources
}

fn parse_meta(content: &String) -> (String, Option<PageMetadata>) {
    if let Ok(document) = YamlFrontMatter::parse::<PageMetadata>(&content) {
        return (document.content, Some(document.metadata));
    } else {
        return (content.to_string(), None);
    }
}

fn md_to_html(md_content: String) -> String {
    let options = &markdown::Options {compile: markdown::CompileOptions {allow_dangerous_html: true,
                                                                         ..markdown::CompileOptions::default()},
                                      ..markdown::Options::default()};

    markdown::to_html_with_options(&md_content, &options).unwrap()
}

fn render(content: &String, site: &SiteMetadata, extra_context: Option<tera::Context>) -> String {
    let mut context = tera::Context::new();
    context.insert("site", &site);
    context.insert("servus", &ServusMetadata { version: env!("CARGO_PKG_VERSION").to_string() });
    if !extra_context.is_none() {
        context.extend(extra_context.unwrap());
    }

    tera::Tera::one_off(&content, &context, true).unwrap()
}

fn render_template(template: &str, tera: &tera::Tera, content: String, site: &SiteMetadata, extra_context: tera::Context) -> String {
    let mut context = tera::Context::new();
    context.insert("site", &site);
    context.insert("servus", &ServusMetadata { version: env!("CARGO_PKG_VERSION").to_string() });
    context.insert("content", &content);
    context.extend(extra_context);

    return tera.render(template, &context).unwrap();
}
