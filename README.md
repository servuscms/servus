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
