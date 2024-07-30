use std::fs;

fn main() {
    println!("cargo:rerun-if-changed=build.rs,admin/index.html");

    let admin_index_html = fs::read_to_string("admin/index.html").unwrap();

    let out_dir = std::env::var_os("OUT_DIR").unwrap();

    std::fs::write(
        std::path::Path::new(&out_dir).join("admin.rs"),
        r##"
pub const INDEX_HTML: &str = r#"%%index_html%%"#;
"##
        .replace("%%index_html%%", &admin_index_html),
    )
    .unwrap();
}
