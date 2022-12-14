use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
    fs,
    sync::{Arc, RwLock},
};
use chrono::NaiveDate;
use http_types::mime;
use markdown;
use serde::{Serialize, Deserialize};
use tera;
use tide::{Request, Response};
use toml;
use yaml_front_matter::{Document, YamlFrontMatter};

const BIND_ADDR: &str = "0.0.0.0:8888";

#[derive(Deserialize)]
struct SiteConfig {
    site: Site,
}

#[derive(Clone)]
#[derive(Serialize)]
#[derive(Deserialize)]
struct Site {
    title: String,
    tagline: String,
    url: String,
    baseurl: String,
}

#[derive(Serialize)]
#[derive(Deserialize)]
struct PageMetadata {
    title: String,
    description: Option<String>,
}

#[derive(Serialize)]
struct Page {
    title: String,
    path: String,
    filename: String,
}

#[derive(Serialize)]
struct Post {
    title: String,
    slug: String,
    date: NaiveDate,
    filename: String,
}

#[derive(Clone)]
struct SiteState {
    path: PathBuf,
    site: Site,
    posts: Arc<RwLock<HashMap<String, Post>>>,
    pages: Arc<RwLock<HashMap<String, Page>>>,
    tera: tera::Tera,
}

#[derive(Clone)]
struct State {
    sites: HashMap<String, SiteState>,
}

#[async_std::main]
async fn main() -> Result<(), std::io::Error> {
    femme::start();

    let mut app = tide::with_state(State { sites: get_sites() });
    app.with(tide::log::LogMiddleware::new());

    app.at("/posts/:slug").get(|request: Request<State>| async move {
	let site = &request.state().sites["default"];
        let slug = request.param("slug")?;
	let response = match site.posts.read().unwrap().get(&slug as &str) {
	    Some(post) => {
		let body = render_markdown(&site, &post.filename);
		Response::builder(200).content_type(mime::HTML).body(body).build()
	    },
	    None => {
		Response::new(404)
	    }
	};
	Ok(response)
    });
    app.at("/").get(|request: Request<State>| async move {
	let site = &request.state().sites["default"];
	let pages = site.pages.read().unwrap();
	let index = pages.get("index").unwrap();
	let body = render_markdown(&site, &index.filename);
	let response = Response::builder(200).content_type(mime::HTML).body(body).build();
	Ok(response)
    });
    app.at("/*path").get(|request: Request<State>| async move {
	let site = &request.state().sites["default"];
	let path = request.param("path")?;
	let response = match site.pages.read().unwrap().get(&path as &str) {
	    Some(page) => {
		let body = render_markdown(&site, &page.filename);
		Response::builder(200).content_type(mime::HTML).body(body).build()
	    },
	    None => {
		let mut file_path = PathBuf::from(&site.path);
		file_path.push(path);

		let content = match fs::read(&file_path) {
		    Ok(content) => content,
		    _ => {
			return Ok(Response::new(404));
		    }
		};

		let content_type = match mime::Mime::sniff(&content) {
		    Ok(m) => m,
		    _ => {
			mime::Mime::from_extension(&file_path.extension().unwrap().to_str().unwrap()).unwrap()
		    }
		};

		Response::builder(200).content_type(content_type).body(content).build()
	    }
	};
	Ok(response)
    });

    app.listen(BIND_ADDR).await?;

    Ok(())
}

fn get_sites() -> HashMap<String, SiteState> {
    // NB: only load the "default" site for now

    let mut site_path = PathBuf::from(env::current_dir().unwrap());
    site_path.push("sites");
    site_path.push("default");

    let mut site_config_path = PathBuf::from(&site_path);
    site_config_path.push("config.toml");
    let site_config: SiteConfig = toml::from_str(&fs::read_to_string(&site_config_path).unwrap()).unwrap();

    let posts = get_posts(&site_path);
    let pages = get_pages(&site_path);

    let mut tera = tera::Tera::new(&format!("{}/templates/*.html", site_path.to_str().unwrap())).unwrap();
    tera.autoescape_on(vec![]);

    let mut sites = HashMap::new();
    sites.insert("default".to_string(),
		 SiteState {
		     path: site_path,
		     site: site_config.site,
		     posts: Arc::new(RwLock::new(posts)),
		     pages: Arc::new(RwLock::new(pages)),
		     tera: tera,
		 });

    sites
}

