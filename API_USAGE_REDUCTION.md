# API Usage Reduction Plan

This document records a future plan for reducing calls to the online Zotero Web API (`https://api.zotero.org`) if API usage, latency, or rate limits become a problem.

## Current behavior summary

- Reads prefer the Zotero Desktop local API (`http://localhost:23119/api`) through `ZoteroClient::read_preferred`.
- Most metadata writes now also prefer the local API through `ZoteroClient::write_preferred`, falling back to the Zotero Web API when local is unavailable or rejects the operation.
- PDF attachment uploads still use the Zotero Web API storage authorization/register flow.
- Most commands call `sync::sync_library`, which skips all network access when the cache is fresh.
- The cache freshness window is currently hard-coded to 60 seconds in `src/sync.rs`.
- Normal BibTeX export calls Zotero's BibTeX renderer once per exported item via `/items/{key}?format=bibtex`; `--bibitem` formats locally from cached data.

## Goals

1. Avoid unexpected Web API writes from read-only commands.
2. Reduce repeated sync traffic during interactive use.
3. Avoid per-item export API calls where practical.
4. Preserve correctness for explicit write commands.
5. Keep `zotero sync` as the user's explicit way to force freshness.

## Implemented local-first changes

- Added `ZoteroClient::write_preferred`, mirroring read fallback behavior for writes.
- Routed normal item/collection metadata writes through local-first clients:
  - add item
  - manual add
  - edit item
  - remove item
  - collection rename
  - collection membership changes
  - sync-time citation-key maintenance writes
- Kept `attach-pdf` on the Web client because PDF file uploads currently depend on Zotero's Web API storage upload flow.

## Recommended implementation phases

### Phase 1: Separate read sync from citation-key maintenance

Currently `sync_library` can perform tag-maintenance writes after pulling changes:

- dedupe citation keys
- normalize citation keys
- backfill missing citation keys

Plan:

- Split sync into two paths:
  - `sync::pull_library(read_client, cache)` — read-only; updates local cache only.
  - `sync::maintain_citation_keys(write_client, cache)` — performs tag fixes/backfills.
- Make read-only commands call only the read-only path.
- Keep tag maintenance for explicit commands such as:
  - `zotero sync --fix-tags` or `zotero maintenance`
  - `zotero add`
  - maybe explicit `zotero sync` if we decide that command should still repair tags.

Expected benefit: read-only commands no longer unexpectedly perform metadata PATCH requests. After the local-first write change, these PATCHes should hit the Web API only when the local API is unavailable or rejects them.

### Phase 2: Make cache freshness configurable

Current freshness window: 60 seconds.

Plan:

- Add config field, for example:

  ```toml
  cache_freshness_secs = 300
  ```

- Default to a larger value such as 300 seconds.
- Preserve `zotero sync` as a freshness-window bypass.
- Optionally add a command-line flag for read commands:

  ```bash
  zotero list --refresh
  zotero search --refresh QUERY
  ```

Expected benefit: fewer repeated incremental sync reads during normal shell workflows.

### Phase 3: Cache or locally generate BibTeX exports

Normal `zotero export` currently calls Zotero once per item to render BibTeX.

Options:

1. Cache rendered BibTeX by `(item_key, item_version)`.
   - Store rendered entries in the local cache or a sidecar cache file.
   - Reuse cached BibTeX when the item version has not changed.

2. Generate BibTeX locally from cached item data.
   - Similar to the existing local `--bibitem` path.
   - Requires careful Zotero-to-BibTeX field mapping.

3. Investigate whether Zotero supports batch BibTeX export for multiple selected item keys.
   - If supported, replace N per-item calls with one batched call.

Recommended first step: cache rendered BibTeX by item key and version. It is lower risk than fully local BibTeX generation.

Expected benefit: large reduction in API calls for repeated exports.

### Phase 4: Batch piped/multi-item command execution

`run_for_each` currently invokes the command function once per identifier. Commands like `edit`, `open`, and similar therefore reload config/cache and may sync repeatedly when given piped input.

Plan:

- Add batch-oriented command entry points where useful.
- Load config/cache and sync once.
- Resolve all identifiers against one cache snapshot.
- Process each target afterward.

Expected benefit: fewer repeated sync attempts and less repeated setup for piped workflows.

### Phase 5: Cache item templates

Commands such as `zotero add --manual`, DOI-based `zotero add`, and `zotero attach-pdf` fetch item templates from Zotero.

Plan:

- Cache templates by `(item_type, link_mode)`.
- Invalidate rarely, or provide a manual refresh path.
- Prefer local API for template fetches where available.

Expected benefit: fewer template reads; modest but easy improvement.

### Phase 6: Add optional local/cache-only mode

Some workflows may prefer avoiding the Web API entirely for read commands.

Plan:

- Add config option such as:

  ```toml
  web_read_fallback = true
  ```

- When disabled, read commands should either:
  - use fresh cache only, or
  - use local API only and fail if unavailable.

Potential CLI flags:

```bash
zotero list --offline
zotero search --offline QUERY
```

Expected benefit: user-controlled guarantee that read-only commands do not hit the online API.

## Suggested priority if this becomes urgent

1. Split read-only sync from citation-key maintenance.
2. Cache BibTeX export output by item key/version.
3. Make cache freshness configurable and increase the default.
4. Batch piped/multi-item command execution.
5. Cache item templates.
6. Add optional local/cache-only read mode.

## Notes and cautions

- Writes still need the Web API; the local Zotero API is treated as read-only here.
- Increasing the freshness window means changes made in Zotero Desktop or elsewhere may take longer to appear automatically.
- BibTeX caching must include item version in the cache key to avoid stale exports.
- If citation-key maintenance is moved out of normal sync, old libraries may need an explicit migration/maintenance command.
