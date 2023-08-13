use async_std::prelude::*;
use chrono::{NaiveDate, NaiveDateTime};
use clap::Parser;
use http_types::mime;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::HashMap,
    env, fs,
    io::prelude::*,
    path::{Path, PathBuf},
    str,
    str::FromStr,
    sync::{Arc, RwLock},
};
use tide::{http::StatusCode, log, Redirect, Request, Response};
use tide_acme::rustls_acme::caches::DirCache;
use tide_acme::{AcmeConfig, TideRustlsExt};
use tide_websockets::{Message, WebSocket, WebSocketConnection};
use walkdir::{DirEntry, WalkDir};
use yaml_front_matter::YamlFrontMatter;

mod nostr;

#[derive(clap::ValueEnum, Clone)]
pub enum Mode {
    Dev,
    Live,
}

#[derive(Parser)]
struct Cli {
    #[clap(short, long)]
    api_domain: Option<String>,

    #[clap(short('e'), long)]
    contact_email: Option<String>,

    #[clap(short('c'), long)]
    ssl_cert: Option<String>,

    #[clap(short('k'), long)]
    ssl_key: Option<String>,

    #[clap(short('s'), long)]
    ssl_acme: bool,

    #[clap(value_enum)]
    mode: Mode,
}

#[derive(Clone, Serialize, Deserialize)]
struct ServusMetadata {
    version: String,
}

#[derive(Clone, Serialize, Eq, PartialEq)]
enum ResourceType {
    Post,
    Page,
    Extra,
}

#[derive(Clone, Serialize)]
struct Content {
    is_raw: bool,
    content: Option<Vec<u8>>,
}

#[derive(Clone, Serialize)]
struct Resource {
    resource_type: ResourceType,
    path: String,
    url: String,
    mime: String,

    // only used for posts and pages
    redirect_to: Option<String>,
    summary: Option<String>,
    #[serde(flatten)]
    front_matter: HashMap<String, serde_yaml::Value>,

    // only used for posts
    inner_html: Option<String>,
    slug: Option<String>,
    date: Option<NaiveDate>,
}

#[derive(Clone)]
struct SiteState {
    site: toml::Value,
    path: String,
    site_data: HashMap<String, serde_yaml::Value>,
    resources: Arc<RwLock<HashMap<String, Resource>>>,
    content: Arc<RwLock<HashMap<String, Content>>>,
    tera: Arc<RwLock<tera::Tera>>,
}

#[derive(Clone)]
struct State {
    api_domain: Option<String>,
    sites: Arc<RwLock<HashMap<String, SiteState>>>,
}

#[derive(Deserialize, Serialize)]
struct Site {
    subdomain: String,
    pubkey: String,
}