fn get_post(path: &PathBuf) -> Option<Post> {
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

    let content = match fs::read_to_string(&path.display().to_string()) {
	Ok(content) => content,
	_ => {
	    println!("Cannot read from: {}. Skipping!", path.display());
	    return None;
	}
    };

    let document = match YamlFrontMatter::parse::<PageMetadata>(&content) {
	Ok(document) => document,
	_ => {
	    println!("Invalid post: {}. Skipping!", path.display());
	    return None;
	}
    };

    Some(
	Post {
	    title: String::from(&document.metadata.title),
	    slug: Path::new(&filename[11..]).file_stem().unwrap().to_str().unwrap().to_string(),
	    date: date,
	    filename: Path::new(&path).display().to_string(),
    })
}

fn get_posts(site_path: &PathBuf) -> HashMap<String, Post> {
    let mut posts = HashMap::new();
    let mut path = PathBuf::from(&site_path);
    path.push("posts");

    for p in fs::read_dir(&path).unwrap() {
	match get_post(&p.unwrap().path()) {
	    Some(post) => {
		posts.insert(post.slug.to_string(), post);
	    }
	    None => {
		continue;
	    }
	};
    }

    posts
}

fn get_page(path: &PathBuf) -> Option<Page> {
    let filename = path.file_name().unwrap().to_str().unwrap();
    let content = match fs::read_to_string(&path.display().to_string()) {
	Ok(content) => content,
	_ => {
	    println!("Cannot read from: {}. Skipping!", path.display());
	    return None;
	}
    };

    let document = match YamlFrontMatter::parse::<PageMetadata>(&content) {
	Ok(document) => document,
	_ => {
	    println!("Invalid post: {}. Skipping!", path.display());
	    return None;
	}
    };

    return Some(
	Page {
	    title: String::from(&document.metadata.title),
	    path: Path::new(&filename).file_stem().unwrap().to_str().unwrap().to_string(),
	    filename: Path::new(&path).display().to_string(),
    });
}

fn get_pages(site_path: &PathBuf) -> HashMap<String, Page> {
    let mut pages = HashMap::new();
    let mut path = PathBuf::from(&site_path);
    path.push("pages");

    for p in fs::read_dir(&path).unwrap() {
	match get_page(&p.unwrap().path()) {
	    Some(page) => {
		pages.insert(page.path.to_string(), page);
	    }
	    None => {
		continue;
	    }
	};
    }

    pages
}

fn render_markdown(site_state: &SiteState, path: &str) -> Vec<u8> {
    let md = fs::read_to_string([&site_state.path, &PathBuf::from(path)].iter().collect::<PathBuf>()).unwrap();
    let document: Document<PageMetadata> = YamlFrontMatter::parse::<PageMetadata>(&md).unwrap();

    let mut context = tera::Context::new();
    context.insert("site", &site_state.site);
    context.insert("page", &document.metadata);
    let posts = site_state.posts.read().unwrap();
    let mut posts_list: Vec<&Post> = posts.values().into_iter().collect();
    posts_list.sort_by(|a, b| b.date.cmp(&a.date));
    context.insert("posts", &posts_list);

    let rendered_content = tera::Tera::one_off(&document.content, &context, true).unwrap();
    let options = &markdown::Options {compile: markdown::CompileOptions {allow_dangerous_html: true,
									 ..markdown::CompileOptions::default()},
				      ..markdown::Options::default()};
    let html_content = &markdown::to_html_with_options(&rendered_content, &options).unwrap();
    context.insert("content", &html_content);

    site_state.tera.render("page.html", &context).unwrap().as_bytes().to_vec()
}
