# zotero-cli

A small command-line client for your [Zotero](https://www.zotero.org) library,
backed by the [Zotero Web API](https://www.zotero.org/support/dev/web_api/v3/start).

## Install

Via Homebrew (macOS/Linux):

```sh
brew install PeterJohannsenCMT/tap/zotero-cli
```

Or build from source:

```sh
cargo install --path .
# or: cargo build --release && ./target/release/zotero --version
```

## Setup

1. Get an API key at https://www.zotero.org/settings/keys (needs read/write access
   to the library you want to manage). Your numeric user ID is shown on the same page.
2. Configure it:

   ```sh
   zotero config
   ```

   You'll be asked for:
   - library type (`user` or `group`)
   - library ID (your numeric user ID, or a group ID)
   - API key
   - local Zotero data directory (leave blank to auto-detect — used only for
     `zotero open`, to find PDF files that have synced to disk)

   This is saved to `~/Library/Application Support/zotero-cli/config.toml` (macOS)
   or the equivalent `dirs::config_dir()` location on other platforms.

## Commands

```sh
zotero list                          # list all top-level items
zotero list --subcollection=Optics   # list items in a subcollection
zotero list --tag=Smith2020          # filter by tag

zotero search majorana               # print matching keys, one per line
zotero search "spin qubit"           # multi-word: matches items containing all words
zotero search majorana | xargs -I{} zotero open {}   # pipe into open/edit/etc.

zotero add 10.1038/nphys1170         # look up a DOI on CrossRef and add it

zotero open Smith2020                # open the PDF for the item tagged Smith2020
                                      # (falls back to the item's URL/DOI if no PDF)

zotero edit Smith2020                # edit metadata in $EDITOR, saved back on exit

zotero search majorana | zotero open   # open every matching PDF
zotero search majorana | zotero edit   # walk through editing every match
```

Items can be referenced by their `AuthorYear` tag (see below) or by their raw
8-character Zotero item key.

`zotero open` and `zotero edit` also accept identifiers piped in on stdin
(one per line) instead of a command-line argument — so you can pipe
`zotero search`/`zotero list` straight into them. Each identifier is
processed in turn; a failure on one doesn't stop the rest.

`zotero list` and `zotero search` print the full TAG/YEAR/AUTHOR/TITLE table
when run interactively, but switch to printing just the bare keys (one per
line, no header) when their output isn't a terminal — e.g. when piped into
another command — so they compose directly with `xargs`, `zotero open`,
`zotero edit`, etc. without needing extra flags.

## AuthorYear tags

`zotero add` automatically tags new items with an `AuthorYear` tag derived from
the first author's last name and the publication year (e.g. `Smith2020`), matching
the citation-key style used by BibTeX/Better BibTeX. If that tag is already used
by another item in your library, a letter suffix is appended (`Smith2020a`,
`Smith2020b`, ...) the same way BibTeX disambiguates duplicate keys.

Every command (`list`, `open`, `edit`) also **backfills** this tag onto any
existing item in your library that doesn't have one yet, the first time it
sees that item — so items you added directly in Zotero get one too. This is
written back to Zotero as a real tag (visible in the Zotero app itself), not
just computed on the fly, which is what makes it possible to rename via
`zotero edit`: the citation-key tag is just an entry in the item's `tags`
list, so editing it there (or deleting/replacing it) changes what `zotero`
matches the item by from then on.

`zotero list`/`open`/`edit` look for a tag shaped like a name followed by a
4-digit year (with an optional letter suffix) and show/match on it in
preference to the raw item key.

## Shell completion (zsh)

`zotero completions zsh` prints a completion script that gives `zotero open`
and `zotero edit` tab-completion of your actual paper keys (it shells out to
`zotero list` at completion time, which is instant once the cache is warm —
see below).

```sh
mkdir -p ~/.zsh/completions
zotero completions zsh > ~/.zsh/completions/_zotero
```

Then, in `~/.zshrc`, make sure that directory is in `fpath` *before*
`compinit` runs:

```sh
fpath=(~/.zsh/completions $fpath)
autoload -Uz compinit
compinit
```

Open a new shell (or `rm -f ~/.zcompdump*; compinit` in your current one, since
`compinit` caches what it found) and `zotero open <TAB>` / `zotero edit <TAB>`
will suggest matching paper keys.

## Local cache

To avoid re-fetching your whole library on every command, `zotero` keeps a
local cache at `~/Library/Caches/zotero-cli/cache.json` (or the equivalent
`dirs::cache_dir()` location on other platforms) and uses the Zotero API's
`since=<version>` parameter to pull only what's changed since the last run.
The first run after `zotero config` (or after deleting the cache file) does
a full sync and will be slower, especially if it needs to backfill
AuthorYear tags onto a lot of existing items; every run after that is
incremental and fast. Delete the cache file any time to force a full resync.

## Notes

- Writes (`add`, `edit`, and the AuthorYear backfill) go through the Zotero
  Web API rather than touching Zotero's local SQLite database directly,
  since direct external writes to that database are unsupported and can
  cause sync corruption.
- `zotero open` looks for a PDF child attachment on the item. For attachments
  Zotero has synced to disk, it resolves the file path under your local
  Zotero data directory (`storage/<key>/<filename>`) and opens it with your
  OS's default handler. If there's no PDF, it opens the item's URL or DOI
  instead.

## License

[MIT](LICENSE)
