use base64::{engine::general_purpose::STANDARD, Engine};
use bytes::Bytes;
use chrono::NaiveDateTime;
use clap::Parser;
use futures_util::stream::once;
use http_types::{mime, Method};
use multer::Multipart;
use phf::phf_map;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::convert::Infallible;
use std::{
    collections::HashMap,
    env, fs,
    path::PathBuf,
    str,
    str::FromStr,
    sync::{Arc, RwLock},
};
use tide::{http::StatusCode, log, Request, Response};
use tide_acme::rustls_acme::caches::DirCache;
use tide_acme::{AcmeConfig, TideRustlsExt};
use tide_websockets::{Message, WebSocket, WebSocketConnection};
use walkdir::WalkDir;
use yaml_front_matter::YamlFrontMatter;

mod nostr;

mod admin {
    include!(concat!(env!("OUT_DIR"), "/admin.rs"));
}

static FILE_EXTENSIONS: phf::Map<&'static str, &'static str> = phf_map! {
    "image/png" => "png",
    "image/jpeg" => "jpg",
    "image/gif" => "gif",
    "audio/mpeg" => "mp3",
};

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

#[derive(Clone, Serialize)]
struct Post {
    event_id: String,
    slug: String,
    path: String,
    url: String,
    summary: Option<String>,
    inner_html: String,
    date: Option<NaiveDateTime>,
    #[serde(flatten)]
    tags: HashMap<String, String>,
}

#[derive(Clone)]
struct Site {
    path: String,
    config: toml::Value,
    data: HashMap<String, serde_yaml::Value>,
    posts: Arc<RwLock<HashMap<String, Post>>>,
    tera: Arc<RwLock<tera::Tera>>,
}

#[derive(Clone)]
struct State {
    admin_domain: Option<String>,
    sites: Arc<RwLock<HashMap<String, Site>>>,
}

#[derive(Deserialize, Serialize)]
struct PostSiteRequestBody {
    domain: String,
}

fn add_post(site: &Site, event: &nostr::Event) {
    match event.get_long_form_slug() {
        None => (),
        Some(slug) => {
            let mut event_path = PathBuf::from(&site.path);
            event_path.push("_events/");
            fs::create_dir_all(&event_path).unwrap();
            let mut path = PathBuf::from(&event_path);
            path.push(format!("{}.md", event.id));

            let date = event.get_long_form_published_at();
            if event.kind != nostr::EVENT_KIND_LONG_FORM_DRAFT {
                let post = Post {
                    event_id: event.id.to_owned(),
                    slug: slug.to_owned(),
                    path: path.display().to_string(),
                    url: get_post_url(&site.config, &slug, date),
                    summary: event.get_long_form_summary(),
                    inner_html: md_to_html(&event.content),
                    date,
                    tags: event.get_tags_hash(),
                };

                let mut posts = site.posts.write().unwrap();
                if posts.contains_key(&post.url) {
                    let mut old_path = event_path;
                    old_path.push(format!("{}.md", posts.get(&post.url).unwrap().event_id));
                    std::fs::remove_file(&old_path).unwrap();
                }
                posts.insert(post.url.to_owned(), post);
            }

            let mut file = fs::File::create(path).unwrap();
            event.write(&mut file).unwrap();

            log::info!("Added post: {}.", &slug);
        }
    }
}

fn remove_post(site: &Site, deletion_event: &nostr::Event) -> bool {
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

    let deleted_event_id = deleted_event_id.unwrap();

    let mut event_path = PathBuf::from(&site.path);
    event_path.push("_events/");
    event_path.push(format!("{}.md", &deleted_event_id.clone()));
    let mut post_url: Option<String> = None;
    {
        let site_posts = site.posts.read().unwrap();
        for post in site_posts.values() {
            if post.event_id == deleted_event_id {
                post_url = Some(post.url.to_owned());
            }
        }
    }

    // NB: drafts have files but no associated Post
    if let Some(post_url) = post_url {
        site.posts.write().unwrap().remove(&post_url);
    }

    if std::fs::remove_file(&event_path).is_ok() {
        log::info!("Removed event: {}!", &deleted_event_id);
        true
    } else {
        false
    }
}

fn build_raw_response(filename: &str, content: Vec<u8>) -> Response {
    let guess = mime_guess::from_path(filename);
    let mime = mime::Mime::from_str(guess.first().unwrap().essence_str()).unwrap();
    Response::builder(StatusCode::Ok)
        .content_type(mime)
        .header("Access-Control-Allow-Origin", "*")
        .body(&*content)
        .build()
}