fn add_post_from_nostr(site_state: &SiteState, event: &nostr::Event) {
    let mut slug = String::new();
    let mut title = String::new();
    let mut summary = None;
    let mut published_at = chrono::offset::Utc::now().naive_local().date();
    for tag in &event.tags {
        match tag[0].as_str() {
            "d" => slug = tag[1].to_owned(),
            "title" => title = tag[1].to_owned(),
            "summary" => summary = Some(tag[1].to_owned()),
            "published_at" => {
                let secs = tag[1].parse::<i64>().unwrap();
                published_at = NaiveDateTime::from_timestamp_opt(secs, 0).unwrap().date();
            }
            _ => {}
        }
    }

    if slug.is_empty() || title.is_empty() {
        return;
    }

    let mut front_matter = HashMap::<String, serde_yaml::Value>::new();
    front_matter.insert("title".to_string(), serde_yaml::Value::String(title));

    let mut posts_path = PathBuf::from(&site_state.path);
    posts_path.push("_posts/");
    let base_filename = format!("{}-{}", published_at.format("%Y-%m-%d"), &slug);

    let mut path = PathBuf::from(&posts_path);
    path.push(format!("{}.md", base_filename));

    let post = Resource {
        resource_type: ResourceType::Post,
        path: path.display().to_string(),
        url: get_post_url(&site_state.site, &slug),
        mime: format!("{}", mime::HTML),
        redirect_to: None,
        summary,
        front_matter: front_matter.to_owned(),
        inner_html: None,
        slug: Some(slug.to_owned()),
        date: Some(published_at),
    };
    let mut extra_context = tera::Context::new();
    extra_context.insert("page", &post);
    extra_context.insert("data", &site_state.site_data);

    let html = md_to_html(&event.content);

    let resource_path = post.url.to_owned();

    let mut resources = site_state.resources.write().unwrap();
    resources.insert(resource_path.clone(), post);

    let content = render_template(
        "post.html",
        &mut site_state.tera.write().unwrap(),
        &html,
        &site_state.site,
        extra_context,
    )
    .as_bytes()
    .to_vec();

    let post_content = Content {
        content: Some(content),
        is_raw: false,
    };

    let mut site_content = site_state.content.write().unwrap();
    site_content.insert(resource_path, post_content);

    for c in site_content.values_mut() {
        if !c.is_raw {
            c.content = None;
        }
    }

    let mut file = fs::File::create(path).unwrap();
    file.write_all(b"---\n").unwrap();
    serde_yaml::to_writer(&file, &front_matter).unwrap();
    file.write_all(b"---\n").unwrap();
    file.write_all(event.content.as_bytes()).unwrap();

    let mut event_path = PathBuf::from(&posts_path);
    event_path.push(format!("{}.json", base_filename));

    let event_file = fs::File::create(event_path).unwrap();
    serde_json::to_writer(&event_file, &event).unwrap();

    log::info!("Added post: {}.", &slug);
}

fn build_response(resource: &Resource, content: &Content) -> Response {
    let mime = mime::Mime::from_str(resource.mime.as_str()).unwrap();

    Response::builder(StatusCode::Ok)
        .content_type(mime)
        .header("Access-Control-Allow-Origin", "*")
        .body(&*content.content.as_ref().unwrap().clone())
        .build()
}

fn render_and_build_response(site_state: &SiteState, resource_path: String) -> Response {
    let site_resources = site_state.resources.read().unwrap();
    let resource = site_resources.get(&resource_path).unwrap();

    let file_content = fs::read_to_string(&resource.path).unwrap();
    let (text, _front_matter) = parse_front_matter(&file_content);

    let rendered_content;

    {
        let mut tera = site_state.tera.write().unwrap();
        let posts_list = get_posts_list(&site_resources);
        rendered_content = render_page(
            resource,
            &posts_list,
            &site_state.site_data,
            &text,
            &site_state.site,
            &mut tera,
        );
    }

    {
        let mut site_content = site_state.content.write().unwrap();
        let content = site_content.get_mut(&resource_path).unwrap();
        content.content = Some(rendered_content.to_owned());
    }

    Response::builder(StatusCode::Ok)
        .content_type(mime::Mime::from_str(resource.mime.as_str()).unwrap())
        .header("Access-Control-Allow-Origin", "*")
        .body(&*rendered_content)
        .build()
}

