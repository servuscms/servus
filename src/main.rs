use std::{
    collections::HashMap,
    env,
    fmt,
    path::{Path, PathBuf},
    fs,
    io::{prelude::*, BufReader},
    net::{TcpListener, TcpStream},
};
use chrono::NaiveDate;
use markdown::{CompileOptions, Options, to_html_with_options};
use mime;
use mime_guess;
use serde::{Serialize, Deserialize};
use tera::Context;
use tera::Tera;
use toml;
use yaml_front_matter::{Document, YamlFrontMatter};

const BIND_ADDR: &str = "127.0.0.1:8888";

#[derive(Deserialize)]
struct SiteConfig {
    site: Site,
}

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
struct Post {
    title: String,
    slug: String,
    date: NaiveDate,
    filename: String,
}

enum HttpStatus {
    Http200,
    Http400,
    Http404,
    Http405,
}

impl fmt::Display for HttpStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            HttpStatus::Http200 => write!(f, "HTTP/1.1 200 OK"),
            HttpStatus::Http400 => write!(f, "HTTP/1.1 400 Bad Request"),
            HttpStatus::Http404 => write!(f, "HTTP/1.1 404 Not Found"),
            HttpStatus::Http405 => write!(f, "HTTP/1.1 405 Method Not Allowed"),
	}
    }
}

fn main() {
    let mut site_path = PathBuf::from(env::current_dir().unwrap());
    site_path.push("sites");
    site_path.push("default");

    let posts = get_posts(&site_path);

    let mut tera = Tera::new(&format!("{}/templates/*.html", site_path.to_str().unwrap())).unwrap();
    tera.autoescape_on(vec![]);

    let listener = TcpListener::bind(BIND_ADDR).unwrap();

    for stream in listener.incoming() {
	handle_connection(stream.unwrap(), &site_path, &posts, &tera);
    }
}

fn handle_connection(mut stream: TcpStream, site_path: &PathBuf, posts: &HashMap<String, Post>, tera: &Tera) {
    let buf_reader = BufReader::new(&mut stream);
    let mut lines = buf_reader.lines();
    let request_line = match lines.next() {
	Some(l) => l,
	None => {
	    return;
	}
    }.unwrap();

    let (method, request_path) = match request_line.split(" ").collect::<Vec<_>>()[..] {
	[method, path, "HTTP/1.1"] => (method, path),
	_ => {
	    send_response(stream, HttpStatus::Http400, mime::TEXT_HTML_UTF_8, &[].to_vec());
	    return;
	}
    };

    match method {
	"GET" => {
	    let (status, mime, content) = handle_get(&site_path, &request_path, &posts, &tera);
	    send_response(stream, status, mime, &content);
	    return;
	},
	_ => {
	    send_response(stream, HttpStatus::Http405, mime::TEXT_HTML_UTF_8, &[].to_vec());
	    return;
	},
    };
}

fn send_response(mut stream: TcpStream, status: HttpStatus, mime: mime::Mime, content: &Vec<u8>) {
    let length = content.len();
    stream.write_all(format!("{status}\r\nContent-Length: {length}\r\n").as_bytes()).unwrap();
    stream.write_all(format!("Content-Type: {mime}\r\n\r\n").as_bytes()).unwrap();
    stream.write_all(content).unwrap();
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

    return Some(
	Post {
	    title: String::from(&document.metadata.title),
	    slug: Path::new(&filename[11..]).file_stem().unwrap().to_str().unwrap().to_string(),
	    date: date,
	    filename: Path::new(&path).display().to_string(),
    });
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
    return posts;
}

fn handle_get(site_path: &PathBuf, request_path: &str, posts: &HashMap<String, Post>, tera: &Tera) -> (HttpStatus, mime::Mime, Vec<u8>) {
    let status: HttpStatus;
    let mime: mime::Mime;
    let content: Vec<u8>;
    match path_from_request_path(&site_path, request_path, &posts) {
	Ok(path) => {
	    match render_path(&site_path, &path, &posts, &tera) {
		(Some(m), c) => (status, mime, content) = (HttpStatus::Http200, m, c),
		(None, _) => {
		    status = HttpStatus::Http404;
		    (mime, content) = render_404(&site_path, &tera);
		}
	    }
	},
	Err(HttpStatus::Http404) => {
	    status = HttpStatus::Http404;
	    (mime, content) = render_404(&site_path, &tera);
	}
	Err(s) => {
	    (status, mime, content) = (s, mime::TEXT_HTML_UTF_8, [].to_vec());
	},
    }
    return (status, mime, content);
}

