# AHEAD extension catalog

AHEAD queries Zed's live extension catalog and versioned package-download API
in both development and release builds. Opening or refreshing the gallery
fetches current results; search uses the catalog's filter query. Zed operates
the catalog and package service. AHEAD is independent and is not endorsed by
Zed. Entries are compatibility candidates: matching an advertised extension
API version does not guarantee that the extension or its language server works
in AHEAD.

`registry.json` is an optional AHEAD-reviewed additions feed. Debug builds read
the checkout's local copy; release builds read this repository's `main`
branch. The checked-in feed is currently empty. When a live Zed result and an
AHEAD entry share an ID, the Zed result takes precedence. AHEAD entries
supplement missing Zed results and remain available when Zed's catalog cannot
be fetched.

Each published `data` entry uses the Zed manifest metadata AHEAD understands
and adds:

- `download_url`: an HTTPS GitHub Release asset in this repository, named
  `<id>-<version>.tar.gz`.
- `archive_sha256`: the lowercase SHA-256 of that exact asset.

The application rejects AHEAD entries without both fields, with a different
host/repository/path, or with an unsupported extension manifest/API.

The AHEAD additions feed stays empty until the reviewed publisher is run for
an approved source pin. `sources.json` pins the checked-out source
repository and revision, and each extension records its own source repository,
revision, path, extension-code license, runtime dependencies and requested host
capabilities. A source path may be a pinned Git submodule in Zed's public
extensions catalog, so the catalog commit and extension commit are verified
separately. The publisher rejects a mismatched remote, revision or dirty source
checkout, builds with the pinned Zed extension CLI, verifies package metadata
and license, publishes a versioned GitHub Release asset and digest, and updates
the AHEAD feed. The
`extensions-publish.yml` workflow is manual-only; it has not been run and no
package has been published yet. Do not clone and execute extension source
while users browse or install the catalog. Direct API use is an AHEAD product
choice, not a statement that Zed has approved third-party clients; service
availability and applicable terms may change.

The first publication, runtime-server review and native install acceptance
remain tracked in [`TODO.md`](../TODO.md); an empty feed is not evidence of
extension support.
