---
title: My posts
description: Posts I've written
---

{% for post in posts %}
* [{{ post.title }}](/posts/{{ post.slug }}) on {{ post.published_at | date(format="%d %B %Y") }}
{% endfor %}