fn path_from_request_path(site_path: &PathBuf, request_path: &str, posts: &HashMap<String, Post>) -> Result<PathBuf, HttpStatus> {
    if !request_path.starts_with('/') {
	return Err(HttpStatus::Http400);
    }

    let mut parts: Vec<&str> = match request_path {
	"/" => vec!["index"],
	_ => request_path[1..].split("/").collect::<Vec<_>>(),
    };

    match (parts.first().unwrap(), parts.len()) {
	(&"posts", 1) => {
	    parts.insert(0, "pages"); /* /posts/ is a "page" -> /pages/posts.md */
	},
	(&"posts", _) => {
	    match posts.get(parts[1]) {
		Some(post) => {
		    return Ok(PathBuf::from(&post.filename));
		},
		None => {
		    return Err(HttpStatus::Http404);
		}
	    };
	},
	(&"static", _) => {},
	_ => {
	    /* everything that is not /static/... or /posts/... is a "page" and should be looked up under pages/ */
	    parts.insert(0, "pages");
	},
    };

    let mut path = PathBuf::from(site_path);
    for (i, part) in parts.iter().enumerate() {
	if part.starts_with('.') {
	    return Err(HttpStatus::Http400);
	}
	if i < parts.len() - 1 && !path.exists() {
	    return Err(HttpStatus::Http404);
	}
	let filename = if i < parts.len() - 1 || !part.contains('?') {
	    part.to_string()
	} else {
	    part[..part.find('?').unwrap()].to_string()
	};
	path.push(filename);
    }

    match path.extension() {
	None => {
	    for ext in &["md", "html"] {
		path.set_extension(ext);
		if path.exists() {
		    return Ok(path);
		}
	    }
	},
	_ => {
	    if path.exists() {
		return Ok(path);
	    }
	},
    }

    return Err(HttpStatus::Http404);
}

fn render_404(site_path: &PathBuf, tera: &Tera) -> (mime::Mime, Vec<u8>) {
    match render_path(&site_path, &PathBuf::from("pages/404.md"), &HashMap::new(), &tera) {
	(Some(m), c) => {
	    return (m, c);
	},
	_ => {
	    return (mime::TEXT_HTML_UTF_8, "<html><body><blink>404 page not found</blink></body></html>".as_bytes().to_vec());
	}
    }
}

fn render_path(site_path: &PathBuf, path: &PathBuf, posts: &HashMap<String, Post>, tera: &Tera) -> (Option<mime::Mime>, Vec<u8>) {
    match path.extension() {
	None => (None, [].to_vec()),
	Some(os_str) => {
	    match os_str.to_str() {
		Some("html") => (Some(mime::TEXT_HTML_UTF_8), render_html(site_path, path)),
		Some("md") => (Some(mime::TEXT_HTML_UTF_8), render_markdown(site_path, path, &posts, &tera)),
		_ => render_static(site_path, path),
	    }
	}
    }
}

fn render_html(site_path: &PathBuf, path: &PathBuf) -> Vec<u8> {
    return fs::read_to_string([site_path, path].iter().collect::<PathBuf>()).unwrap().as_bytes().to_vec();
}

fn render_markdown(site_path: &PathBuf, path: &PathBuf, posts: &HashMap<String, Post>, tera: &Tera) -> Vec<u8> {
    let md = fs::read_to_string([site_path, path].iter().collect::<PathBuf>()).unwrap();
    let document: Document<PageMetadata> = YamlFrontMatter::parse::<PageMetadata>(&md).unwrap();
    let mut context = Context::new();
    let mut site_config_path = PathBuf::from(site_path);
    site_config_path.push("config.toml");
    let site_config_content = fs::read_to_string(&site_config_path).unwrap();
    let site_config: SiteConfig = toml::from_str(&site_config_content).unwrap();
    context.insert("site", &site_config.site);
    context.insert("page", &document.metadata);
    let mut posts_list: Vec<&Post> = posts.into_iter().map(|(_k, p)| p).collect();
    posts_list.sort_by(|a, b| b.date.cmp(&a.date));
    context.insert("posts", &posts_list);
    let rendered_content = Tera::one_off(&document.content, &context, true).unwrap();
    let options = &Options {compile: CompileOptions {allow_dangerous_html: true,
						     ..CompileOptions::default()},
			    ..Options::default()};
    let html_content = &to_html_with_options(&rendered_content, &options).unwrap();
    context.insert("content", &html_content);
    return tera.render("page.html", &context).unwrap().as_bytes().to_vec();
}

fn render_static(site_path: &PathBuf, path: &PathBuf) -> (Option<mime::Mime>, Vec<u8>) {
    let mut file = fs::File::open([site_path, path].iter().collect::<PathBuf>()).unwrap();
    let mut content: Vec<u8> = Vec::with_capacity(file.metadata().unwrap().len() as usize);
    file.read_to_end(&mut content).unwrap();
    return (mime_guess::from_path(path).first(), content);
}
