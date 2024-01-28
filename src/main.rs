use async_std::prelude::*;
use chrono::NaiveDateTime;
use clap::Parser;
use http_types::mime;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    str,
    str::FromStr,
    sync::{Arc, RwLock},
};
use tide::{http::StatusCode, log, Request, Response};
use tide_acme::rustls_acme::caches::DirCache;
use tide_acme::{AcmeConfig, TideRustlsExt};
use tide_websockets::{Message, WebSocket, WebSocketConnection};
use walkdir::{DirEntry, WalkDir};
use yaml_front_matter::YamlFrontMatter;

mod nostr;

mod admin {
    include!(concat!(env!("OUT_DIR"), "/admin.rs"));
}

#[derive(Parser)]
struct Cli {
    #[clap(short('a'), long)]
    admin_domain: Option<String>,

    #[clap(short('e'), long)]
    contact_email: Option<String>,

    #[clap(short('c'), long)]
    ssl_cert: Option<String>,

    #[clap(short('k'), long)]
    ssl_key: Option<String>,

    #[clap(short('s'), long)]
    ssl_acme: bool,

    #[clap(long)]
    ssl_acme_production: bool,

    #[clap(short('p'), long)]
    port: Option<u32>,
}

#[derive(Clone, Serialize, Deserialize)]
struct ServusMetadata {
    version: String,
}

#[derive(Clone, Serialize, Eq, PartialEq)]
enum ResourceType {
    Post,
    Extra,
}

#[derive(Clone, Serialize)]
struct Resource {
    resource_type: ResourceType,
    path: String,
    url: String,
    mime: String,

    // only used for posts
    event_id: Option<String>,
    summary: Option<String>,
    #[serde(flatten)]
    tags: HashMap<String, String>,

    // only used for posts
    inner_html: Option<String>,
    slug: Option<String>,
    date: Option<NaiveDateTime>,
}

#[derive(Clone)]
struct SiteState {
    site: toml::Value,
    path: String,
    site_data: HashMap<String, serde_yaml::Value>,
    resources: Arc<RwLock<HashMap<String, Resource>>>,
    tera: Arc<RwLock<tera::Tera>>,
}

#[derive(Clone)]
struct State {
    admin_domain: Option<String>,
    sites: Arc<RwLock<HashMap<String, SiteState>>>,
}

#[derive(Deserialize, Serialize)]
struct Site {
    domain: String,
}

fn add_post_via_nostr(site_state: &SiteState, event: &nostr::Event) {
    let maybe_slug = event.get_long_form_slug();

    if maybe_slug.is_none() {
        return;
    }

    let is_draft = event.kind == nostr::EVENT_KIND_LONG_FORM_DRAFT;

    let mut events_path = PathBuf::from(&site_state.path);
    events_path.push("_events/");
    let mut path = PathBuf::from(&events_path);
    path.push(format!("{}.md", event.id));

    let slug = maybe_slug.unwrap();
    let date = event.get_long_form_published_at();
    if !is_draft {
        let post = Resource {
            resource_type: ResourceType::Post,
            path: path.display().to_string(),
            url: get_post_url(&site_state.site, &slug, date),
            mime: format!("{}", mime::HTML),
            event_id: Some(event.id.to_owned()),
            summary: event.get_long_form_summary(),
            tags: event.get_tags_hash(),
            inner_html: None,
            slug: Some(slug.to_owned()),
            date,
        };

        let resource_path = post.url.to_owned();

        let mut resources = site_state.resources.write().unwrap();
        resources.insert(resource_path, post);
    }

    fs::create_dir_all(&events_path).unwrap();
    let mut file = fs::File::create(path).unwrap();
    event.write(&mut file).unwrap();

    log::info!("Added post: {}.", &slug);
}