async fn handle_websocket(
    request: Request<State>,
    mut ws: WebSocketConnection,
) -> tide::Result<()> {
    while let Some(Ok(Message::Text(message))) = ws.next().await {
        let parsed: nostr::Message = serde_json::from_str(&message).unwrap();
        match parsed {
            nostr::Message::Event(cmd) => {
                {
                    let host = request.host().unwrap().to_string();
                    let sites = request.state().sites.read().unwrap();
                    if !sites.contains_key(&host) {
                        return Ok(());
                    }
                    if let Some(site_pubkey) = sites.get(&host).unwrap().site.get("pubkey") {
                        if cmd.event.pubkey != site_pubkey.as_str().unwrap() {
                            log::info!("Ignoring event for unknown pubkey: {}.", cmd.event.pubkey);
                            continue;
                        }
                    } else {
                        log::info!("Ignoring event because site has no pubkey.");
                        continue;
                    }
                }

                if cmd.event.validate_sig().is_err() {
                    log::info!("Ignoring invalid event.");
                    continue;
                }

                if cmd.event.kind == 30023 {
                    {
                        let host = request.host().unwrap().to_string();
                        let sites = request.state().sites.read().unwrap();
                        if !sites.contains_key(&host) {
                            return Ok(());
                        }
                        add_post_from_nostr(sites.get(&host).unwrap(), &cmd.event);
                    }
                    ws.send_json(&json!(vec![
                        serde_json::Value::String("OK".to_string()),
                        serde_json::Value::String(cmd.event.id.to_string()),
                        serde_json::Value::Bool(true),
                        serde_json::Value::String("".to_string())
                    ]))
                    .await
                    .unwrap();
                } else {
                    log::info!("Ignoring event of unknown kind: {}.", cmd.event.kind);
                    continue;
                }
            }
            nostr::Message::Req(cmd) => {
                let mut posts_subscription: Option<String> = None;
                for (filter_by, filter) in &cmd.filter.extra {
                    if filter_by != "kinds" {
                        log::info!("Ignoring unknown filter: {}.", filter_by);
                        continue;
                    }
                    let filter_values = filter
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|f| f.as_u64().unwrap())
                        .collect::<Vec<u64>>();
                    if filter_values.contains(&30023) {
                        posts_subscription = Some(cmd.subscription_id.to_owned());
                    } else {
                        log::info!(
                            "Ignoring subscription for unknown kinds: {}.",
                            filter_values
                                .iter()
                                .map(|f| f.to_string())
                                .collect::<Vec<String>>()
                                .join(", ")
                        );
                        continue;
                    }
                }
                if let Some(subscription_id) = posts_subscription {
                    let mut events: Vec<String> = vec![];
                    {
                        let host = request.host().unwrap().to_string();
                        let sites = request.state().sites.read().unwrap();
                        if !sites.contains_key(&host) {
                            return Ok(());
                        };
                        let site_resources = sites.get(&host).unwrap().resources.read().unwrap();
                        for post in get_posts_list(&site_resources) {
                            let mut path = PathBuf::from(&post.path);
                            path.set_extension("json");
                            if let Ok(json) = fs::read_to_string(&path) {
                                events.push(json);
                            }
                        }
                    }

                    for event in &events {
                        ws.send_json(&json!(vec!["EVENT", &subscription_id, event]))
                            .await
                            .unwrap();
                    }
                    ws.send_json(&json!(vec!["EOSE", &subscription_id]))
                        .await
                        .unwrap();

                    log::info!(
                        "Sent {} events back for subscription {}.",
                        events.len(),
                        subscription_id
                    );

                    // TODO: At this point we should save the subscription and notify this client later if other posts appear.
                    // For that, we probably need to introduce a dispatcher thread.
                    // See: https://stackoverflow.com/questions/35673702/chat-using-rust-websocket/35785414#35785414
                }
            }
            nostr::Message::Close(_cmd) => {
                // Nothing to do here, since we don't actually store subscriptions!
            }
        }
    }
    Ok(())
}

async fn handle_index(request: Request<State>) -> tide::Result<Response> {
    let host = request.host().unwrap().to_string();
    let sites = request.state().sites.read().unwrap();
    let site_state = if sites.contains_key(&host) {
        sites.get(&host).unwrap()
    } else {
        return Ok(Response::new(StatusCode::NotFound));
    };
    let resource_path = "/index".to_string();
    {
        let site_resources = site_state.resources.read().unwrap();
        let site_content = site_state.content.read().unwrap();
        match site_resources.get(&resource_path) {
            Some(resource) => {
                if resource.redirect_to.is_some() {
                    return Ok(Redirect::new(resource.redirect_to.as_ref().unwrap()).into());
                }
                let content = site_content.get(&resource_path).unwrap();
                if content.content.is_some() {
                    return Ok(build_response(resource, content));
                }
            }
            None => return Ok(Response::new(StatusCode::NotFound)),
        }
    }

    Ok(render_and_build_response(site_state, resource_path))
}

