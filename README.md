# Servus

![alt text](https://github.com/servuscms/servus.page/blob/master/images/logo.png?raw=true)

## About

**Servus** is a **CMS** and **personal Nostr relay** fully self-contained within one executable file.

### What?! And why??

#### CMS

The CMS part is heavily inspired by [Jekyll](https://jekyllrb.com/). I used to be a big fan of Jekyll and recommended it to anyone asking for an easy way to build a website... until I realized that Jekyll, as awesome as it is, is actually not very friendly with beginners for a few reasons: 1) lack of an admin interface ("I should use an *editor* to create a file?") 2) the *build step* which takes a long time - no immediate feedback 3) needing to run a web server to serve Jekyll's output (sure, GitHub pages would also do, but even setting that up is too much for many people).

At the same time, these beginners that find Jekyll hard to use, think WordPress is easier. And as much as I dislike WP, I had to admit... it was 1) easier to install (copy a bunch of files, versus fighting Ruby Gems and configuring nginx) 2) had a decent admin interface 3) click "Save" and see the result live!

But I pretty much refuse to run WordPress or to recommend it to anyone. So, I had to come up with an alternative...

Unlike WordPress, **Servus** does not require MySQL or Apache.

Unlike Jekyll, **Servus** does not have a build step, does not require nginx or even GitHub pages, does not require you messing with Ruby Gems or Docker and comes with an admin interface where you can manage your content.

#### Personal Nostr Relay

While I think the Nostr protocol is a step forward from RSS/Atom and ActivityPub and will eventually supersede both, writing apps that make use of the Nostr protocol is still widely misunderstood. People use Nostr clients to post to Nostr relays they have absolutely no control over. This model may work for Twitter clones or messaging apps where you don't need long term persistence of the content... But I want to be in control of my own data and know my data is there to stay.

**Servus** is the "canonical" source of my data, which it exposes as a Nostr relay.

Always remember, the *T* in Nostr stands for "transmitted". Relays are used to *relay* information, not to *store* information!

## Goals and non-goals

Saying "let's build a CMS" is like saying "let's build a housing unit" in that 1) it's nothing new and 2) it is *extremely vague*. Therefore, defining the goals and non-goals of *this particular* CMS is essential for staying on track. Also, by reading these points, you can quickly decide whether Servus suits your particular needs or pick up one of the other 999 CMSes available to choose from...

### Goals

* **Single executable** that you can `scp` to a bare VPS and it will just work. Without Docker, without MySQL, without Python venv, without Node or PHP, without setting up an nginx reverse proxy and whatnot... You shouldn't need any of that to self-host your personal website!
* All content and settings stored as **plain text**. Except, of course, images or other media you have as content. Not in a SQL database, not in "the cloud", not in "some Nostr relays"... but in plain text files on the machine running Servus.
* As a corolary of the above, a *full backup* is just an `rsync` command... or a `.zip` file. Download a copy of it to your laptop, write a script that imports it to another CMS, search it, copy-paste parts of it to other places...
* All content served to the readers is **plain HTML served over HTTP(S)**. No Javascript that generates UI elements on the client side, no Javascript that queries Nostr relays or uses background HTTP requests to get content from the server. What you get is a plain "website" that you can open in any web browser or even using `wget`.
* The **admin interface** however is a Javascript client-side app, because signing of Nostr events has to be done by your web browser. You don't want your Nostr private key sitting around on some VPS.
* **Support for "themes"**. *Simple* doesn't mean ugly nor does it mean it should be limited in any way. Avoiding unnecessary client-side technologies doesn't mean the websites built using Servus need to look "old school" or be limited in functionality. In fact, themes *can* use Javascript *if they want to* - for certain effects, etc. The goal is to not *require* Javascript as part of the overall architecture, not to avoid it at any cost.
* **Multiple websites** that can be separately administered in one instance. So you will be able to, for example, self-host your personal website, your business's website and have your uncle host his blog, all with a single Servus instance.

### Performance and limitations

Being first and foremost a *web-based CMS* and then a *personal Nostr relay*, **the (perceived) performance for the visitors of web pages** hosted using Servus is the most important.

As mentioned above, the web browser does not need to run any client-side code or make any additional requests to get the full experience! Plain HTML, CSS + any images, etc... It is also very easy to put a CDN in front of Servus and make requests even faster because of this very reason (static pages with no dependence on external requests)!

**Servus** does **not** aim to be a performant general-purpose Nostr relay - one that can efficiently ingest huge numbers of events, execute random queries or stream back events for subscriptions in real-time. There are others much better at that!

The *Nostr relay* offered by Servus is very limited! It should be **fast to get all events belonging to a website**... but it may be slow or even impossible to make more complex queries. Also, you don't get streaming of new events coming in after a query has been issued! After existing events are returned as response to a query, you get [`EOSE`](https://github.com/nostr-protocol/nips/blob/master/01.md) and the connection is closed. The client needs to open a new connection and make a new query later in the future if it wants to get new events.

## Status

While **Servus** has quite a few features that may look like "advanced" and I use it personally to serve two web sites, it is also still very much experimental and definitely not for everyone - especially not for beginners!

In order to use it, you need at least some basic understanding of:

* the Linux command line
* `cargo`
* SSL certificates
* DNS

You also need a VPS with SSH access where you would run **Servus** unless you are just curious and want to test it locally, which is doable, although a bit tricky due to the SSL certificates.

**Also keep in mind that everything changes all the time without prior notice!** So using it for a production website is very risky. For now...

### UI

It is worth mentioning, before you go any further with false expectations, that **Servus** has a very basic admin interface which is not only lacking features but also very buggy. Don't rely on it... yet!

