# servus

## About

**servus** is a simple CMS / blogging engine that is fully self-contained within one executable file.

Unlike WordPress, it does not require a database nor a web server with the capability of executing server-side scripts such as PHP.

While that might sound like SSG, it is not.

Unlike static site generators such as Jekyll, it does not have a build step nor does it require a web server to actually serve the files.

However, the design is heavily influenced by Jekyll in that the posts are stored in Markdown files with YAML front matter. The main difference is that rendered files are stored in memory and served directly by **servus**.

Posting can be done using the [Nostr protocol](https://github.com/nostr-protocol/nostr)'s [Long-form Content](https://github.com/nostr-protocol/nips/blob/master/23.md) event kind, so any Nostr client compatible with NIP-23 can be used for posting.

Posts can be retrieved using RSS/Atom in a similar way one would accomplish that with Jekyll (by building a template that iterates over the available posts and generates RSS/Atom) or by using the Nostr protocol to subscribe to events of kind `30023`. In the latter case, only posts that came via Nostr will be returned, because they need to be cryptographically signed by the client when posted.

## Usage

* `cargo build` - this builds the "debug" version
* `./target/debug/servus dev` - this starts **servus** on port 4884

* `cargo build --release` - this builds the "release" version
* `sudo ./target/release/servus live` - this starts **servus** on port 443 (note the `sudo` required to bind to that port!) and obtains SSL certificates from Let's Encrypt

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

## Nostr

The Nostr protocol is used for posting and retrieving posts, so you can use a Nostr client with NIP-23 support, like [Habla](https://habla.news), to post to your blog. Posts that come from Nostr have an extra `.json` file under the `_posts` directory that includes the raw Nostr event used to create (or edit) that post, which also includes the client's signature. This JSON will be returned to the clients asking for posts using Nostr's `REQ` command, so the signature will match the pubkey when the client validates it.

You can also use **servus** without Nostr in a similar way you would use Jekyll, by just editing the `.md` files under `_posts` manually, but since these files would not include a signature, they won't be returned to Nostr clients because the verification would fail would fail anyway.