async fn handle_get(request: Request<State>) -> tide::Result<Response> {
    let mut path = request.param("path").unwrap();
    if path.ends_with('/') {
        path = path.strip_suffix('/').unwrap();
    }

    let host = request.host().unwrap().to_string();
    let sites = request.state().sites.read().unwrap();
    let site_state = if sites.contains_key(&host) {
        sites.get(&host).unwrap()
    } else {
        return Ok(Response::new(StatusCode::NotFound));
    };

    let mut resource_path;
    {
        let site_resources = site_state.resources.read().unwrap();
        let site_content = site_state.content.read().unwrap();
        resource_path = format!("/{}", &path);
        match site_resources.get(&resource_path) {
            Some(resource) => {
                if resource.redirect_to.is_some() {
                    return Ok(Redirect::new(resource.redirect_to.as_ref().unwrap()).into());
                }
                let content = site_content.get(&resource_path).unwrap();
                if content.content.is_some() {
                    return Ok(build_response(resource, content));
                }
            }
            None => {
                resource_path = format!("/{}/index", &path);
                match site_resources.get(&resource_path) {
                    Some(resource) => {
                        let content = site_content.get(&resource_path).unwrap();
                        if content.content.is_some() {
                            return Ok(build_response(resource, content));
                        }
                    }
                    None => return Ok(Response::new(StatusCode::NotFound)),
                };
            }
        }
    }

    Ok(render_and_build_response(site_state, resource_path))
}

async fn handle_api(mut request: Request<State>) -> tide::Result<Response> {
    let site: Site = request.body_json().await.unwrap();
    let state = &request.state();

    if state.api_domain.is_none() {
        return Ok(Response::builder(StatusCode::NotFound).build());
    }

    let api_domain = state.api_domain.to_owned().unwrap();

    if *request.host().unwrap() != api_domain {
        return Ok(Response::builder(StatusCode::NotFound).build());
    }

    if site.subdomain.contains('.') {
        return Ok(Response::builder(StatusCode::BadRequest)
            .content_type(mime::JSON)
            .body("{'message': 'Invalid subdomain.'}")
            .build());
    }

    let new_site = format!("{}.{}", site.subdomain, api_domain);
    if state.sites.read().unwrap().contains_key(&new_site) {
        Ok(Response::builder(StatusCode::Conflict).build())
    } else {
        let path = format!("./sites/{}", new_site);
        fs::create_dir_all(&path).unwrap();
        fs::create_dir_all(format!("./sites/{}/_posts", new_site)).unwrap();

        let mut tera = tera::Tera::new(&format!("{}/_layouts/**/*", path)).unwrap();
        tera.autoescape_on(vec![]);

        let config_content = format!("[site]\npubkey = \"{}\"", site.pubkey);
        fs::write(
            format!("./sites/{}/_config.toml", new_site),
            &config_content,
        )
        .unwrap();

        let config: HashMap<String, toml::Value> = toml::from_str(&config_content).unwrap();
        let site_config = config.get("site").unwrap();

        let sites = &mut state.sites.write().unwrap();
        sites.insert(
            new_site,
            SiteState {
                site: site_config.clone(),
                path,
                site_data: HashMap::new(),
                resources: Arc::new(RwLock::new(HashMap::new())),
                content: Arc::new(RwLock::new(HashMap::new())),
                tera: Arc::new(RwLock::new(tera)),
            },
        );
        Ok(Response::builder(StatusCode::Ok)
            .content_type(mime::JSON)
            .header("Access-Control-Allow-Origin", "*")
            .body("{}")
            .build())
    }
}