fn remove_post_via_nostr(site_state: &SiteState, deletion_event: &nostr::Event) -> bool {
    let mut deleted_event_id: Option<String> = None;
    for tag in &deletion_event.tags {
        if tag[0] == "e" {
            // TODO: should we also support "a" tags?
            deleted_event_id = Some(tag[1].to_owned());
        }
    }

    if deleted_event_id.is_none() {
        log::info!("No event reference found!");
        return false;
    }

    let mut event_path = PathBuf::from(&site_state.path);
    event_path.push("_events/");
    event_path.push(format!("{}.md", &deleted_event_id.clone().unwrap()));
    let mut resource_url: Option<String> = None;
    {
        let site_resources = site_state.resources.read().unwrap();
        for resource in site_resources.values() {
            if resource.event_id.is_some() && resource.event_id == deleted_event_id {
                resource_url = Some(resource.url.to_owned());
            }
        }
    }

    // NB: drafts have files but no associated "resources"
    if let Some(resource_url) = resource_url {
        site_state.resources.write().unwrap().remove(&resource_url);
    }

    if std::fs::remove_file(&event_path).is_ok() {
        log::info!("Removed event: {}!", &deleted_event_id.unwrap());
        true
    } else {
        false
    }
}

fn build_response(resource: &Resource, content: Vec<u8>) -> Response {
    let mime = mime::Mime::from_str(resource.mime.as_str()).unwrap();

    Response::builder(StatusCode::Ok)
        .content_type(mime)
        .header("Access-Control-Allow-Origin", "*")
        .body(&*content)
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

                if cmd.event.kind == nostr::EVENT_KIND_LONG_FORM
                    || cmd.event.kind == nostr::EVENT_KIND_LONG_FORM_DRAFT
                {
                    {
                        let host = request.host().unwrap().to_string();
                        let sites = request.state().sites.read().unwrap();
                        if !sites.contains_key(&host) {
                            return Ok(());
                        }
                        add_post_via_nostr(sites.get(&host).unwrap(), &cmd.event);
                    }
                    ws.send_json(&json!(vec![
                        serde_json::Value::String("OK".to_string()),
                        serde_json::Value::String(cmd.event.id.to_string()),
                        serde_json::Value::Bool(true),
                        serde_json::Value::String("".to_string())
                    ]))
                    .await
                    .unwrap();
                } else if cmd.event.kind == nostr::EVENT_KIND_DELETE {
                    let post_removed: bool;
                    {
                        let host = request.host().unwrap().to_string();
                        let sites = request.state().sites.read().unwrap();
                        if !sites.contains_key(&host) {
                            return Ok(());
                        }
                        post_removed = remove_post_via_nostr(sites.get(&host).unwrap(), &cmd.event);
                    }

                    ws.send_json(&json!(vec![
                        serde_json::Value::String("OK".to_string()),
                        serde_json::Value::String(cmd.event.id.to_string()),
                        serde_json::Value::Bool(post_removed),
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
                let mut query_posts = false;
                let mut query_drafts = false;

                for (filter_by, filter) in &cmd.filter.extra {
                    if filter_by != "kinds" {
                        log::info!("Ignoring unknown filter: {}.", filter_by);
                        continue;
                    }
                    let filter_values = filter
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|f| f.as_i64().unwrap())
                        .collect::<Vec<i64>>();
                    if filter_values.contains(&nostr::EVENT_KIND_LONG_FORM) {
                        query_posts = true;
                    }
                    if filter_values.contains(&nostr::EVENT_KIND_LONG_FORM_DRAFT) {
                        query_drafts = true;
                    }
                }

                let mut events: Vec<nostr::Event> = vec![];

                if query_posts || query_drafts {
                    // TODO: maybe the two queries should be unified?
                    let host = request.host().unwrap().to_string();
                    let sites = request.state().sites.read().unwrap();
                    if !sites.contains_key(&host) {
                        return Ok(());
                    };
                    let site = sites.get(&host).unwrap();
                    if query_posts {
                        let site_resources = site.resources.read().unwrap();
                        for post in get_posts_list(&site_resources) {
                            if let Some(event) = nostr::read_event(&post.path) {
                                events.push(event);
                            }
                        }
                    }
                    if query_drafts {
                        let mut drafts_path = PathBuf::from(&site.path);
                        drafts_path.push("_events/");
                        let drafts = match fs::read_dir(drafts_path) {
                            Ok(paths) => paths
                                .map(|r| r.unwrap().path().display().to_string())
                                .collect(),
                            _ => vec![],
                        };
                        for draft in &drafts {
                            if let Some(event) = nostr::read_event(draft) {
                                if event.kind == nostr::EVENT_KIND_LONG_FORM_DRAFT {
                                    events.push(event);
                                }
                            }
                        }
                    }
                }

                for event in &events {
                    ws.send_json(&json!([
                        serde_json::Value::String("EVENT".to_string()),
                        serde_json::Value::String(cmd.subscription_id.to_string()),
                        event.to_json(),
                    ]))
                    .await
                    .unwrap();
                }
                ws.send_json(&json!(vec!["EOSE", &cmd.subscription_id.to_string()]))
                    .await
                    .unwrap();

                log::info!(
                    "Sent {} events back for subscription {}.",
                    events.len(),
                    cmd.subscription_id
                );

                // TODO: At this point we should save the subscription and notify this client later if other posts appear.
                // For that, we probably need to introduce a dispatcher thread.
                // See: https://stackoverflow.com/questions/35673702/chat-using-rust-websocket/35785414#35785414
            }
            nostr::Message::Close(_cmd) => {
                // Nothing to do here, since we don't actually store subscriptions!
            }
        }
    }
    Ok(())
}

