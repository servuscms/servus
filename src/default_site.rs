use std::{
    fs,
    io::Write,
    path::PathBuf,
};

const DEFAULT_CONFIG: &str = r#"
[site]
title = "servus!"
tagline = "a simple example"
url = "https://servus.page"
contact_email = "servus@servus.page"
"#;

const DEFAULT_ATOM_XML: &str = r#"<?xml version="1.0" encoding="utf-8" ?>
<feed xmlns="http://www.w3.org/2005/Atom">
    <title>{{ site.title }}</title>
    <link href="{{ site.url | safe }}/atom.xml" rel="self" />
    <link href="{{ site.url | safe }}/" />
    <id>{{ site.url | safe }}</id>

    {% for post in posts %}
    <entry>
        <title>{{ post.meta.title }}</title>
        <link href="{{ site.url | safe }}/{{ post.slug }}" />
        <updated>{{ post.date | date }}</updated>
        <id>{{ site.url | safe }}/{{ post.slug }}</id>
        <content type="xhtml">
            <div xmlns="http://www.w3.org/1999/xhtml">
                {{ post.text | safe }}
            </div>
        </content>
    </entry>
    {% endfor %}
</feed>
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
* [{{ post.meta.title }}](/posts/{{ post.slug }}) on {{ post.date | date(format="%d %B %Y") }}
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
    <div class="page">
        <h1>{{ page.meta.title }}</h1>
        {{ content }}
    </div>
{% endblock %}
"#;

const DEFAULT_POST_TEMPLATE: &str = r#"
{% extends "base.html" %}
{% block content %}
    <div class="post">
        <span class="date">{{ post.date | date(format="%d %B %Y") }}</span>
        <h1>{{ post.meta.title }}</h1>
        {{ content }}
    </div>
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
    fs::create_dir_all(get_path(site_path, ".servus")).unwrap();
    fs::create_dir_all(get_path(site_path, ".servus/templates")).unwrap();
    fs::create_dir_all(get_path(site_path, "posts")).unwrap();

    write!(fs::File::create(get_path(site_path, ".servus/config.toml")).unwrap(), "{}", DEFAULT_CONFIG).unwrap();
    write!(fs::File::create(get_path(site_path, "atom.xml")).unwrap(), "{}", DEFAULT_ATOM_XML).unwrap();
    write!(fs::File::create(get_path(site_path, "index.md")).unwrap(), "{}", DEFAULT_INDEX_PAGE).unwrap();
    write!(fs::File::create(get_path(site_path, "posts.md")).unwrap(), "{}", DEFAULT_POSTS_PAGE).unwrap();
    write!(fs::File::create(get_path(site_path, "posts/2022-12-30-servus.md")).unwrap(), "{}", DEFAULT_POST_HELLO).unwrap();
    write!(fs::File::create(get_path(site_path, ".servus/templates/page.html")).unwrap(), "{}", DEFAULT_PAGE_TEMPLATE).unwrap();
    write!(fs::File::create(get_path(site_path, ".servus/templates/post.html")).unwrap(), "{}", DEFAULT_POST_TEMPLATE).unwrap();
    write!(fs::File::create(get_path(site_path, ".servus/templates/base.html")).unwrap(), "{}", DEFAULT_BASE_TEMPLATE).unwrap();
}