#[async_std::main]
async fn main() -> Result<(), std::io::Error> {
    let args = Cli::parse();

    femme::with_level(log::LevelFilter::Info);

    let mut app = tide::with_state(State {
        api_domain: args.api_domain.clone(),
        sites: Arc::new(RwLock::new(get_sites())),
    });

    app.with(log::LogMiddleware::new());
    app.at("/").with(WebSocket::new(handle_websocket));
    app.at("/").get(handle_index);
    app.at("*path").get(handle_get);
    if args.api_domain.is_some() {
        app.at("/api/sites").post(handle_api);
    }

    let addr = "0.0.0.0";
    let mut port = 443;
    let mut ssl = true;
    let mut dev_mode = false;
    let mut production_cert = false; // default to staging

    match args.mode {
        Mode::Dev => {
            port = 4884;
            ssl = false;
            dev_mode = true;
        }
        Mode::Live => {
            production_cert = true;
        }
    }

    if dev_mode {
        println!("Open http://localhost:{} in your browser!", port);
    } else {
        println!("Running on port {}", port);
        if !production_cert {
            println!("Using Let's Encrypt staging environment. Great for testing, but browsers will complain about the certificate.");
        }
    }

    let bind_to = format!("{addr}:{port}");

    if ssl {
        let mut listener = tide_rustls::TlsListener::build().addrs(bind_to);
        listener = if args.ssl_cert.is_some() && args.ssl_key.is_some() {
            listener
                .cert(args.ssl_cert.unwrap())
                .key(args.ssl_key.unwrap())
        } else if args.ssl_acme {
            if args.contact_email.is_none() {
                panic!("Use -e to provide a contact email!");
            }
            let mut domains: Vec<String> = app
                .state()
                .sites
                .read()
                .unwrap()
                .keys()
                .map(|x| x.to_string())
                .collect();
            if args.api_domain.is_some() {
                domains.push(args.api_domain.unwrap());
            }
            let cache = DirCache::new("./cache");
            let acme_config = AcmeConfig::new(domains)
                .cache(cache)
                .directory_lets_encrypt(production_cert)
                .contact_push(format!("mailto:{}", args.contact_email.unwrap()));
            listener.acme(acme_config)
        } else {
            panic!("Pass either --ssh-cert and --ssl-key OR --ssl-acme");
        };
        app.listen(listener).await?;
    } else {
        app.listen(bind_to).await?;
    }

    Ok(())
}

fn get_sites() -> HashMap<String, SiteState> {
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

        let (resources, content) = load_resources(&path.path(), site_config, &site_data, &mut tera);

        sites.insert(
            path.file_name().to_str().unwrap().to_string(),
            SiteState {
                site: site_config.clone(),
                path: path.path().display().to_string(),
                site_data,
                resources: Arc::new(RwLock::new(resources)),
                content: Arc::new(RwLock::new(content)),
                tera: Arc::new(RwLock::new(tera)),
            },
        );
    }

    sites
}

fn get_post_url(site: &toml::Value, slug: &str) -> String {
    site.get("post_permalink").map_or_else(
        || format!("/posts/{}", &slug),
        |p| p.as_str().unwrap().replace(":slug", slug),
    )
}

