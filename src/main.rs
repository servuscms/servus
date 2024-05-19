use base64::{engine::general_purpose::STANDARD, Engine};
use clap::Parser;
use http_types::{mime, Method};
use phf::phf_set;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::HashMap,
    fs,
    fs::File,
    io::BufReader,
    path::PathBuf,
    str,
    str::FromStr,
    sync::{Arc, RwLock},
};
use tide::{http::StatusCode, log, Request, Response};
use tide_acme::rustls_acme::caches::DirCache;
use tide_acme::{AcmeConfig, TideRustlsExt};
use tide_websockets::{Message, WebSocket, WebSocketConnection};

mod admin {
    include!(concat!(env!("OUT_DIR"), "/admin.rs"));
}
mod default_theme {
    include!(concat!(env!("OUT_DIR"), "/default_theme.rs"));
}
mod content;
mod nostr;
mod site;

use site::Site;

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

#[derive(Clone)]
struct State {
    admin_domain: Option<String>,
    sites: Arc<RwLock<HashMap<String, Site>>>,
}

#[derive(Deserialize, Serialize)]
struct PostSiteRequestBody {
    domain: String,
}

static BLOSSOM_CONTENT_TYPES: phf::Set<&'static str> = phf_set! {
    "audio/mpeg",
    "image/gif",
    "image/jpeg",
    "image/png",
    "image/webp",
};

#[derive(Debug, Deserialize, Serialize)]
struct FileMetadata {
    sha256: String,
    #[serde(rename = "type")]
    content_type: String,
    size: usize,
    url: String,
}

fn build_raw_response(content: Vec<u8>, mime: mime::Mime) -> Response {
    Response::builder(StatusCode::Ok)
        .content_type(mime)
        .header("Access-Control-Allow-Origin", "*")
        .body(&*content)
        .build()
}