async fn handle_index(request: Request<State>) -> tide::Result<Response> {
    let state = &request.state();

    if state.admin_domain.is_some() {
        let admin_domain = state.admin_domain.to_owned().unwrap();
        if *request.host().unwrap() == admin_domain {
            let admin_index =
                admin::INDEX_HTML.replace("%%API_BASE_URL%%", &format!("//{}", admin_domain));
            return Ok(Response::builder(StatusCode::Ok)
                .content_type(mime::HTML)
                .body(admin_index)
                .build());
        }
    }

    let host = request.host().unwrap().to_string();
    let sites = state.sites.read().unwrap();
    let site_state = if sites.contains_key(&host) {
        sites.get(&host).unwrap()
    } else {
        return Ok(Response::new(StatusCode::NotFound));
    };
    let resource_path = "/index".to_string();
    {
        let site_resources = site_state.resources.read().unwrap();
        match site_resources.get(&resource_path) {
            Some(..) => Ok(render_and_build_response(site_state, resource_path)),
            None => Ok(Response::new(StatusCode::NotFound)),
        }
    }
}

fn render_standard_resource(
    resource_name: &str,
    site_state: &SiteState,
) -> Option<(mime::Mime, String)> {
    let site_url = site_state.site.get("url")?.as_str().unwrap();
    let site_title = site_state.site.get("title")?.as_str().unwrap();
    match resource_name {
        "robots.txt" => Some((
            mime::PLAIN,
            format!("User-agent: *\nSitemap: {}/sitemap.xml", site_url),
        )),
        "sitemap.xml" => {
            let mut response: String = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n".to_owned();
            {
                let site_resources = site_state.resources.read().unwrap();
                response.push_str("<urlset xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:schemaLocation=\"http://www.sitemaps.org/schemas/sitemap/0.9 http://www.sitemaps.org/schemas/sitemap/0.9/sitemap.xsd\" xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n");
                for resource in site_resources.values() {
                    if resource.resource_type == ResourceType::Post {
                        let mut url = resource.url.trim_end_matches("/index").to_owned();
                        if url == site_url && !url.ends_with('/') {
                            url.push('/');
                        }
                        response.push_str(&format!("    <url><loc>{}</loc></url>\n", url));
                    }
                }
                response.push_str("</urlset>");
            }
            Some((mime::XML, response))
        }
        "atom.xml" => {
            let mut response: String = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n".to_owned();
            {
                response.push_str("<feed xmlns=\"http://www.w3.org/2005/Atom\">\n");
                response.push_str(&format!("<title>{}</title>\n", site_title));
                response.push_str(&format!(
                    "<link href=\"{}/atom.xml\" rel=\"self\"/>\n",
                    site_url
                ));
                response.push_str(&format!("<link href=\"{}/\"/>\n", site_url));
                response.push_str(&format!("<id>{}</id>\n", site_url));
                let site_resources = site_state.resources.read().unwrap();
                for resource in site_resources.values() {
                    if resource.resource_type == ResourceType::Post && resource.date.is_some() {
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
                                &resource.tags.get("title").unwrap(),
                                &resource.url,
                                &resource.date.unwrap(),
                                site_url,
                                resource.slug.clone().unwrap(),
                                &resource.inner_html.clone().unwrap().to_owned()
                            )
                            .to_owned(),
                        );
                    }
                }
                response.push_str("</feed>");
            }
            Some((mime::XML, response))
        }
        _ => None,
    }
}

