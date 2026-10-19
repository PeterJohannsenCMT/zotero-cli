# zotero-cli

A small command-line client for your [Zotero](https://www.zotero.org) library,
backed by the [Zotero Web API](https://www.zotero.org/support/dev/web_api/v3/start).

## Install

Via Homebrew (macOS/Linux):

```sh
brew install peterjohannsencmt/tap/zotero-cli
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
zotero list                          # list all top-level items (or current subcollection)
zotero list --subcollection=Optics   # list items in a subcollection
zotero list --tag=Smith2020          # filter by tag

zotero search majorana               # print matching keys, one per line
zotero search "spin qubit"           # multi-word: matches items containing all words
zotero search --author claeys        # only match against author/creator names
zotero search --title qubit          # only match against the title
zotero search --key richardson       # only match against the citation key
zotero search --tags optics          # only match against Zotero tags
zotero search --keywords topological # only match against local keywords (see below)
zotero search --author claeys --key 2017   # combine flags (AND'ed together)
zotero search majorana | xargs -I{} zotero open {}   # pipe into open/edit/etc.

zotero collections list              # list all subcollections as an indented tree
zotero collections search optics     # print subcollection names matching a query
zotero collections use Optics        # make list/search/export default to Optics
zotero collections current           # show the current default subcollection
zotero collections clear             # return to the library top level by default
zotero collections edit Optics "Optics & Photonics"   # rename a subcollection

zotero add 10.1038/nphys1170         # look up a DOI and attach a PDF if CrossRef exposes one
zotero add --nopdf 10.1038/nphys1170 # add metadata only; don't look for a PDF

zotero export Smith2020 Doe2021             # print LaTeX \bibitem entries
zotero export --bib --subcollection=Optics > optics.bib   # export a whole collection as BibTeX
zotero search majorana | zotero export --bib > refs.bib   # export search results as BibTeX

zotero open Smith2020                # open the PDF for the item tagged Smith2020
                                      # (falls back to the item's URL/DOI if no PDF)

zotero edit Smith2020                # edit metadata in $EDITOR, saved back on exit
zotero fetch-pdf Smith2020           # look up and attach a PDF for an existing item
zotero remove Smith2020              # remove an item from your Zotero library

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
when run interactively. When piped into another `zotero` command, they print
just the bare keys (one per line, no header) so they compose directly with
`xargs`, `zotero open`, `zotero edit`, etc. without needing extra flags. When
piped into other commands, they print one aligned reference per line.

## AuthorYear tags

`zotero add` automatically tags new items with an `AuthorYear` tag derived from
the first author's last name and the publication year (e.g. `Smith2020`), matching
the citation-key style used by BibTeX/Better BibTeX. If that tag is already used
by another item in your library, a letter suffix is appended (`Smith2020a`,
`Smith2020b`, ...) the same way BibTeX disambiguates duplicate keys.

Every command (`list`, `open`, `edit`) also **backfills** this tag onto any
existing item in your library that doesn't have one yet, the first time it
sees that item — so items you added directly in Zotero get one too. This is
written back to Zotero as a real tag (visible in the Zotero app itself).
To rename a citation-key tag, edit the tag in Zotero; the CLI editor preserves
existing tags without displaying them.

`zotero list`/`open`/`edit` look for a tag shaped like a name followed by a
4-digit year (with an optional letter suffix) and show/match on it in
preference to the raw item key.

`zotero export --bib` uses Zotero's own BibTeX rendering (so field formatting and
escaping match a normal Zotero export) but always rewrites the citekey to
the item's AuthorYear tag, so a `.bib` file pulled with `zotero export --bib`
cites the same way `zotero open`/`edit` address the item. Items without an
AuthorYear tag (no derivable author/year) fall back to their raw Zotero key,
with a warning on stderr.

For theses, regular BibTeX export also preserves Zotero's degree type (`type`),
university (`school`), location, date, and DOI/URL/arXiv fields when supplied by
Zotero's BibTeX renderer. The original BibTeX entry type is preserved, including
`@phdthesis` and `@mastersthesis`.

In `--bib` output, recognized journal names are written as REVTeX/apsrev macros or
abbreviations, e.g. `journal = prb,` instead of
`journal = {Physical Review B},`. Pass `--no-revtex-journals` to preserve
Zotero's original journal strings for a portable non-REVTeX export.

By default, `zotero export` emits local LaTeX `\bibitem` entries in APS style. Journal
articles are rendered as `J. Doe, Title, Journal No, Pages (Year)`, with the
journal/volume/pages/year block hyperlinked through the DOI (or URL) when one is
available.

Thesis entries include the title, thesis type, university, and year, followed by
an arXiv link when present. Store dissertations as Zotero `thesis` items and fill
in `thesisType` and `university`; an arXiv identifier does not override thesis
metadata. If the degree is missing, export uses `Thesis` rather than assuming a
doctorate.

Both export formats also check arXiv comments for preprints, manuscripts, and
reports with arXiv identifiers. Explicit doctoral or master's thesis metadata
is used for the export, including a university when clearly stated. This does
not change the Zotero record. If the lookup fails, export warns and uses the
stored metadata; ordinary preprints retain their existing formatting.

## Neovim integration

The Neovim plugin is a thin editor wrapper around the `zotero` executable.
It does not embed Zotero API access itself. For the plugin to work, Neovim
must be able to run `zotero` from `$PATH`.

Install the CLI first:

```sh
brew install peterjohannsencmt/tap/zotero-cli
zotero config
```

Then add [`PeterJohannsenCMT/zotero-cli.nvim`](https://github.com/PeterJohannsenCMT/zotero-cli.nvim)
to your Neovim plugin specs:

```lua
{
  "PeterJohannsenCMT/zotero-cli.nvim",
  dependencies = {
    "ibhagwan/fzf-lua", -- optional but recommended
  },
  opts = {},
  cmd = {
    "ZoteroSearch",
    "ZoteroOpen",
    "ZoteroCite",
    "ZoteroExportBib",
    "ZoteroExportBibitem",
    "ZoteroAdd",
    "ZoteroAddManual",
  },
  keys = {
    { "<leader>zs", "<cmd>ZoteroSearch<cr>", desc = "Zotero search" },
    { "<leader>zo", "<cmd>ZoteroOpen<cr>", desc = "Zotero open" },
    { "<leader>zc", "<cmd>ZoteroCite<cr>", desc = "Zotero cite" },
    { "<leader>zb", "<cmd>ZoteroExportBib<cr>", desc = "Zotero export BibTeX" },
    { "<leader>zi", "<cmd>ZoteroExportBibitem<cr>", desc = "Zotero export bibitem" },
    { "<leader>za", "<cmd>ZoteroAdd<cr>", desc = "Zotero add DOI" },
    { "<leader>zA", "<cmd>ZoteroAddManual<cr>", desc = "Zotero add manual" },
  },
}
```

Typical commands:

```vim
:ZoteroSearch
:ZoteroSearch smith
:ZoteroOpen
:ZoteroOpen Smith2020
:ZoteroCite
:ZoteroExportBib Smith2020 Doe2021
:ZoteroExportBibitem Smith2020
:ZoteroAdd 10.1038/nphys1170
:ZoteroAddManual book
```

`ZoteroSearch` opens an fzf picker immediately, shows Zotero's
TAG/YEAR/AUTHOR/TITLE table for context, and inserts only the selected key at
the cursor. This is meant for editing inside an existing citation, e.g.
`\cite{_}` with the cursor at `_`.

`ZoteroOpen`, `ZoteroCite`, `ZoteroExportBib`, and `ZoteroExportBibitem` use
explicit command arguments when provided. Without arguments, they use the
`<cword>` under the cursor as the Zotero key, falling back to the picker when
there is no word under the cursor.

## Local keywords

Existing local keywords remain in `keywords.json`, next to `config.toml`,
and can be searched with `zotero search --keywords <term>`. They are not synced
to Zotero. The citation editor hides and preserves these keywords.

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

## Citation-key checks

`zotero check-citation-keys` reports citation keys whose collision suffixes can
be compacted. For example, if `surname2018a` exists but `surname2018` does not,
it prints `surname2018a -> surname2018`. The command only reports suggested
renames; it does not modify Zotero items.

## Offline use

Read commands automatically fall back to the last cached library if refreshing
fails, with a warning that results may be stale. This includes `list`, `search`,
`show`, collection browsing, citation-key checks, export, and opening local PDFs.

Use `--offline` to avoid all online services explicitly:

```sh
zotero --offline search majorana
zotero --offline show Smith2020
zotero --offline open Smith2020
zotero --offline export --bib Smith2020
```

Offline reads can use Zotero Desktop's local API (if enabled) or the CLI's saved
cache, even when Desktop is closed. Run `zotero sync` online once to populate the
cache. The CLI does not read Zotero's SQLite database directly; if neither the
cache nor the local API is available, it explains how to make data available.
Only metadata and PDFs already on disk are available offline. Missing PDFs and
write commands require connectivity; changes are not queued for later upload.
BibTeX is rendered from stored metadata when Zotero's renderer is unavailable
(and may differ from Zotero's rendering). Optional arXiv enrichment is skipped
in offline mode. Explicit `sync` reports refresh errors instead of silently
using stale data.

## Local API and cache

For reads, `zotero` first tries Zotero Desktop's local API at
`http://localhost:23119/api/`. Enable it in Zotero's settings under Advanced:
"Allow other applications on this computer to communicate with Zotero".
The local API is read-only, offline-capable, faster, and has no Web API rate
limits. If it isn't available, the CLI falls back to the Zotero Web API.

To avoid re-fetching your whole library on every command, `zotero` keeps a local
cache at `~/Library/Caches/zotero-cli/cache.json` (or the equivalent
`dirs::cache_dir()` location on other platforms) and uses Zotero's
`since=<version>` parameter to pull only what's changed since the last run.
The first run after `zotero config` (or after deleting the cache file) does
a full sync and will be slower, especially if it needs to backfill
AuthorYear tags onto a lot of existing items; every run after that is
incremental and fast. Delete the cache file any time to force a full resync.

## Notes

- Writes (`add`, `edit`, `collections edit`, and the AuthorYear backfill) go
  through the Zotero Web API rather than touching Zotero's local SQLite
  database directly, since direct external writes to that database are
  unsupported and can cause sync corruption. Zotero's local API currently
  supports reads only, so writes and file uploads still use the Web API.
- `zotero open` looks for a PDF child attachment on the item. For attachments
  Zotero has synced to disk, it resolves the file path under your local
  Zotero data directory (`storage/<key>/<filename>`) and opens it with your
  OS's default handler. If there's no PDF, it opens the item's URL or DOI
  instead.

## License

[MIT](LICENSE)

`zotero edit KEY` shows the fields used by BibTeX or `\bibitem` export for
that item's type, plus `abstract`. The editor uses readable names: `journal`,
`journal_abbreviation`, `thesis_type`, `university`, `book_title`, and
`report_number`. `date` supplies the export year. Cataloguing fields, tags,
local keywords, and non-author creator roles are hidden and preserved on save.

Journal articles use this form (prefilled for `edit`, blank for manual add):

```toml
title = "Example article"
journal = "Physical Review B"
journal_abbreviation = "Phys. Rev. B"
date = "2026"
volume = "114"
pages = "012345"
doi = "10.1103/example"
url = ""
abstract = "Article summary."

[authors]
names = [
    { first = "Ada", last = "Lovelace" },
    { first = "Ludwig", last = "van Beethoven" },
    { name = "Research Collaboration" },
]
```

Create an item with `zotero add manual` or `zotero add --manual` (defaults
to `journalArticle`). Use `--type thesis` for a thesis or `--type book` for a
book. The thesis form is:

```toml
title = ""
thesis_type = ""       # e.g. "Ph.D. thesis"
university = ""
date = ""
place = ""
doi = ""
url = ""
arxiv_id = ""
abstract = ""

[authors]
names = [
    { first = "", last = "" },
]
```

Enter authors in publication order. Use `name` for institutional authors.
`arxiv_id` maps to Zotero's existing archive metadata or a labelled entry in
Extra, preserving unrelated metadata. DOI also uses Extra when the Zotero
item template has no dedicated DOI field. Other fields are shown only when
supported by the selected item type. Save and exit to create the item; an
empty title aborts creation.