fn get_post(
    path: &Path,
    site: &toml::Value,
    site_data: &HashMap<String, serde_yaml::Value>,
    tera: &mut tera::Tera,
) -> Option<(Resource, Content)> {
    let filename = path.file_name().unwrap().to_str().unwrap();
    let extension = path.extension().unwrap().to_str().unwrap();
    if extension != "md" {
        return None;
    }
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

    let file_content = fs::read_to_string(path.display().to_string()).unwrap();

    let (text, front_matter) = parse_front_matter(&file_content);
    if front_matter.is_empty() {
        println!("Empty front matter for {}. Skipping post!", path.display());
        return None;
    }

    let html = md_to_html(&text);
    let slug = Path::new(&filename[11..])
        .file_stem()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let mut summary: Option<String> = None;
    let dom = tl::parse(&html, tl::ParserOptions::default()).unwrap();
    let parser = dom.parser();
    if let Some(p) = dom.query_selector("p").unwrap().next() {
        summary = Some(p.get(parser).unwrap().inner_text(parser).to_string());
    }

    let redirect_to = front_matter
        .get("redirect_to")
        .map(|r| r.as_str().unwrap().to_owned());

    let resource = Resource {
        resource_type: ResourceType::Post,
        path: path.display().to_string(),
        url: get_post_url(site, &slug),
        mime: format!("{}", mime::HTML),
        redirect_to,
        summary,
        front_matter,
        inner_html: Some(html.to_owned()),
        slug: Some(slug),
        date: Some(date),
    };

    let mut extra_context = tera::Context::new();
    extra_context.insert("page", &resource);
    extra_context.insert("data", &site_data);

    let content = Content {
        content: Some(
            render_template("post.html", tera, &html, site, extra_context)
                .as_bytes()
                .to_vec(),
        ),
        is_raw: false,
    };

    Some((resource, content))
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
    tera: &mut tera::Tera,
) -> (HashMap<String, Resource>, HashMap<String, Content>) {
    let mut posts = HashMap::new();
    let mut posts_content = HashMap::new();

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
        let (post, post_content) = maybe_post.unwrap();
        let resource_path = post.url.to_owned();

        println!("Loaded post {} from {}", resource_path, post.path);
        posts.insert(resource_path.to_string(), post);
        posts_content.insert(resource_path, post_content);
    }

    (posts, posts_content)
}

fn render_page(
    resource: &Resource,
    posts: &Vec<&Resource>,
    site_data: &HashMap<String, serde_yaml::Value>,
    text: &str,
    site: &toml::Value,
    tera: &mut tera::Tera,
) -> Vec<u8> {
    let mut extra_context = tera::Context::new();
    extra_context.insert("page", resource);
    extra_context.insert("posts", &posts);
    extra_context.insert("data", &site_data);

    let rendered_text = render(text, site, Some(extra_context.clone()), tera);
    let extension = Path::new(&resource.path)
        .extension()
        .unwrap()
        .to_str()
        .unwrap();
    let html = if extension == "md" {
        md_to_html(&rendered_text)
    } else {
        rendered_text
    };

    let layout = match resource.front_matter.get("layout") {
        Some(layout) => format!("{}.html", layout.as_str().unwrap()),
        _ => "page.html".to_string(),
    };

    render_template(&layout, tera, &html, site, extra_context)
        .as_bytes()
        .to_vec()
}

fn load_pages(
    site_path: &PathBuf,
    site: &toml::Value,
    site_data: &HashMap<String, serde_yaml::Value>,
    tera: &mut tera::Tera,
    posts: &Vec<&Resource>,
) -> (HashMap<String, Resource>, HashMap<String, Content>) {
    let mut pages = HashMap::new();
    let mut pages_content = HashMap::new();

    let mut posts_path = PathBuf::from(site_path);
    posts_path.push("_posts/");

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

        let file_content = fs::read_to_string(&path).unwrap();
        let (text, front_matter) = parse_front_matter(&file_content);
        if front_matter.is_empty() {
            println!("Empty front matter for {}. Skipping page!", path.display());
            continue;
        }

        let resource_path = if front_matter.get("permalink").is_some() {
            let permalink = front_matter.get("permalink").unwrap().as_str().unwrap();
            if permalink.ends_with('/') {
                // TODO: we might want to make this configurable
                permalink.strip_suffix('/').unwrap().to_string()
            } else {
                permalink.to_string()
            }
        } else {
            path_str
                .strip_prefix(site_prefix.as_str())
                .unwrap()
                .strip_suffix(&format!(".{}", extension))
                .unwrap()
                .to_string()
        };

        let canonical_resource_path = match resource_path.strip_suffix("/index") {
            Some(s) => format!("{}/", s),
            None => resource_path.to_owned(),
        };

        let redirect_to = front_matter
            .get("redirect_to")
            .map(|r| r.as_str().unwrap().to_owned());

        let resource = Resource {
            resource_type: ResourceType::Page,
            path: path.display().to_string(),
            url: canonical_resource_path,
            mime: format!("{}", mime::HTML),
            redirect_to,
            summary: None,
            front_matter,
            inner_html: None,
            slug: None,
            date: None,
        };

        let content = Content {
            content: Some(render_page(&resource, posts, site_data, &text, site, tera)),
            is_raw: false,
        };

        println!("Loaded page {} from {}", resource_path, resource.path);
        pages.insert(resource_path.to_string(), resource);
        pages_content.insert(resource_path.to_string(), content);
    }

    (pages, pages_content)
}