async fn handle_get(request: Request<State>) -> tide::Result<Response> {
    let mut path = request.param("path").unwrap();
    if path.ends_with('/') {
        path = path.strip_suffix('/').unwrap();
    }

    let host = request.host().unwrap().to_string();
    let sites = request.state().sites.read().unwrap();

    if !sites.contains_key(&host) {
        return Ok(Response::new(StatusCode::NotFound));
    }

    let site_state = sites.get(&host).unwrap();

    if let Some((mime, response)) = render_standard_resource(path, site_state) {
        return Ok(Response::builder(StatusCode::Ok)
            .content_type(mime)
            .header("Access-Control-Allow-Origin", "*")
            .body(response)
            .build());
    }

    let mut resource_path;
    {
        let site_resources = site_state.resources.read().unwrap();
        resource_path = format!("/{}", &path);
        match site_resources.get(&resource_path) {
            Some(resource) => {
                if resource.resource_type == ResourceType::Extra {
                    let raw_content = fs::read(&resource.path).unwrap();
                    Ok(build_response(resource, raw_content))
                } else {
                    Ok(render_and_build_response(site_state, resource_path))
                }
            }
            None => {
                resource_path = format!("/{}/index", &path);
                return match site_resources.get(&resource_path) {
                    Some(..) => Ok(render_and_build_response(site_state, resource_path)),
                    None => Ok(Response::new(StatusCode::NotFound)),
                };
            }
        }
    }
}

