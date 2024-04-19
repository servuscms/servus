use std::fs;

fn main() {
    println!("cargo:rerun-if-changed=build.rs,admin/index.html");

    let admin_index_html = fs::read_to_string("admin/index.html").unwrap();

    let index_md = fs::read_to_string("themes/default/_content/pages/index.md").unwrap();
    let posts_md = fs::read_to_string("themes/default/_content/pages/posts.md").unwrap();

    let base_html = fs::read_to_string("themes/default/_layouts/base.html").unwrap();
    let note_html = fs::read_to_string("themes/default/_layouts/note.html").unwrap();
    let page_html = fs::read_to_string("themes/default/_layouts/page.html").unwrap();
    let post_html = fs::read_to_string("themes/default/_layouts/post.html").unwrap();

    let style_css = fs::read_to_string("themes/default/style.css").unwrap();

    let out_dir = std::env::var_os("OUT_DIR").unwrap();

    std::fs::write(
        std::path::Path::new(&out_dir).join("admin.rs"),
        r##"
pub const INDEX_HTML: &str = r#"%%index_html%%"#;
"##
        .replace("%%index_html%%", &admin_index_html),
    )
    .unwrap();

    std::fs::write(
        std::path::Path::new(&out_dir).join("default_theme.rs"),
        r##"
use std::{fs, io::Write};
fn get_path(site_path: &str, extra: &str) -> std::path::PathBuf {
    [site_path, extra].iter().collect()
}
const INDEX_MD: &str = r"%%index_md%%";
const POSTS_MD: &str = r"%%posts_md%%";
const BASE_HTML: &str = r#"%%base_html%%"#;
const NOTE_HTML: &str = r#"%%note_html%%"#;
const PAGE_HTML: &str = r#"%%page_html%%"#;
const POST_HTML: &str = r#"%%post_html%%"#;
const STYLE_CSS: &str = r#"%%style_css%%"#;

pub fn generate(site_path: &str) {
    fs::create_dir_all(get_path(site_path, "_content/pages")).unwrap();
    fs::create_dir_all(get_path(site_path, "_layouts")).unwrap();
    write!(fs::File::create(get_path(site_path, "_content/pages/index.md")).unwrap(), "{}", INDEX_MD).unwrap();
    write!(fs::File::create(get_path(site_path, "_content/pages/posts.md")).unwrap(), "{}", POSTS_MD).unwrap();
    write!(fs::File::create(get_path(site_path, "_layouts/base.html")).unwrap(), "{}", BASE_HTML).unwrap();
    write!(fs::File::create(get_path(site_path, "_layouts/note.html")).unwrap(), "{}", NOTE_HTML).unwrap();
    write!(fs::File::create(get_path(site_path, "_layouts/page.html")).unwrap(), "{}", PAGE_HTML).unwrap();
    write!(fs::File::create(get_path(site_path, "_layouts/post.html")).unwrap(), "{}", POST_HTML).unwrap();
    write!(fs::File::create(get_path(site_path, "style.css")).unwrap(), "{}", STYLE_CSS).unwrap();
}
"##.replace("%%index_md%%", &index_md).replace("%%posts_md%%", &posts_md).replace("%%base_html%%", &base_html).replace("%%note_html%%", &note_html).replace("%%page_html%%", &page_html).replace("%%post_html%%", &post_html).replace("%%style_css%%", &style_css),
    )
    .unwrap();
}