fn load_extra_resources(
    site_path: &PathBuf,
    site: &toml::Value,
    tera: &mut tera::Tera,
    posts: &Vec<&Resource>,
    pages: &Vec<&Resource>,
) -> (HashMap<String, Resource>, HashMap<String, Content>) {
    let mut resources = HashMap::new();
    let mut resources_content = HashMap::new();

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
        let extension = path.extension().unwrap().to_str().unwrap();

        let resource_path = path_str.strip_prefix(site_prefix.as_str()).unwrap();
        let mime;
        let content;
        let is_raw = match extension {
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

                false
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

                true
            }
        };

        println!(
            "Loaded resource {} from {} ({} bytes={})",
            resource_path,
            path_str,
            mime,
            content.len()
        );

        let resource = Resource {
            resource_type: ResourceType::Extra,
            path: path_str.to_owned(),
            url: resource_path.to_owned(),
            mime: format!("{}", mime),
            redirect_to: None,
            summary: None,
            front_matter: HashMap::new(),
            inner_html: None,
            slug: None,
            date: None,
        };

        resources.insert(resource_path.to_string(), resource);
        resources_content.insert(
            resource_path.to_string(),
            Content {
                content: Some(content),
                is_raw,
            },
        );
    }

    (resources, resources_content)
}

fn get_posts_list(resources: &HashMap<String, Resource>) -> Vec<&Resource> {
    let mut posts_list: Vec<&Resource> = resources
        .values()
        .filter(|&r| r.resource_type == ResourceType::Post)
        .collect();
    posts_list.sort_by(|a, b| b.date.cmp(&a.date));

    posts_list
}

fn load_resources(
    site_path: &PathBuf,
    site: &toml::Value,
    site_data: &HashMap<String, serde_yaml::Value>,
    tera: &mut tera::Tera,
) -> (HashMap<String, Resource>, HashMap<String, Content>) {
    let (posts, posts_content) = load_posts(site_path, site, site_data, tera);

    let posts_list = get_posts_list(&posts);

    let (pages, pages_content) = load_pages(site_path, site, site_data, tera, &posts_list);

    let pages_list: Vec<&Resource> = pages.values().collect();

    let (extra, extra_content) =
        load_extra_resources(site_path, site, tera, &posts_list, &pages_list);

    let mut resources = HashMap::new();
    resources.extend(posts);
    resources.extend(pages);
    resources.extend(extra);

    let mut content = HashMap::new();
    content.extend(posts_content);
    content.extend(pages_content);
    content.extend(extra_content);

    (resources, content)
}

fn parse_front_matter(content: &String) -> (String, HashMap<String, serde_yaml::Value>) {
    match YamlFrontMatter::parse::<HashMap<String, serde_yaml::Value>>(content) {
        Ok(document) => (document.content, document.metadata),
        _ => (content.to_string(), HashMap::new()),
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