async fn handle_new_site(mut request: Request<State>) -> tide::Result<Response> {
    let site: Site = request.body_json().await.unwrap();
    let state = &request.state();

    if state.admin_domain.is_none() {
        return Ok(Response::builder(StatusCode::NotFound).build());
    }

    let admin_domain = state.admin_domain.to_owned().unwrap();

    if *request.host().unwrap() != admin_domain {
        return Ok(Response::builder(StatusCode::NotFound).build());
    }

    if state.sites.read().unwrap().contains_key(&site.domain) {
        Ok(Response::builder(StatusCode::Conflict).build())
    } else {
        let path = format!("./sites/{}", site.domain);
        fs::create_dir_all(&path).unwrap();
        fs::create_dir_all(format!("./sites/{}/_events", site.domain)).unwrap();

        let mut tera = tera::Tera::new(&format!("{}/_layouts/**/*", path)).unwrap();
        tera.autoescape_on(vec![]);

        let key = request.param("key").unwrap();
        let config_content = format!("[site]\npubkey = \"{}\"", key);
        fs::write(
            format!("./sites/{}/_config.toml", site.domain),
            &config_content,
        )
        .unwrap();

        let config: HashMap<String, toml::Value> = toml::from_str(&config_content).unwrap();
        let site_config = config.get("site").unwrap();

        let sites = &mut state.sites.write().unwrap();
        sites.insert(
            site.domain,
            SiteState {
                site: site_config.clone(),
                path,
                site_data: HashMap::new(),
                resources: Arc::new(RwLock::new(HashMap::new())),
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

async fn handle_list_sites(request: Request<State>) -> tide::Result<Response> {
    let key = request.param("key").unwrap();
    let all_sites = &request.state().sites.read().unwrap();
    let sites = all_sites
        .iter()
        .filter_map(|s| {
            let pk = s.1.site.get("pubkey")?;
            if pk.as_str().unwrap() == key {
                Some(HashMap::from([("domain", s.0)]))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    Ok(Response::builder(StatusCode::Ok)
        .content_type(mime::JSON)
        .body(json!(sites).to_string())
        .build())
}

#[async_std::main]
async fn main() -> Result<(), std::io::Error> {
    let args = Cli::parse();

    femme::with_level(log::LevelFilter::Info);

    let mut app = tide::with_state(State {
        admin_domain: args.admin_domain.clone(),
        sites: Arc::new(RwLock::new(load_sites())),
    });

    app.with(log::LogMiddleware::new());
    app.at("/")
        .with(WebSocket::new(handle_websocket))
        .get(handle_index);
    app.at("*path").get(handle_get);
    if args.admin_domain.is_some() {
        app.at("/api/keys/:key/sites").post(handle_new_site);
        app.at("/api/keys/:key/sites").get(handle_list_sites);
    }

    let addr = "0.0.0.0";

    if args.ssl_cert.is_some() && args.ssl_key.is_some() {
        let port = args.port.unwrap_or(443);
        let bind_to = format!("{addr}:{port}");
        let mut listener = tide_rustls::TlsListener::build().addrs(bind_to);
        listener = listener
            .cert(args.ssl_cert.unwrap())
            .key(args.ssl_key.unwrap());
        app.listen(listener).await?;
    } else if args.ssl_acme || args.ssl_acme_production {
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
        if args.admin_domain.is_some() {
            domains.push(args.admin_domain.unwrap());
        }
        let cache = DirCache::new("./cache");
        let acme_config = AcmeConfig::new(domains)
            .cache(cache)
            .directory_lets_encrypt(args.ssl_acme_production)
            .contact_push(format!("mailto:{}", args.contact_email.unwrap()));
        let port = args.port.unwrap_or(443);
        let bind_to = format!("{addr}:{port}");
        let mut listener = tide_rustls::TlsListener::build().addrs(bind_to);
        listener = listener.acme(acme_config);
        if !args.ssl_acme_production {
            println!("NB: Using Let's Encrypt STAGING environment! Great for testing, but browsers will complain about the certificate.");
        }
        app.listen(listener).await?;
    } else {
        let port = args.port.unwrap_or(4884);
        let bind_to = format!("{addr}:{port}");
        app.listen(bind_to).await?;
    };

    Ok(())
}

fn load_sites() -> HashMap<String, SiteState> {
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

        let resources = load_resources(&path.path(), site_config);

        println!("Site loaded!");

        sites.insert(
            path.file_name().to_str().unwrap().to_string(),
            SiteState {
                site: site_config.clone(),
                path: path.path().display().to_string(),
                site_data,
                resources: Arc::new(RwLock::new(resources)),
                tera: Arc::new(RwLock::new(tera)),
            },
        );
    }

    println!("{} sites loaded!", sites.len());

    sites
}

fn get_post_url(site: &toml::Value, slug: &str, date: Option<NaiveDateTime>) -> String {
    if date.is_some() {
        site.get("post_permalink").map_or_else(
            || format!("/posts/{}", &slug),
            |p| p.as_str().unwrap().replace(":slug", slug),
        )
    } else {
        format!("/{}", &slug)
    }
}

fn get_post(path: &Path, site: &toml::Value) -> Option<Resource> {
    let event = nostr::read_event(&path.display().to_string())?;

    let html = md_to_html(&event.content);

    let mut summary: Option<String> = None;
    let dom = tl::parse(&html, tl::ParserOptions::default()).unwrap();
    let parser = dom.parser();
    if let Some(p) = dom.query_selector("p").unwrap().next() {
        summary = Some(p.get(parser).unwrap().inner_text(parser).to_string());
    }

    let slug = event.get_long_form_slug()?;
    let date = event.get_long_form_published_at();

    let is_draft = event.kind == nostr::EVENT_KIND_LONG_FORM_DRAFT;

    if !is_draft {
        Some(Resource {
            resource_type: ResourceType::Post,
            path: path.display().to_string(),
            url: get_post_url(site, &slug, date),
            mime: format!("{}", mime::HTML),
            event_id: Some(event.id.to_owned()),
            summary,
            tags: event.get_tags_hash(),
            inner_html: Some(html.to_owned()),
            slug: Some(slug),
            date,
        })
    } else {
        None
    }
}

fn skipped(entry: &DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(|s| (s.starts_with('.') || s.starts_with('_')) && s != ".well-known")
        .unwrap_or(false)
}

fn load_posts(site_path: &PathBuf, site: &toml::Value) -> HashMap<String, Resource> {
    let mut posts = HashMap::new();

    let mut posts_path = PathBuf::from(site_path);
    posts_path.push("_events/");

    if !posts_path.is_dir() {
        return posts;
    }

    for entry in WalkDir::new(&posts_path) {
        let path = entry.unwrap().into_path();
        if !path.is_file() {
            continue;
        }
        let maybe_post = get_post(&path, site);
        if maybe_post.is_none() {
            continue;
        }
        let post = maybe_post.unwrap();
        let resource_path = post.url.to_owned();

        println!("Loaded post {} from {}", resource_path, post.path);
        posts.insert(resource_path.to_string(), post);
    }

    posts
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
    let html = md_to_html(&rendered_text);
    let layout = if resource.date.is_some() {
        "post.html".to_string()
    } else {
        "page.html".to_string()
    };

    render_template(&layout, tera, &html, site, extra_context)
        .as_bytes()
        .to_vec()
}

fn load_extra_resources(site_path: &PathBuf) -> HashMap<String, Resource> {
    let mut resources = HashMap::new();

    let walker = WalkDir::new(site_path).into_iter();
    for entry in walker.filter_entry(|e| !skipped(e)) {
        let path = entry.unwrap().into_path();

        if !path.is_file() {
            continue;
        }

        let site_prefix = site_path.display().to_string();
        let path_str = path.display().to_string();
        let extension = path.extension().unwrap().to_str().unwrap();

        let resource_path = path_str.strip_prefix(site_prefix.as_str()).unwrap();
        let content = fs::read(&path).unwrap();

        let mime = match mime::Mime::sniff(&content) {
            Ok(m) => m,
            _ => match mime::Mime::from_extension(extension) {
                Some(m) => m,
                _ => mime::PLAIN,
            },
        };

        println!(
            "Loaded resource {} from {} ({}).",
            resource_path, path_str, mime,
        );

        let resource = Resource {
            resource_type: ResourceType::Extra,
            path: path_str.to_owned(),
            url: resource_path.to_owned(),
            mime: format!("{}", mime),
            event_id: None,
            summary: None,
            tags: HashMap::new(),
            inner_html: None,
            slug: None,
            date: None,
        };

        resources.insert(resource_path.to_string(), resource);
    }

    resources
}

fn get_posts_list(resources: &HashMap<String, Resource>) -> Vec<&Resource> {
    let mut posts_list: Vec<&Resource> = resources
        .values()
        .filter(|&r| r.resource_type == ResourceType::Post)
        .collect();
    posts_list.sort_by(|a, b| b.date.cmp(&a.date));

    posts_list
}

fn load_resources(site_path: &PathBuf, site: &toml::Value) -> HashMap<String, Resource> {
    let mut resources = HashMap::new();
    resources.extend(load_posts(site_path, site));
    resources.extend(load_extra_resources(site_path));

    resources
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