### Beginners

Does the above sound complicated to you?

**You might want to stop here, bookmark this repo, and check back in a year.**

Things are definitely going to improve, but I am too busy building a solid foundation in order to consider beginners. Sorry.

## Themes

Not only there is no stable UI, but there are no usable themes included.

A separate repository named `themes` exists, but it is very much WIP.

## Building

* `cargo build` - this builds the "debug" version
* `cargo build --release` - this builds the "release" version
* `docker run --rm -it -v "$PWD":/home/rust/src messense/rust-musl-cross:x86_64-musl cargo build --release` - this is an alternative way to build with musl

## Usage

* `./target/debug/servus` - this starts **Servus** on port 4884, without SSL
* `sudo ./target/debug/servus --ssl-acme[-production] --contact-email <contact_email>` - this starts **Servus** on port 443 and obtains SSL certificates from Let's Encrypt using ACME by providing `<contact_email>`
* `sudo ./target/debug/servus --ssl-cert <SSL_CERT_FILE> --ssl-key <SSL_KEY>` - this starts **Servus** on port 443 using the provided `<SSL_CERT>` and `<SSL_KEY>`

Note the `sudo` required to bind to port 443! Other ports can be used by passing `-p`, whether in SSL mode or not!

NB: in order to obtain Let's Encrypt certificates you must be running Servus on a machine that is accessible via a public IP (such as a VPS) and have the domain name mapped to that machine's IP. Running the `--ssl-acme` version on your developement machine won't work because Let's Encrypt will try to actually connect to your domain and validate your setup.

You can try running the SSL version locally using a custom certificate by passing `--ssl-cert` and `--ssl-key` like in the example above. Certificates can be obtained using [acme.sh](https://github.com/acmesh-official/acme.sh), but make sure you run `acme.sh --to-pkcs8` to convert the key to PKCS8 before you pass it to `Servus`.

PS: you can map `127.0.0.1` to your domain name from `/etc/hosts` to get a realistic simulation of the live environment on your local machine!

## Directory structure

You can run the **Servus** executable from any directory. On start, it looks for a directory named `sites` in the same directory as the executable, then loads all available "sites" that it finds in that directory. A "site" is identified by the domain name, which is passed by the browser using the [`Host` header](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/host). So a valid `sites` directory would look like this:

```
.
└── sites
    ├── domain1.com
    ├── domain2.com
    └── domain3.com
```

Each of these "sites" has the following structure:

```
├── _config.toml
├── _data
│   └── some_data.yml
├── _layouts
│   ├── includes
│   │   └── [...]
│   ├── base.html
│   ├── page.html
│   └── post.html
├── _events
│   ├── pages
│   └── posts
├── favicon.ico
```

Files and directories starting with "." are ignored.

Files and directories starting with "_" have special meaning: `_config.toml`, `_data`, `_layouts`, `_events`.

Anything else will be directly served to the clients requesting it.

## _config.toml

Every site needs a config file which has one section named `site`.

All properties present under `[site]` are passed directly to the templates: `title` becomes `site.title`, `url` becomes `site.url`, etc.

`post_permalink`, if specified, is used to generate permalinks for posts by replacing `:slug` with the actual *slug* of the post. If not specified, it defaults to `/posts/:slug`.

`pubkey`, if specified, is used to enable posting using the Nostr protocol. Only events from the specified pubkey will be accepted, after validating the signature.

## Templating

Templating is handled by `Tera`, which looks familiar to anyone who has used Liquid or Jinja2. See Tera's [documentation](https://tera.netlify.app/docs/) for more details.

## Template variables

The following variables are passed to the templates:

* `servus.version` - the version of Servus currently running
* `site` - the `[site]` section in `_config.toml`
* `page` - the current page being rendered
* `data` - any data loaded from YAML files in `_data/`
* `posts` - a list of all the posts (NB: this is available only for pages, not for the posts themselves)
* `pages` - a list of all the pages (NB: this is available only for the "extra resources", ie. resources that are neither pages nor posts, like sitemap.xml, for example)

### Page variables

* `page.url` - the URL of this page
* `page.date` - the date associated with this post

## Posting

Ways you can post to your blog:

1. **Post using a 3rd party Nostr NIP-23 client** such as [Habla](https://github.com/verbiricha/habla.news)
2. **Post using the built-in admin interface**, which is essentially a Nostr NIP-23 client

## REST API

A simple REST API exists that can be used to create new sites and list sites associated with a Nostr pubkey.

In order to activate the API, you need to pass `--admin-domain <ADMIN_DOMAIN>`. Servus will listen to that domain for API requests.

### `/api/sites`

A `POST` to `https://<ADMIN_DOMAIN>/api/sites` can be used to add a new site associated with a key.

A `GET` to `https://<ADMIN_DOMAIN>/api/sites` can be used to get a list of all the sites belonging to a key.

NB: Both requests require a [NIP-98](https://github.com/nostr-protocol/nips/blob/master/98.md) authorization header to be present, which will be validated and used to decide which Nostr pubkey the request is referring to!

## Admin interface

The same `--admin-domain <ADMIN_DOMAIN>` flag used to activate the REST API is also used to activate... you guessed it... the *admin interface*!

The *admin interface* requires you to have a Nostr extension such as [Alby](https://getalby.com/) or [nos2x](https://github.com/fiatjaf/nos2x) installed in your browser and lets you create sites, create posts and edit posts. Still very experimental, even more so than **Servus** itself!

## Any questions?

If you read this far without giving up and still want to try it yourself, feel free to open GitHub issues with any problems you encounter and I'll try to help. I currently use *Servus* to run two live sites, but it is probably not for everyone, yet...