fn render_and_build_response(site: &Site, resource_path: String) -> Response {
    let resources = site.resources.read().unwrap();
    let event_ref = resources.get(&resource_path).unwrap();

    Response::builder(StatusCode::Ok)
        .content_type(mime::HTML)
        .header("Access-Control-Allow-Origin", "*")
        .body(&*event_ref.render(site))
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

                if cmd.event.kind == nostr::EVENT_KIND_NOTE
                    || cmd.event.kind == nostr::EVENT_KIND_LONG_FORM
                    || cmd.event.kind == nostr::EVENT_KIND_LONG_FORM_DRAFT
                {
                    {
                        let host = request.host().unwrap().to_string();
                        let sites = request.state().sites.read().unwrap();
                        if !sites.contains_key(&host) {
                            return Ok(());
                        }
                        sites.get(&host).unwrap().add_content(&cmd.event);
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
                        post_removed = sites.get(&host).unwrap().remove_content(&cmd.event);
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
                let mut events: Vec<nostr::Event> = vec![];
                for (filter_by, filter) in &cmd.filter.extra {
                    if filter_by != "kinds" {
                        log::info!("Ignoring unknown filter: {}.", filter_by);
                        continue;
                    }
                    let filter_kinds: Vec<i64> = filter
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|f| f.as_i64().unwrap())
                        .collect();

                    let host = request.host().unwrap().to_string();
                    let sites = request.state().sites.read().unwrap();
                    if !sites.contains_key(&host) {
                        return Ok(());
                    };
                    let site = sites.get(&host).unwrap();
                    let resources = site.resources.read().unwrap();
                    for resource in resources.values() {
                        // NB: we are currently only returning resources with underlying events,
                        // but we could actually return *all* resources by generating an event for them
                        // and signing it with a key from config.
                        if let Some(event_ref) = resource.event_ref.clone() {
                            if filter_kinds.contains(&event_ref.kind) {
                                if let Some((front_matter, content)) = resource.read() {
                                    if let Some(event) = nostr::parse_event(&front_matter, &content)
                                    {
                                        events.push(event);
                                    }
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
        let resources = site.resources.read().unwrap();
        match resources.get("/index") {
            Some(..) => Ok(render_and_build_response(site, "/index".to_owned())),
            None => Ok(Response::new(StatusCode::NotFound)),
        }
    }
}

async fn handle_request(request: Request<State>) -> tide::Result<Response> {
    let mut path = request.param("path").unwrap();
    if path.ends_with('/') {
        path = path.strip_suffix('/').unwrap();
    }

    let mut part: Option<String> = None;
    if path.contains(".") {
        let parts = path.split(".").collect::<Vec<_>>();
        if parts.len() == 2 {
            part = Some(parts[0].to_string());
        }
    } else {
        part = Some(path.to_string());
    }
    let mut sha256: Option<String> = None;
    if let Some(part) = part {
        if part.len() == 64 && part.chars().all(|c| char::is_ascii_alphanumeric(&c)) {
            sha256 = Some(part.to_string());
        }
    }

    if sha256.is_some() && request.method() == Method::Options {
        return Ok(Response::builder(StatusCode::Ok)
            .header("Access-Control-Allow-Origin", "*")
            .header("Access-Control-Allow-Headers", "Authorization,*")
            .header("Access-Control-Allow-Methods", "GET,PUT,DELETE")
            .build());
    }

    let host = request.host().unwrap().to_string();
    let sites = request.state().sites.read().unwrap();

    if !sites.contains_key(&host) {
        return Ok(Response::new(StatusCode::NotFound));
    }

    let site = sites.get(&host).unwrap();

    if let Some((mime, response)) = site::render_standard_resource(path, site) {
        return Ok(Response::builder(StatusCode::Ok)
            .content_type(mime)
            .header("Access-Control-Allow-Origin", "*")
            .body(response)
            .build());
    }

    let existing_posts: Vec<String>;
    {
        let posts = site.resources.read().unwrap();
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
            if PathBuf::from(&resource_path).exists() {
                // look for a static file
                let raw_content = fs::read(&resource_path).unwrap();
                let guess = mime_guess::from_path(resource_path);
                let mime = mime::Mime::from_str(guess.first().unwrap().essence_str()).unwrap();
                Ok(build_raw_response(raw_content, mime))
            } else {
                // look for an uploaded file
                if let Some(sha256) = sha256 {
                    resource_path = format!("{}/_content/files/{}", site.path, sha256);
                    if PathBuf::from(&resource_path).exists() {
                        let raw_content = fs::read(&resource_path).unwrap();
                        let metadata_file = File::open(&format!(
                            "{}/_content/files/{}.metadata.json",
                            site.path, sha256
                        ))
                        .unwrap();
                        let metadata_reader = BufReader::new(metadata_file);
                        let metadata: FileMetadata =
                            serde_json::from_reader(metadata_reader).unwrap();
                        let mime = mime::Mime::from_str(&metadata.content_type).unwrap();
                        Ok(build_raw_response(raw_content, mime))
                    } else {
                        Ok(Response::builder(StatusCode::NotFound).build())
                    }
                } else {
                    Ok(Response::builder(StatusCode::NotFound).build())
                }
            }
        }
    }
}

fn get_nostr_auth_event(request: &Request<State>) -> Option<nostr::Event> {
    let auth_header = request.header(tide::http::headers::AUTHORIZATION);
    let parts = auth_header?.as_str().split(' ').collect::<Vec<_>>();
    if parts.len() != 2 {
        return None;
    }
    if parts[0].to_lowercase() != "nostr" {
        return None;
    }

    Some(
        serde_json::from_str(str::from_utf8(&STANDARD.decode(parts[1]).unwrap()).unwrap()).unwrap(),
    )
}

fn nostr_auth(request: &Request<State>) -> Option<String> {
    get_nostr_auth_event(request)?
        .get_nip98_pubkey(request.url().as_str(), request.method().as_ref())
}

fn blossom_auth(request: &Request<State>, method: &str) -> Option<String> {
    get_nostr_auth_event(request)?.get_blossom_pubkey(method)
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
        let key = nostr_auth(&request);
        if key.is_none() {
            return Ok(Response::builder(StatusCode::BadRequest).build());
        }

        let path = format!("./sites/{}", domain);
        fs::create_dir_all(&path).unwrap();

        default_theme::generate(&format!("./sites/{}/", domain));

        let mut tera = tera::Tera::new(&format!("{}/_layouts/**/*", path)).unwrap();
        tera.autoescape_on(vec![]);

        let config_content = format!(
            "[site]\npubkey = \"{}\"\nurl = \"https://{}\"\ntitle = \"{}\"\ntagline = \"{}\"",
            key.unwrap(),
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
            resources: Arc::new(RwLock::new(HashMap::new())),
            tera: Arc::new(RwLock::new(tera)),
        };
        site.load_resources();

        let sites = &mut state.sites.write().unwrap();
        sites.insert(domain, site);

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

async fn handle_list_request(request: Request<State>) -> tide::Result<Response> {
    let site_path = {
        let host = request.host().unwrap().to_string();
        let sites = request.state().sites.read().unwrap();
        if !sites.contains_key(&host) {
            return Ok(Response::builder(StatusCode::NotFound).build());
        };

        let site = sites.get(&host).unwrap();

        let pubkey = request.param("pubkey").unwrap();
        if let Some(site_pubkey) = site.config.get("pubkey") {
            if site_pubkey.as_str().unwrap() != pubkey {
                log::info!("Invalid key.");
                return Ok(Response::builder(StatusCode::NotFound)
                    .header("Access-Control-Allow-Origin", "*")
                    .build());
            }
        } else {
            log::info!("The site has no pubkey.");
            return Ok(Response::builder(StatusCode::NotFound)
                .header("Access-Control-Allow-Origin", "*")
                .build());
        }

        site.path.clone()
    };

    let paths = match fs::read_dir(format!("{}/_content/files", site_path)) {
        Ok(paths) => paths.map(|r| r.unwrap()).collect(),
        _ => vec![],
    };

    let mut list = vec![];

    for path in &paths {
        if path.path().extension().is_none() {
            let mut metadata_path = path.path();
            metadata_path.set_extension("metadata.json");
            let metadata_file = File::open(&metadata_path).unwrap();
            let metadata_reader = BufReader::new(metadata_file);
            let metadata: FileMetadata = serde_json::from_reader(metadata_reader).unwrap();
            list.push(metadata);
        }
    }

    return Ok(Response::builder(StatusCode::Created)
        .content_type(mime::JSON)
        .header("Access-Control-Allow-Origin", "*")
        .body(serde_json::to_string(&list).unwrap())
        .build());
}

async fn handle_upload_request(mut request: Request<State>) -> tide::Result<Response> {
    if request.method() == Method::Options {
        return Ok(Response::builder(StatusCode::Ok)
            .header("Access-Control-Allow-Origin", "*")
            .header("Access-Control-Allow-Headers", "Authorization,*")
            .header("Access-Control-Allow-Methods", "GET,PUT,DELETE")
            .build());
    }

    let site_path = {
        let host = request.host().unwrap().to_string();
        let sites = request.state().sites.read().unwrap();
        if !sites.contains_key(&host) {
            return Ok(Response::builder(StatusCode::NotFound).build());
        };

        let site = sites.get(&host).unwrap();

        if let Some(pubkey) = blossom_auth(&request, "upload") {
            if let Some(site_pubkey) = site.config.get("pubkey") {
                if site_pubkey.as_str().unwrap() != pubkey {
                    log::info!("Non-matching key.");
                    return Ok(Response::builder(StatusCode::Unauthorized)
                        .header("Access-Control-Allow-Origin", "*")
                        .build());
                }
            } else {
                log::info!("The site has no pubkey.");
                return Ok(Response::builder(StatusCode::Unauthorized)
                    .header("Access-Control-Allow-Origin", "*")
                    .build());
            }
        } else {
            log::info!("Missing Blossom auth.");
            return Ok(Response::builder(StatusCode::Unauthorized)
                .header("Access-Control-Allow-Origin", "*")
                .build());
        }

        site.path.clone()
    };

    let bytes = request.body_bytes().await?;

    let hash = sha256::digest(&*bytes);

    let mime = mime::Mime::sniff(&bytes);
    if mime.is_err() || !BLOSSOM_CONTENT_TYPES.contains(mime.as_ref().unwrap().essence()) {
        return Ok(Response::builder(StatusCode::BadRequest)
            .content_type(mime::JSON)
            .header("Access-Control-Allow-Origin", "*")
            .body(json!({"message": "Unknown content type."}))
            .build());
    }

    let metadata = FileMetadata {
        sha256: hash.to_owned(),
        content_type: mime.unwrap().essence().to_owned(),
        size: bytes.len(),
        url: format!("https://{}/{}", request.host().unwrap(), hash),
    };

    fs::create_dir_all(format!("{}/_content/files", site_path)).unwrap();
    fs::write(format!("{}/_content/files/{}", site_path, hash), bytes).unwrap();
    fs::write(
        format!("{}/_content/files/{}.metadata.json", site_path, hash),
        serde_json::to_string(&metadata).unwrap(),
    )
    .unwrap();

    return Ok(Response::builder(StatusCode::Created)
        .content_type(mime::JSON)
        .header("Access-Control-Allow-Origin", "*")
        .body(serde_json::to_string(&metadata).unwrap())
        .build());
}

async fn handle_delete_request(request: Request<State>) -> tide::Result<Response> {
    let site_path = {
        let host = request.host().unwrap().to_string();
        let sites = request.state().sites.read().unwrap();
        if !sites.contains_key(&host) {
            return Ok(Response::builder(StatusCode::NotFound).build());
        };

        let site = sites.get(&host).unwrap();

        if let Some(pubkey) = blossom_auth(&request, "delete") {
            if let Some(site_pubkey) = site.config.get("pubkey") {
                if site_pubkey.as_str().unwrap() != pubkey {
                    log::info!("Non-matching key.");
                    return Ok(Response::builder(StatusCode::Unauthorized)
                        .header("Access-Control-Allow-Origin", "*")
                        .build());
                }
            } else {
                log::info!("Site has no pubkey.");
                return Ok(Response::builder(StatusCode::Unauthorized)
                    .header("Access-Control-Allow-Origin", "*")
                    .build());
            }
        } else {
            log::info!("Missing Blossom auth.");
            return Ok(Response::builder(StatusCode::Unauthorized)
                .header("Access-Control-Allow-Origin", "*")
                .build());
        }

        site.path.clone()
    };

    let hash = request.param("sha256").unwrap();

    fs::remove_file(format!("{}/_content/files/{}", site_path, hash)).unwrap();
    fs::remove_file(format!(
        "{}/_content/files/{}.metadata.json",
        site_path, hash
    ))
    .unwrap();

    return Ok(Response::builder(StatusCode::Created)
        .content_type(mime::JSON)
        .header("Access-Control-Allow-Origin", "*")
        .body(json!({}))
        .build());
}

#[async_std::main]
async fn main() -> Result<(), std::io::Error> {
    let args = Cli::parse();

    femme::with_level(log::LevelFilter::Info);

    let mut app = tide::with_state(State {
        admin_domain: args.admin_domain.clone(),
        sites: Arc::new(RwLock::new(site::load_sites())),
    });

    app.with(log::LogMiddleware::new());
    app.at("/")
        .with(WebSocket::new(handle_websocket))
        .get(handle_index);
    app.at("*path").options(handle_request).get(handle_request);
    app.at("/upload")
        .options(handle_upload_request)
        .put(handle_upload_request);
    app.at("/list/:pubkey").get(handle_list_request);
    app.at("/:sha256").delete(handle_delete_request);
    if args.admin_domain.is_some() {
        app.at("/api/sites")
            .post(handle_post_site)
            .get(handle_get_sites);
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
