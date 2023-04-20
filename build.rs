use std::fs;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let config_toml = fs::read_to_string("themes/blank/_config.toml").unwrap();
    let atom_xml = fs::read_to_string("themes/blank/atom.xml").unwrap();
    let robots_txt = fs::read_to_string("themes/blank/robots.txt").unwrap();
    let sitemap_xml = fs::read_to_string("themes/blank/sitemap.xml").unwrap();
    let index_md = fs::read_to_string("themes/blank/index.md").unwrap();
    let posts_md = fs::read_to_string("themes/blank/posts.md").unwrap();
    let base_html = fs::read_to_string("themes/blank/_layouts/base.html").unwrap();
    let page_html = fs::read_to_string("themes/blank/_layouts/page.html").unwrap();
    let post_html = fs::read_to_string("themes/blank/_layouts/post.html").unwrap();
    let post_servus = fs::read_to_string("themes/blank/_posts/2022-12-30-servus.md").unwrap();

    let out_dir = std::env::var_os("OUT_DIR").unwrap();
    std::fs::write(
        std::path::Path::new(&out_dir).join("themes.rs"),
        r##"
use std::{fs, io::Write};
fn get_path(site_path: &str, extra: &str) -> std::path::PathBuf {
    [site_path, extra].iter().collect()
}
const CONFIG_TOML: &str = r#"%%config_toml%%"#;
const ATOM_XML: &str = r#"%%atom_xml%%"#;
const ROBOTS_TXT: &str = r#"%%robots_txt%%"#;
const SITEMAP_XML: &str = r#"%%sitemap_xml%%"#;
const INDEX_MD: &str = r#"%%index_md%%"#;
const POSTS_MD: &str = r#"%%posts_md%%"#;
const BASE_HTML: &str = r#"%%base_html%%"#;
const PAGE_HTML: &str = r#"%%page_html%%"#;
const POST_HTML: &str = r#"%%post_html%%"#;
const POST_SERVUS: &str = r#"%%post_servus%%"#;

pub fn generate(site_path: &str) {
    fs::create_dir_all(site_path).unwrap();

    write!(fs::File::create(get_path(site_path, "_config.toml")).unwrap(), "{}", CONFIG_TOML).unwrap();
    write!(fs::File::create(get_path(site_path, "atom.xml")).unwrap(), "{}", ATOM_XML).unwrap();
    write!(fs::File::create(get_path(site_path, "robots.txt")).unwrap(), "{}", ROBOTS_TXT).unwrap();
    write!(fs::File::create(get_path(site_path, "sitemap.xml")).unwrap(), "{}", SITEMAP_XML).unwrap();
    write!(fs::File::create(get_path(site_path, "index.md")).unwrap(), "{}", INDEX_MD).unwrap();
    write!(fs::File::create(get_path(site_path, "posts.md")).unwrap(), "{}", POSTS_MD).unwrap();

    fs::create_dir_all(get_path(site_path, "_layouts")).unwrap();
    write!(fs::File::create(get_path(site_path, "_layouts/base.html")).unwrap(), "{}", BASE_HTML).unwrap();
    write!(fs::File::create(get_path(site_path, "_layouts/page.html")).unwrap(), "{}", PAGE_HTML).unwrap();
    write!(fs::File::create(get_path(site_path, "_layouts/post.html")).unwrap(), "{}", POST_HTML).unwrap();

    fs::create_dir_all(get_path(site_path, "_posts")).unwrap();
    write!(fs::File::create(get_path(site_path, "_posts/2022-12-30-servus.md")).unwrap(), "{}", POST_SERVUS).unwrap();
}
"##.replace("%%config_toml%%", &config_toml).replace("%%atom_xml%%", &atom_xml).replace("%%robots_txt%%", &robots_txt).replace("%%sitemap_xml%%", &sitemap_xml).replace("%%index_md%%", &index_md).replace("%%posts_md%%", &posts_md).replace("%%base_html%%", &base_html).replace("%%page_html%%", &page_html).replace("%%post_html%%", &post_html).replace("%%post_servus%%", &post_servus),
    )
    .unwrap();
}