fn render_and_build_response(site: &Site, post_path: String) -> Response {
    let posts = site.posts.read().unwrap();
    let post = posts.get(&post_path).unwrap();

    let file_content = fs::read_to_string(&post.path).unwrap();
    let (text, _front_matter) = parse_front_matter(&file_content);

    let rendered_content = {
        let mut tera = site.tera.write().unwrap();
        let mut posts_list: Vec<&Post> = posts.values().collect();
        posts_list.sort_by(|a, b| b.date.cmp(&a.date));

        let mut extra_context = tera::Context::new();
        extra_context.insert("page", post);
        extra_context.insert("posts", &posts_list);
        extra_context.insert("data", &site.data);

        let rendered_text = render(&text, &site.config, Some(extra_context.clone()), &mut tera);
        let html = md_to_html(&rendered_text);
        let layout = if post.date.is_some() {
            "post.html".to_string()
        } else {
            "page.html".to_string()
        };

        render_template(&layout, &mut tera, &html, &site.config, extra_context)
            .as_bytes()
            .to_vec()
    };

    Response::builder(StatusCode::Ok)
        .content_type(mime::HTML)
        .header("Access-Control-Allow-Origin", "*")
        .body(&*rendered_content)
        .build()
}

async fn handle_websocket(
    request: Request<State>,
    mut ws: WebSocketConnection,
) -> tide::Result<()> {
    while let Some(Ok(Message::Text(message))) = async_std::stream::StreamExt::next(&mut ws).await {
        let parsed: nostr::Message = serde_json::from_str(&message).unwrap();
        match parsed {
            nostr::Message::Event(cmd) => {
                {
                    let host = request.host().unwrap().to_string();
                    let sites = request.state().sites.read().unwrap();
                    if !sites.contains_key(&host) {
                        return Ok(());
                    }
                    if let Some(site_pubkey) = sites.get(&host).unwrap().config.get("pubkey") {
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
                        add_post(sites.get(&host).unwrap(), &cmd.event);
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
                        post_removed = remove_post(sites.get(&host).unwrap(), &cmd.event);
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
                        let posts = site.posts.read().unwrap();
                        let mut posts_list: Vec<&Post> = posts.values().collect();
                        posts_list.sort_by(|a, b| b.date.cmp(&a.date));
                        for post in posts_list {
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
    let site = if sites.contains_key(&host) {
        sites.get(&host).unwrap()
    } else {
        return Ok(Response::new(StatusCode::NotFound));
    };

    {
        let posts = site.posts.read().unwrap();
        match posts.get("/index") {
            Some(..) => Ok(render_and_build_response(site, "/index".to_owned())),
            None => Ok(Response::new(StatusCode::NotFound)),
        }
    }
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
    let posts = site.posts.read().unwrap();
    response.push_str("<urlset xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:schemaLocation=\"http://www.sitemaps.org/schemas/sitemap/0.9 http://www.sitemaps.org/schemas/sitemap/0.9/sitemap.xsd\" xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n");
    for post in posts.values() {
        let mut url = post.url.trim_end_matches("/index").to_owned();
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
    let posts = site.posts.read().unwrap();
    for post in posts.values() {
        if post.date.is_some() {
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
                    &post.tags.get("title").unwrap(),
                    &post.url,
                    &post.date.unwrap(),
                    site_url,
                    post.slug.clone(),
                    &post.inner_html.to_owned()
                )
                .to_owned(),
            );
        }
    }
    response.push_str("</feed>");

    (mime::XML, response)
}

fn render_standard_resource(resource_name: &str, site: &Site) -> Option<(mime::Mime, String)> {
    let site_url = site.config.get("url")?.as_str().unwrap();
    match resource_name {
        "robots.txt" => Some(render_robots_txt(site_url)),
        ".well-known/nostr.json" => Some(render_nostr_json(site)),
        "sitemap.xml" => Some(render_sitemap_xml(site_url, site)),
        "atom.xml" => Some(render_atom_xml(site_url, site)),
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

    let site = sites.get(&host).unwrap();

    if let Some((mime, response)) = render_standard_resource(path, site) {
        return Ok(Response::builder(StatusCode::Ok)
            .content_type(mime)
            .header("Access-Control-Allow-Origin", "*")
            .body(response)
            .build());
    }

    let existing_posts: Vec<String>;
    {
        let posts = site.posts.read().unwrap();
        existing_posts = posts.keys().cloned().collect();
    }
    let mut resource_path = format!("/{}", &path);
    if existing_posts.contains(&resource_path) {
        Ok(render_and_build_response(site, resource_path))
    } else {
        resource_path = format!("{}/index", &resource_path);
        if existing_posts.contains(&resource_path) {
            Ok(render_and_build_response(site, resource_path))
        } else {
            resource_path = format!("{}/{}", site.path, path);
            for part in resource_path.split('/').collect::<Vec<_>>() {
                let first_char = part.chars().next().unwrap();
                if first_char == '_' || (first_char == '.' && part.len() > 1) {
                    return Ok(Response::builder(StatusCode::NotFound).build());
                }
            }
            let raw_content = fs::read(&resource_path).unwrap();
            Ok(build_raw_response(&resource_path, raw_content))
        }
    }
}

fn nostr_auth(request: &Request<State>) -> Option<String> {
    let auth_header = request.header(tide::http::headers::AUTHORIZATION);
    let parts = auth_header?.as_str().split(' ').collect::<Vec<_>>();
    if parts.len() != 2 {
        return None;
    }
    if parts[0].to_lowercase() != "nostr" {
        return None;
    }

    let event: nostr::Event =
        serde_json::from_str(str::from_utf8(&STANDARD.decode(parts[1]).unwrap()).unwrap()).unwrap();

    event.get_nip98_pubkey(request.url().as_str(), request.method().as_ref())
}

async fn handle_post_site(mut request: Request<State>) -> tide::Result<Response> {
    let domain = request
        .body_json::<PostSiteRequestBody>()
        .await
        .unwrap()
        .domain;
    let state = &request.state();

    if state.admin_domain.is_none() {
        return Ok(Response::builder(StatusCode::NotFound).build());
    }

    let admin_domain = state.admin_domain.to_owned().unwrap();

    if *request.host().unwrap() != admin_domain {
        return Ok(Response::builder(StatusCode::NotFound).build());
    }

    if state.sites.read().unwrap().contains_key(&domain) {
        Ok(Response::builder(StatusCode::Conflict).build())
    } else {
        let path = format!("./sites/{}", domain);
        fs::create_dir_all(&path).unwrap();

        let mut tera = tera::Tera::new(&format!("{}/_layouts/**/*", path)).unwrap();
        tera.autoescape_on(vec![]);

        let key = nostr_auth(&request);
        if key.is_none() {
            return Ok(Response::builder(StatusCode::BadRequest).build());
        }

        let config_content = format!(
            "[site]\npubkey = \"{}\"\nurl = \"https://{}\"",
            key.unwrap(),
            domain
        );
        fs::write(format!("./sites/{}/_config.toml", domain), &config_content).unwrap();

        let site_config = toml::from_str::<HashMap<String, toml::Value>>(&config_content).unwrap();
        let sites = &mut state.sites.write().unwrap();
        sites.insert(
            domain,
            Site {
                config: site_config.get("site").unwrap().clone(),
                path,
                data: HashMap::new(),
                posts: Arc::new(RwLock::new(HashMap::new())),
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

async fn handle_get_sites(request: Request<State>) -> tide::Result<Response> {
    let key = nostr_auth(&request);
    if key.is_none() {
        return Ok(Response::builder(StatusCode::BadRequest).build());
    }
    let key = key.unwrap();
    let all_sites = &request.state().sites.read().unwrap();
    let sites = all_sites
        .iter()
        .filter_map(|s| {
            let pk = s.1.config.get("pubkey")?;
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

async fn handle_post_file(mut request: Request<State>) -> tide::Result<Response> {
    if request.method() == Method::Options {
        return Ok(Response::builder(StatusCode::Ok)
            .header("Access-Control-Allow-Origin", "*")
            .header("Access-Control-Allow-Headers", "Authorization")
            .build());
    }

    let site_path = {
        let host = request.host().unwrap().to_string();
        let sites = request.state().sites.read().unwrap();
        if !sites.contains_key(&host) {
            return Ok(Response::builder(StatusCode::NotFound).build());
        };

        let site = sites.get(&host).unwrap();

        if let Some(pubkey) = nostr_auth(&request) {
            if let Some(site_pubkey) = site.config.get("pubkey") {
                if site_pubkey.as_str().unwrap() != pubkey {
                    return Ok(Response::builder(StatusCode::Unauthorized)
                        .header("Access-Control-Allow-Origin", "*")
                        .build());
                }
            } else {
                return Ok(Response::builder(StatusCode::Unauthorized)
                    .header("Access-Control-Allow-Origin", "*")
                    .build());
            }
        } else {
            return Ok(Response::builder(StatusCode::Unauthorized)
                .header("Access-Control-Allow-Origin", "*")
                .build());
        }

        site.path.clone()
    };

    let content_type = request
        .header(tide::http::headers::CONTENT_TYPE)
        .unwrap()
        .as_str();
    let boundary_index = content_type.find("boundary=").unwrap();
    let boundary: String = content_type
        .chars()
        .skip(boundary_index)
        .skip(String::from("boundary=").len())
        .collect();

    let bytes = request.body_bytes().await?;
    let stream = once(async move { Result::<Bytes, Infallible>::Ok(Bytes::from(bytes)) });

    let mut multipart = Multipart::new(stream, boundary);
    while let Some(field) = multipart.next_field().await.unwrap() {
        if field.name().unwrap() == "file" {
            let content = field.bytes().await.unwrap();
            let hash = sha256::digest(&*content);
            let mime = mime::Mime::sniff(&content);
            if mime.is_err() || !FILE_EXTENSIONS.contains_key(mime.as_ref().unwrap().essence()) {
                return Ok(Response::builder(StatusCode::BadRequest)
                    .content_type(mime::JSON)
                    .header("Access-Control-Allow-Origin", "*")
                    .body(json!({"status": "error", "message": "Unknown content type."}))
                    .build());
            }
            let extension = FILE_EXTENSIONS.get(mime.unwrap().essence()).unwrap();
            fs::create_dir_all(format!("{}/files", site_path)).unwrap();
            fs::write(
                format!("{}/files/{}.{}", site_path, hash, extension),
                content,
            )
            .unwrap();
            let url = format!(
                "https://{}/files/{}.{}",
                request.host().unwrap(),
                hash,
                extension
            );
            return Ok(Response::builder(StatusCode::Created)
               .content_type(mime::JSON)
               .header("Access-Control-Allow-Origin", "*")
               .body(json!({"status": "success", "nip94_event": {"tags": [["url", url], ["ox", hash]]}}).to_string())
               .build());
        }
    }

    Ok(Response::builder(StatusCode::BadRequest)
        .content_type(mime::JSON)
        .header("Access-Control-Allow-Origin", "*")
        .body(json!({"status": "error", "message": "File not found."}))
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
        app.at("/api/sites")
            .post(handle_post_site)
            .get(handle_get_sites);
        app.at("/api/files")
            .options(handle_post_file)
            .post(handle_post_file);
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

fn load_sites() -> HashMap<String, Site> {
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
            let f = std::fs::File::open(data_path.path()).unwrap();
            let data: serde_yaml::Value = serde_yaml::from_reader(f).unwrap();
            site_data.insert(data_name, data);
        }

        let config: HashMap<String, toml::Value> = toml::from_str(&config_content).unwrap();
        let site_config = config.get("site").unwrap();

        let posts = load_posts(&path.path(), site_config);

        println!("Site loaded!");

        sites.insert(
            path.file_name().to_str().unwrap().to_string(),
            Site {
                config: site_config.clone(),
                path: path.path().display().to_string(),
                data: site_data,
                posts: Arc::new(RwLock::new(posts)),
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

fn load_posts(site_path: &PathBuf, site: &toml::Value) -> HashMap<String, Post> {
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

        if let Some(event) = nostr::read_event(&path.display().to_string()) {
            if let Some(slug) = event.get_long_form_slug() {
                let date = event.get_long_form_published_at();
                if event.kind != nostr::EVENT_KIND_LONG_FORM_DRAFT {
                    let post = Post {
                        event_id: event.id.to_owned(),
                        path: path.display().to_string(),
                        url: get_post_url(site, &slug, date),
                        summary: event.get_long_form_summary(),
                        inner_html: md_to_html(&event.content),
                        date,
                        slug,
                        tags: event.get_tags_hash(),
                    };
                    println!("Loaded post {} from {}", &post.url, post.path);
                    posts.insert(post.url.to_string(), post);
                }
            }
        }
    }

    posts
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
