# servus

## About

**servus** is a simple CMS / blogging engine that is fully self-contained within one executable file.

Unlike WordPress, it does not require a database nor a web server with the capability of executing server-side scripts such as PHP.

While that might sound like SSG, it is not.

Unlike static site generators such as Jekyll, it does not have a build step nor does it require a web server to actually serve the files.

However, the design is heavily influenced by Jekyll in that the posts are stored in Markdown files with YAML front matter. The main difference is that rendered files are stored in memory and served directly by **servus**.

Posting can be done using the [Nostr protocol](https://github.com/nostr-protocol/nostr)'s [Long-form Content](https://github.com/nostr-protocol/nips/blob/master/23.md) event kind, so any Nostr client compatible with NIP-23 can be used for posting.

Posts can be retrieved using RSS/Atom in a similar way one would accomplish that with Jekyll (by building a template that iterates over the available posts and generates RSS/Atom) or by using the Nostr protocol to subscribe to events of kind `30023`. In the latter case, only posts that came via Nostr will be returned, because they need to be cryptographically signed by the client when posted.

## UI

It is worth mentioning, before you go any further with false expectations, that **servus** has no UI at all. It is very much like Jekyll in this regard. If you are familiar with tools like Jekyll you will feel at home. Otherwise you might want to stop here, bookmark this repo, and check back in a year.

## Themes

Not only there is no UI, but there are no usable themes included. On first run, a theme named `blank` is installed, which is, like the name says, *blank*. No CSS or anything. It is just a directory structure with some plain HTML files that you can use to test your instalation.

However, porting themes over from Jekyll should be pretty straight forward. There are a few changes required to match the directory structure required by *servus*, after which you will start getting errors from the templating engine, which you can solve pretty easily.

## Usage

* `cargo build` - this builds the "debug" version
* `./target/debug/servus dev` - this starts **servus** on port 4884

* `cargo build --release` - this builds the "release" version
* `sudo ./target/release/servus live` - this starts **servus** on port 443 (note the `sudo` required to bind to that port!) and obtains SSL certificates from Let's Encrypt

NB: in order to obtain Let's Encrypt certificates you must be running Servus on a machine that is accessible via a public IP (such as a VPS) and have the domain name mapped to that machine's IP. Running the "live" version on your developement machine won't work because Let's Encrypt will try to actually connect to your domain and validate your setup.

However, there is a way to run the "live" version locally *if* you have already run it on your VPS and obtained the certificates. You can copy over the `cache` subdirectory, which includes the certificates, and you can even map `127.0.0.1` to your domain name from `/etc/hosts` to get a realistic simulation of the live environment on your local machine.

## Directory structure

You can run the **servus** executable from any directory. On start, it looks for a directory named `sites` in the same directory as the executable, then loads all available "sites" that it finds in that directory. A "site" is identified by the domain name, which is passed by the browser using the [`Host` header](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/host) (minus the port). So a valid `sites` directory would look like this:

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
├── _posts
│   ├── 2022-12-30-servus.md
│   └── ...
├── atom.xml
├── favicon.ico
├── index.md
├── robots.txt
└── sitemap.xml
```

Files and directories starting with "." are ignored.

Files and directories starting with "_" have special meaning: `_config.toml`, `_data`, `_layouts`, `_posts`.

Anything else is considered a "resource" that will be served to the clients requesting it - regardless of the name - whether it is a binary file (like `favicon.ico`) or a text file (like `atom.xml`).

Files ending in `.md` are rendered to HTML first, injected into a *layout* and served as HTML.

Files ending in `.xml` or `.txt` are preprocessed using the template engine, so you can customize your `atom.xml` for example.

Other files are considered "raw" and sent to the clients as they are.

## _config.toml

Every site needs a config file which has one section named `site`.

The only required property is `contact_email`, which is used for requesting certificates from Let's Encrypt.

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
* ...

Any custom front matter that you specify will be available under `page`. You can, for example, pass `lang: en` in your page's front matter and this value will be available under `page.lang`.

## Posting

You can post to your blog in two ways:

1. **Post by adding files to the *_posts* directory** - this is basically what you would do with Jekyll.
2. **Post using a Nostr NIP-23 client**

## Nostr

The Nostr protocol can be used for posting and retrieving posts, so you can use a Nostr client with NIP-23 support, like [Habla](https://habla.news), to post to your blog. Posts that come from Nostr have an extra `.json` file under the `_posts` directory that includes the raw Nostr event used to create (or edit) that post, which also includes the client's signature. This JSON will be returned to the clients asking for posts using Nostr's `REQ` command, so the signature will match the pubkey when the client validates it.

You can also use **servus** without Nostr in a similar way you would use Jekyll, by just editing the `.md` files under `_posts` manually, but since these files would not include a signature, they won't be returned to Nostr clients because the verification would fail would fail anyway.

## Any questions?

If you read this far without giving up and still want to try it yourself, feel free to open GitHub issues with any problems you encounter and I'll try to help. I currently use *servus* to run two live sites, but it is probably not for everyone, yet...
