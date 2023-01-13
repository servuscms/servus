use std::{
    fs,
    io::Write,
    path::PathBuf,
};

const DEFAULT_CONFIG: &str = r#"
[site]
title = "servus!"
tagline = "a simple example"
url = "https://servus.page/"
contact_email = "servus@servus.page"
"#;

const DEFAULT_INDEX_PAGE: &str = r#"
---
title: servus
---

Servus!
"#;

const DEFAULT_POSTS_PAGE: &str = r#"
---
title: My posts
description: Posts I've written
---

{% for post in posts %}
* [{{ post.title }}](/posts/{{ post.slug }}) on {{ post.date }}
{% endfor %}
"#;

const DEFAULT_POST_HELLO: &str = r#"
---
title: Servus, world!

---
Servus, all! It's almost 2023 and it's time for yet another blogging engine!

This one will truly be a game changer...
"#;

const DEFAULT_PAGE_TEMPLATE: &str = r#"
{% extends "base.html" %}
{% block content %}
  <h1>{{ page.title }}</h1>
  {{ content }}
{% endblock %}
"#;

const DEFAULT_POST_TEMPLATE: &str = r#"
{% extends "base.html" %}
{% block content %}
  <h1>{{ page.title }}</h1>
  {{ content }}
{% endblock %}
"#;

const DEFAULT_BASE_TEMPLATE: &str = r#"
<!DOCTYPE html>
<html lang="en-us">
    <head>
        <meta http-equiv="content-type" content="text/html; charset=utf-8">
    </head>
    <body>
        <div>
            <strong><a href="/">{{ site.title }}</a></strong> | <a href="/posts">posts</a>
        </div>
        <div>
            {% block content %}
            {% endblock content %}
        </div>
    </body>
</html>
"#;

fn get_path(site_path: &str, extra: &str) -> PathBuf {
    [site_path, extra].iter().collect()
}

pub fn generate(site_path: &str) {
    fs::create_dir_all(get_path(site_path, "pages")).unwrap();
    fs::create_dir_all(get_path(site_path, "posts")).unwrap();
    fs::create_dir_all(get_path(site_path, "templates")).unwrap();

    write!(fs::File::create(get_path(site_path, "config.toml")).unwrap(), "{}", DEFAULT_CONFIG).unwrap();
    write!(fs::File::create(get_path(site_path, "pages/index.md")).unwrap(), "{}", DEFAULT_INDEX_PAGE).unwrap();
    write!(fs::File::create(get_path(site_path, "pages/posts.md")).unwrap(), "{}", DEFAULT_POSTS_PAGE).unwrap();
    write!(fs::File::create(get_path(site_path, "posts/2022-12-30-servus.md")).unwrap(), "{}", DEFAULT_POST_HELLO).unwrap();
    write!(fs::File::create(get_path(site_path, "templates/page.html")).unwrap(), "{}", DEFAULT_PAGE_TEMPLATE).unwrap();
    write!(fs::File::create(get_path(site_path, "templates/post.html")).unwrap(), "{}", DEFAULT_POST_TEMPLATE).unwrap();
    write!(fs::File::create(get_path(site_path, "templates/base.html")).unwrap(), "{}", DEFAULT_BASE_TEMPLATE).unwrap();
}
