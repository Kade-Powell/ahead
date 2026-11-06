## Building from source

It is easy to build Ahead from source on a GNU/Linux distribution. Cargo handles the build process, all you need to do, is ensure the correct dependencies are installed.

1. Install Rust 1.95 or newer with Cargo using [`rustup.rs`](https://rustup.rs/). If you already have the toolchain, ensure you are using a current Rust version.

2. Install dependencies for your operating system:

#### Ubuntu
```sh
sudo apt install clang libxkbcommon-x11-dev pkg-config libvulkan-dev libwayland-dev xorg-dev libxcb-shape0-dev libxcb-xfixes0-dev
```
The same list is installed in CI via `just ubuntu-deps`.
#### Fedora
```sh
sudo dnf install clang libxkbcommon-x11-devel libxcb-devel vulkan-loader-devel wayland-devel openssl-devel pkgconf
```
#### Void Linux
```sh
sudo xbps-install -S base-devel clang libxkbcommon-devel vulkan-loader wayland-devel
```

3. Clone this repository (this command will clone to your home directory):
```sh
git clone https://github.com/Kade-Powell/ahead.git ~/ahead
```

4. `cd` into the repository, and run the build command with the release flag
```sh
cd ~/ahead
```

```sh
cargo install --path . --bin ahead --profile release-lto --locked
```

> If you use a different distribution, and are having trouble finding appropriate dependencies, let us know in an issue!

Once Ahead is compiled, the executable will be available in `$HOME/.cargo/bin/ahead` and should be available in `PATH` automatically.

That direct `cargo install` installs only the editor executable. Models that
require Code Mode also need `ahead-code-mode-host` beside it; without the
sidecar, AHEAD leaves those tools unavailable instead of silently changing the
model's tool contract. On macOS, use `just dev` for the full development build
or `just app` for the bundle recipe. The bundle still needs native acceptance
testing before release.

On supported macOS and Linux hosts, install the sidecar into the same Cargo bin
directory after installing the editor:

```bash
source scripts/ensure-code-mode-v8.sh
cargo install --path ahead-code-mode-host --bin ahead-code-mode-host --profile release-lto --locked
```

The pinned sandboxed V8 artifacts currently cover macOS and Linux, not Windows;
Code Mode models remain unavailable on Windows until that host is supported.

For day-to-day development, `just dev` runs the editor, its re-executed proxy,
and the sibling Code Mode host beside the editor binary. On its first
supported macOS or Linux build,
`scripts/ensure-code-mode-v8.sh` downloads the pinned sandboxed V8 archive and
Rust binding from the OpenAI release, verifies their SHA-256 hashes, and caches
them outside the Cargo target directory: `~/Library/Caches/ahead/code-mode-v8/`
on macOS, or `$XDG_CACHE_HOME/ahead/code-mode-v8/` (defaulting to
`~/.cache/ahead/code-mode-v8/`) on Linux. Set `AHEAD_V8_CACHE_DIR` to override
that cache root. Subsequent builds reuse the verified cache, including after
`cargo clean`. `just dev` waits two seconds after a burst of file events before
restarting, reducing rebuild churn while agents or formatters touch several
files. It watches all first-party workspace crates, including the separate
agent graph store, skills, model-auth, runtime API and retained engine
packages, plus the repo-owned `third_party/gpui-pre` patch. Shell-script
changes are watched as well. On macOS, the watcher polls
once per second because native file events missed atomically replaced Rust
sources in the local development checkout; this adds up to one second of event
detection delay before the existing debounce. Its macOS dev bundle uses the
system's copy-on-write file copy to avoid duplicating the large debug
executable on APFS; the system falls back to a regular copy on filesystems
without cloning. On macOS, `just app` bundles it
as `target/release-lto/macos/Ahead.app`. A direct `cargo run -p ahead --bin
ahead` still launches the editor but does not build or place the sidecar.

`just dev` defaults the editor workspace to the repository (`.`). For UI
acceptance, keep the command's working directory at this repository root so it
continues watching AHEAD's source, and pass a disposable project as its
workspace argument:

```sh
test_workspace="$(mktemp -d "${TMPDIR:-/tmp}/ahead-editor-smoke.XXXXXX")"
cp -R tests/fixtures/editor-smoke/. "$test_workspace/"
just dev "$test_workspace"
```

This starts the normal shared build loop while opening the copied fixture in
the editor. AHEAD may ask whether to trust project-configured language servers
or extensions; allow them only for a disposable project you recognize. Avoid
opening the AHEAD source checkout for broad editor acceptance, since project
bootstrap writes private session state under that workspace's `.ahead/`.

`cargo clean` removes the Rust build cache but leaves the verified Code Mode V8
assets in the user cache, avoiding a second download. It still forces a cold
Rust rebuild, so use it to recover corrupted artifacts or reclaim disk, not as
a general build-speed fix. Keep the shared `target/` warm and let Cargo rebuild
only the changed packages.

After building both debug binaries, run the headless managed Code Mode smoke
without credentials or a real project:

```sh
node tests/agent-code-mode-smoke.mjs target/debug/ahead
```

It creates a disposable project and loopback model, checks the sibling host,
delegated AHEAD file search, model continuation, explicit cancellation and
retry after a proxy restart, Turso-restored chat and shutdown.
It does not replace a rendered `just dev` or authenticated-provider pass.
The disposable proxy smokes use an isolated `AHEAD_DATA_HOME`; managed-agent
smokes also isolate `AHEAD_HOME` and `AHEAD_USER_HOME`, so they do not use your
normal AHEAD profile.

To exercise an authenticated managed turn against the supplied OpenRouter
free model, set `OPENROUTER_API_KEY` in your environment and run:

```sh
rtk node tests/agent-openrouter-smoke.mjs target/debug/ahead
```

This is an opt-in network smoke. It uses a private temporary AHEAD profile and
trusted disposable workspace, puts the key only in a mode-0600 temporary user
settings file, removes it from the proxy process environment, and deletes the
profile after the run. The agent must stream a response and use Code Mode
`file_search` to return a random value that was not included in the prompt.
Only synthetic test text is sent; do not adapt this test to include source or
personal data. OpenRouter's free-model page warns that inputs may be logged for
service improvement. This smoke does not verify the rendered chat UI or
real-provider cancellation/resume.

To exercise the installed Pi ACP adapter with a live OpenRouter model, make
`OPENROUTER_API_KEY` available to the test process without putting it in the
command history, then run this focused ignored test:

```sh
AHEAD_PI_TEST_MODEL=openrouter/nvidia/nemotron-3-ultra-550b-a55b:free \
  rtk cargo test --locked -p ahead-agent --lib \
  adapters::tests::pi_registry_adapter_completes_an_acp_turn -- \
  --ignored --exact --test-threads=1
```

This smoke requires Node.js/npm and a separate Pi CLI (`pi`) v0.81.0 or newer
on `PATH`; the ACP registry package is only the `pi-acp` bridge, which starts
that CLI in RPC mode. The test installs the curated bridge into a disposable
cache, creates a temporary Pi home with only the selected model, and sends one
synthetic `PONG` prompt. The OpenRouter key stays in the process environment;
the test does not read or write the user's Pi profile. It verifies one
authenticated ACP turn, not Pi cancellation/resume or authenticated
Codex/Claude turns. See the [adapter's prerequisites](https://github.com/svkozak/pi-acp#prerequisites).

To install and launch all three curated ACP bridges without sending a prompt,
run the disposable registry smoke:

```sh
node tests/agent-acp-install-smoke.mjs target/debug/ahead
```

It fetches the supported entries from the ACP registry, installs each bridge
into a temporary AHEAD data home, launches it with an isolated user home, and
checks for model configuration options or an explicit authentication request.
Pi's `pi` CLI must already be on `PATH`; AHEAD installs the ACP bridge, not the
Pi CLI itself. This smoke makes no model request and removes its temporary
packages and profiles afterward. It verifies bridge installation and startup,
not authenticated turns or native Settings rendering.

To send one authenticated turn through the installed Pi ACP bridge, make
`OPENROUTER_API_KEY` available in the environment without putting its value in
shell history, then run:

```sh
AHEAD_ACP_LIVE_PI_TURN=1 node tests/agent-acp-install-smoke.mjs \
  target/debug/ahead pi-acp
```

The smoke creates an isolated Pi home and model catalog, selects the configured
OpenRouter model, sends a no-tools prompt, and removes its temporary profile.
It verifies one completed external turn, not rendered streaming, cancellation,
resume, or authenticated Codex/Claude Code sessions.

### Agent development loop

From the repository root, start `just dev /absolute/path/to/disposable-project`
in the shared terminal for agent testing; the path must be a test project, not a
real workspace. `tests/fixtures/editor-smoke/` is the reusable template: copy
it into a disposable directory and open the copy without precreating `.ahead/`
to exercise the editor's first-time setup. Its README has the smoke matrix.
The watcher takes one lock for the repository's shared Git directory, including
linked worktrees. A second `just dev` exits before starting Cargo and reports
the lock path (and holder when the platform can identify it); reuse the existing
dev terminal instead of starting another watcher. Stop that watcher with
Ctrl-C before a focused Cargo command. `just dev-lock-smoke` verifies the
duplicate-loop guard without building the app.
Plain `just dev` opens the AHEAD repository itself. Use a fresh disposable copy
when testing first-run storage or after changing the pre-production schema.
Session history is stored by the `libsql` client using its embedded local
Turso database; AHEAD has no alternate `rusqlite` or SQLx session backend.
There is one initial Turso schema and no schema-version checks, migrations or
automatic resets yet. Startup reruns the initial transactional
`CREATE ... IF NOT EXISTS` statements, but does not reconcile or validate
existing table definitions. This also creates missing AHEAD tables in a
nonempty database while leaving existing rows in place. If an existing
pre-production database has an incompatible layout, storage operations can
fail. Recreate disposable data when the schema changes. A workspace storage
failure disables agent sessions instead of substituting an in-memory session,
while the editor's file tools remain available.
On Unix, `.ahead` must be owned by the current user and not writable by other
users. New session databases use mode `0600`; opened databases and their
sidecars are tightened on reopen. Linked/special database paths and orphaned
sidecars produce a storage error instead of being followed or replaced. Keep
valuable data outside disposable pre-production workspaces.

For an isolated native smoke run, set all three absolute paths before starting
the watcher:

```sh
AHEAD_DATA_HOME=/private/tmp/ahead-smoke-data \
AHEAD_HOME=/private/tmp/ahead-smoke-runtime \
AHEAD_USER_HOME=/private/tmp/ahead-smoke-user \
just dev /private/tmp/ahead-smoke-workspace
```

`AHEAD_DATA_HOME` redirects AHEAD's app data, config, plugins, cache and logs;
`AHEAD_HOME` redirects the managed agent runtime; `AHEAD_USER_HOME` redirects
the managed agent and extension host's user-home settings and memory. A
disposable project path alone does **not** isolate installed extensions or
user-level settings. For `just dev`, setting `AHEAD_DATA_HOME` alone now defaults
the runtime and user homes to `runtime-home/` and `user-home/` beneath that data
directory. Explicit `AHEAD_HOME` or `AHEAD_USER_HOME` values take precedence.
These defaults are applied whenever the watcher launches the app; they do not
change an already-running process.

Without `AHEAD_DATA_HOME`, app-private data uses the operating system's normal
application-data location. The workspace keeps project settings and session
history in `.ahead/`; AHEAD does not create an `ahead-data/` directory beside
the project or executable. That directory was used by the removed portable
build mode.

On macOS and Linux, AHEAD captures only `PATH` from the configured interactive
login shell once at startup, following Zed's login-shell environment pattern.
This makes tools available from a GUI launch visible to the integrated
terminal, language servers and agent subprocesses. Common POSIX shells and
Fish are supported; capture failures leave the inherited `PATH` unchanged and
are logged. Other shell variables and per-project `direnv`/environment hooks
are not captured by this startup step.

The recipe launches the app once, then rebuilds
and relaunches it when watched Rust sources, Cargo manifests, or build inputs
change. It also watches repository-level files embedded at compile time,
including `.ahead/.gitignore`, the default settings/themes, and app icons. The
watcher targets `.ahead/.gitignore` rather than the whole `.ahead` directory,
so changing runtime session state does not trigger a rebuild. Language-extension
host sources, WIT interfaces and the patched GPUI source are watched too. On
macOS, the private launch
recipe builds the root `ahead` binary into a stable development bundle at
`target/debug/macos/Ahead.app`, then runs that bundle's executable in the
watcher's process tree. On macOS that copied bundle identifies as `io.ahead.dev`
(`AHEAD Dev`), separate from the installed `io.ahead` app. Bind native UI
  checks to `AHEAD Dev` and reuse the same disposable workspace instead of making
  a new temporary bundle for each test.
  When attaching native UI automation on macOS, wait until that bundle is
  running and bind its existing bundle ID. Looking it up by path before its
  window appears can launch a second, unisolated copy.
  This does not bypass OS or computer-use approval requirements. Other platforms
build the root `ahead` package and run `target/debug/ahead`. Use the
package-qualified command `cargo run -p ahead --bin ahead` for an app-only launch.
It does not provide the watcher or the bundled Code Mode host; use `just dev` for
the full development loop.
Test-only `tests/` trees, `tests.rs` modules and `*_tests.rs` files, plus source
`README.md` and `AGENTS.md` files, are ignored so editing tests or guidance does
not rebuild the editor. `TODO.md`, `docs/development/`, and these build
instructions are also ignored, avoiding Cargo relinks for planning and workflow
edits. User guides under `docs/guide/`, embedded Markdown prompts and bundled
skill files remain watched because they are compiled into the app.

The root and editor build scripts watch only their own linker setup. Ordinary
documentation edits should not rerun those scripts and relink the app on the
next Cargo invocation; Rust sources and embedded assets retain their normal
dependency tracking. To investigate an unexpected rebuild with the watcher
stopped, set `CARGO_LOG=cargo::core::compiler::fingerprint=info` on the build
command and inspect the reported dirty input before clearing any caches.

Launching the native macOS app from a restricted agent terminal may require
native-app access even when compilation succeeds. Stop the failed watcher
before retrying the same `just dev` command with that access; keep the stable
bundle and disposable workspace. This is separate from computer-use approval
and does not permit interacting with a locked desktop. On 2026-10-01, a
restricted `just dev` built successfully but aborted in macOS
`_RegisterApplication` with signal 6 before AHEAD window setup; rerunning the
same recipe with native-app access launched the disposable workspace normally.
Do not classify that registration failure as an editor crash without checking
the crash report.

All linked Git worktrees in a repository share the repository's `target/`
directory for Cargo commands run through `just`, so dev, test, and focused
check recipes can reuse compiled artifacts across agent worktrees. Just derives
the target from Git's common directory and honors an explicit
`CARGO_TARGET_DIR` override. Direct Cargo commands do not inherit Just's
environment; from another worktree, set `CARGO_TARGET_DIR` to the common
repository `target/` yourself if artifact reuse is intended. `just dev` also
takes a per-user, per-repository OS lock (`lockf` on macOS or `flock` on Linux)
and fails immediately if another dev watcher already owns it. When available,
`lsof`/`fuser` reports the lock owner; reuse the existing watcher terminal
rather than starting another build loop. The guard does not attach to or stop
watchers that were already running when it was added. Keep one broad Cargo
build loop active: do not start a second `just dev`, `bacon run`, or
workspace-wide Cargo build/check/test from another task at the same time.
To exercise the duplicate-loop guard without compiling or launching a watcher,
stop any active AHEAD dev loop and run `just dev-lock-smoke`; it holds the same
repository lock and substitutes an inert `watchexec` command for the nested
`just dev` invocation.
Ctrl-C stops the watcher and reaps its app/proxy child processes. The watched
command runs through a nested reaper so Watchexec restart and shutdown signals
reach the `_run-dev` shell and its app; `just dev-shutdown-smoke` checks the
nested signal-forwarding path. The reapers also stop a child that remains alive
if `wait` returns early on an interrupt.
Before a one-off Cargo check or test, coordinate with the loop owner and stop
the watcher with Ctrl-C; restart it with `just dev` afterward. Prefer a focused
check such as `just ahead-check` when that is sufficient.

After `cargo clean`, `just test-all` builds `target/debug/ahead` for native-agent
tests. For a package-focused test, run `cargo build -p ahead --bin ahead`
first, or use `just dev` for a rendered pass; stop its watcher before the test
and resume it afterward. Keep the package selection stable across repeated
checks: switching between proxy-only and app-plus-proxy Clippy runs can change
Cargo's unified dependency features and rebuild much of the retained agent closure. On
2026-10-01, separate package-focused test runs reused existing test artifacts
more effectively than a new combined-package run; use the already-cached set
when possible.

After the current `just dev` loop has rebuilt `target/debug/ahead`, run
`just agent-attribution-smoke` for a credential-free disposable acceptance of
shell source-write denial, generated build output, Code Mode edits, direct
OpenAI-compatible `apply_patch` function calls, Turso CodeAnchors, and the
resulting staged commit metadata/anchor cleanup. The recipe uses the shared
Cargo target for its temporary fixture and never builds the editor itself.

Save or explicitly discard disposable-project edits before stopping or
restarting the watcher. AHEAD's Close Window and Quit actions check dirty
buffers, but watcher termination/signals do not run those prompts. Dirty file
buffers now queue private recovery snapshots in `.ahead/session.db` every
250 ms, coalescing changes while a write is pending. Restart restores the last
acknowledged snapshot into an unsaved tab without writing the source file.
File > Recover Unsaved Changes retries restoration when another dirty tab
prevented it. This is not a guarantee for the last keystroke: abrupt termination
can lose changes that have not reached storage, and GPUI allows only 200 ms
for the best-effort shutdown flush. Files over 32 MiB are not backed up; errors
appear in the editor status. Continue using disposable projects for checks.

After building, `node tests/editor-recovery-smoke.mjs target/debug/ahead`
exercises recovery through real proxy processes in a fresh temporary project.
It kills only its own proxies and keeps the fixture for inspection. It sends
no model requests and does not install language servers. This protocol check
does not replace the native restart and recovery-menu journey.

With the watcher stopped, run
`cargo test --locked -p ahead-proxy --lib session_storage_ -- --test-threads=1`
for initial-schema creation, four-process concurrent startup/history writes,
hot-journal recovery, file privacy, and retry after a blocked commit. These
tests use disposable databases. Connections wait up to five seconds for a
database lock; persistent contention returns an error rather than retrying
forever or replacing the database. The blocked-commit tests shorten only their
own connection's timeout and verify that rollback leaves it usable for retry.
Linux CI runs these with the workspace tests; macOS and Windows have an
explicit storage filter. Platform CI results remain separate from a local pass.

`node tests/agent-shutdown-smoke.mjs target/debug/ahead` checks an active managed
turn across real proxy exit and Turso reopen. It uses a loopback mock model in a
disposable project, streams a partial answer, then closes the proxy both explicitly
and through stdin EOF. Reopen must retain the partial answer and cancelled status.
No provider credentials or downloads are needed. The fixture remains on disk;
forced process cleanup is used only when the test fails. This does not verify
authenticated tools, external ACP descendants or native window interaction.

`node tests/agent-mcp-smoke.mjs target/debug/ahead` exercises managed MCP through
the rebuilt proxy in a disposable project. It uses a loopback mock model and the
offline stdio fixture to verify modern discovery, tool-list exposure, per-call
approval events, tool execution, resource/template listing, resource reads and
model continuation. It contacts no external service or provider. Local loopback
access is required; this proxy test does not verify rendered approval UI, legacy
approval UI or in-flight cancellation.

`node tests/git-source-control-smoke.mjs target/debug/ahead` exercises the
Source Control proxy in a disposable Git repository with two local bare remotes.
It covers status and diffs, per-file and bulk staging, commit history, push,
branch changes, and merge-conflict protection and resolution. It needs no
provider credentials or network access. A pass verifies the Git/proxy path,
not the rendered Source Control panel or remote authentication.

With the watcher stopped, run
`cargo test --locked --offline -p ahead-app --lib terminal::tests -- --test-threads=1`
for Unix PTY cleanup and final-output retention. These tests launch `/bin/sh`
in disposable directories, including a shell and foreground job that ignore
HUP/TERM. They do not source the user's interactive shell configuration.
The unread-output regression stops the reader before releasing a waiting
shell, then requires process exit and the shutdown receipt within the same
three-second bound. It catches the macOS PTY-drain stall during child reap.
Normal-exit output retention is checked separately.
`cargo test --locked --offline -p ahead-app --lib app::lifecycle_tests -- --test-threads=1`
checks terminal close, recovery and Cancel routing through GPUI. Neither suite
proves native window/quit behavior or debugger cleanup: DAP's `runInTerminal`
currently uses the separate proxy terminal path.

`cargo test --locked --offline -p ahead-app --lib settings_lock_serializes_independent_writers`
checks Settings lock exclusion and scope-exit release while a duplicate file
handle remains open, modelling a handle inherited during concurrent process
creation. For the concurrent editor sweep use
`cargo test --locked --offline -p ahead-app --lib -- --test-threads=16`;
keep the single Cargo build slot even when the test harness uses more threads.

`cargo test --locked --offline -p ahead-app --lib settings -- --test-threads=1`
also checks coalesced background saves, rendering under lock contention,
concurrent MCP changes, failed-save retry and close/quit generation checks. It
also covers asynchronous bootstrap, watcher-driven reloads and late loads
racing with edits. The latter runs 20 GPUI scheduler seeds by default.
These use temporary workspaces and fake provider names, with no model calls.
The timeout test uses GPUI's virtual clock. The contention fixtures have a
five-second safety release so synchronous IO regressions fail instead of
hanging the suite. The startup fixture installs its view into an empty test
window before pumping background work; `add_window_view` itself drains that
work before returning. Native close/quit and forced termination need separate
verification after rebuilding the app.

`cargo test --locked --offline -p ahead-app --lib model_picker -- --test-threads=1`
checks background model loading, unsent draft preservation, invalid-layer
warnings, selection changes while loading, removal and coalesced reloads.
The startup fixture changes the file after constructing/drawing the panel but
before pumping its worker, so foreground reads are detectable. Choice and
coalescing tests run 20 scheduler seeds. They use disposable settings and do
not contact model providers or launch ACP agents.

`node tests/dap-shutdown-smoke.mjs target/debug/ahead` checks debugger lifecycle
on Unix with disposable mock adapters. It covers proxy shutdown after launch,
stdin EOF during initialization, and enough stderr to require a live reader.
No debugger installation, model access or real target program is needed. The
previous executable fails these checks. The ownership fix has focused test
coverage, but its rebuilt-app smoke replay remains in `TODO.md`. Run
`cargo test --locked --offline -p ahead-proxy --lib plugin::dap::tests -- --test-threads=1`
for adapter ownership, stderr, EOF and initialization cleanup without building
the UI executable. It also checks async/sync request expiry, late-reply
isolation, and direct-child exit while a descendant holds its pipe open.
These checks do not establish debuggee or native UI behavior.
The lifecycle regressions also cover Stop during stalled initialization,
missing adapters, rejected launch/step requests, late step replies after a new
breakpoint, and disconnect arguments followed by adapter reap. The new cases
compiled on 2026-10-01, but disk-full linker failures prevented execution;
rerun this filter and the full proxy suite once space is available.
Later submission regressions cover old-generation launch/control requests,
terminal requests queued after Stop, and async Continue/Pause errors. Those
edits still need compiler and execution validation. A test-only
`cargo rustc --locked --offline -p ahead-proxy --lib --profile test -- -C strip=symbols`
attempt also ran out of disk space; it is not a validated workaround.
Failed fixtures are cleaned up
only within their isolated process groups, and their event logs remain on disk.

`cargo test --locked --offline -p ahead-app --lib debugger_ -- --test-threads=1`
checks debugger session IDs across serialized start/terminal messages, controls
using the stopped thread, stale-session rejection, keyboard routing and view
notifications. Failure display, retry and waiting for confirmed cleanup have
a separate GPUI test. Both GPUI tests run 20 scheduler seeds. No adapter or
debuggee is launched by these app tests. The proxy test
`cargo test --locked --offline -p ahead-proxy --lib debugger_disconnect -- --test-threads=1`
checks catalog ownership through disconnect cleanup and removal on the next
launch. Typed adapter state/error notifications now drive the bar. Native
startup, stop/restart, debuggee cleanup and the rebuilt-root smoke remain in
`TODO.md`; injected notifications are not proof that those workflows work.
The advertised `runInTerminal` capability is not implemented end to end:
the editor currently ignores its notification, and the old command field is
not serialized. The standalone shutdown fixture never requests a terminal,
so passing it would not close this gap. The native terminal handoff and its
cancellation tests are tracked separately in `TODO.md`.

For the local ACP editor bridge, run
`cargo test --locked --offline -p ahead-agent --lib acp_client::tests -- --test-threads=1`
with the watcher stopped. The socket tests use loopback only and cover split
requests, oversized input rejected before EOF, and cancellation while an
editor action is pending. Stdin tests check the frame boundary, UTF-8 and
termination after invalid input. Blocking test
servers must explicitly set accepted sockets to blocking mode when their
listener is nonblocking: macOS inherits that mode, and a read timeout does not
change it. These checks do not require an installed external agent or prove
authenticated ACP interoperability.

For installed language servers, run
`node tests/lsp-smoke.mjs target/debug/ahead` with `rust-analyzer`, `vtsls`,
`basedpyright-langserver` and `ruff` on PATH; it defaults to all three language groups.
Set `AHEAD_LSP_LANGUAGES=rust` or a comma-separated subset to run only the
servers available locally (`typescript` also exercises JavaScript). It copies
the multi-language fixture into
a temporary directory, isolates AHEAD's data/runtime/user and tool-cache homes
in a sibling temporary profile outside the project, and permits language servers
only for that fixture. A successful run removes the profile but retains the
project copy; failures retain and report both for diagnosis. It then checks
completion, definition, semantic-token
ranges, diagnostics, auto-import resolution, close/reopen and language-server restart through the
real proxy. It also prepares symbol renames, follows references and applies
cross-file declaration-rename edits to unsaved TypeScript, JavaScript and
Python snapshots, checking navigation and diagnostic repair without saving
source files. TypeScript formatting must be idempotent; TypeScript/JavaScript
and Python signature checks retain argument types. Rust additionally checks
formatting, hover, signature help, references and cross-file rename replies.
Python runs BasedPyright and Ruff together: the smoke applies Ruff formatting,
checks idempotence, combines a linter warning with a type error, repairs the
type error without losing the warning, and confirms that typed analyzer hover
still works. The fixture explicitly enables BasedPyright's unused-import warning
to exercise code actions from both servers for the same range. The smoke
resolves Ruff's fix with its owning server when needed and applies the edit to
the unsaved buffer. It also creates, edits, renames and deletes
`pyproject.toml`/`ruff.toml`, checking that formatting follows the new
configuration without restarting Ruff. These checks repeat after server restart.
The Python checks also edit `.ahead/config.toml` to reorder servers, pin hover
and formatting to separate servers, disable and re-enable Ruff, and reject a
formatting route to a disabled server. Native settings-file events must cause
the restart; each replacement must preserve unsaved snapshots and leave disk
source unchanged.
Restart must preserve unsaved text without saving it, keep TS/JS
on one replacement server, and reject completion items from the old process.
The script also checks graceful proxy exit and that its language-server
process group has stopped. It does not download servers or call a model.
Check `rust-analyzer --version` before running it: a rustup shim on PATH can
exist without the component installed. For a local smoke, already-installed
Zed server binaries can be added to PATH; a versioned Rust Analyzer can be
passed through `AHEAD_LSP_RUST_ANALYZER_PATH`. Normal AHEAD startup does not
depend on Zed's installation. Ruff resolution prefers the project's
`.venv/bin/ruff` (`.venv/Scripts/ruff.exe` on Windows), then PATH, then AHEAD's
managed cache. Missing Ruff binaries are acquired from the official
`astral-sh/ruff` GitHub release, using the same bounded downloads and staged
cache publication as Rust Analyzer. GitHub's release checksum is verified when
provided. The running app can acquire its built-in npm-backed
servers into an AHEAD-owned cache. It prefers system Node.js/npm and otherwise
reuses a valid cached managed Node.js runtime without a network check. On a
cache miss it downloads AHEAD's pinned Node.js release and verifies the
official SHA-256 manifest; the
`lsp-smoke.mjs` protocol check uses installed binaries by default. To allow it
to exercise managed installation for selected servers missing from PATH, run
`AHEAD_LSP_INSTALL_MISSING=1 node tests/lsp-smoke.mjs target/debug/ahead`;
this mode needs access to the public npm registry and installs into the
isolated profile's `AHEAD_DATA_HOME` with lifecycle scripts disabled, outside
the project copy. It
does not contact a model provider. The smoke retains and prints its disposable
workspace path for inspection, and now reports a selected server's startup
error immediately instead of waiting for the diagnostics timeout.
For a clean-PATH cache replay, invoke the script through an absolute Node
binary while `PATH` excludes Node and npm. Set `AHEAD_LSP_KEEP_PROFILE=1` on
the first run; the script prints its retained profile. On a second run, set
`AHEAD_LSP_NODE_RUNTIME_SEED` to that profile's `data/cache/node-runtime` and
`AHEAD_LSP_CACHE_SEED` to `data/cache/language-servers`. Both runs create fresh
disposable profiles and workspaces; remove the retained first profile afterward.

To install HTML from Zed's live public catalog and exercise its language server
and grammar captures by themselves, run this from the AHEAD repository root:

```sh
AHEAD_LSP_INSTALL_MISSING=1 \
AHEAD_LSP_INSTALL_ZED_HTML=1 \
AHEAD_LSP_LANGUAGES=html \
node tests/lsp-smoke.mjs target/debug/ahead
```

This downloads a compatible HTML extension and its required assets into the
smoke's isolated AHEAD data home, then verifies HTML completion and the
extension-provided `tag` syntax capture. It needs access to the public Zed
catalog and package host. The script removes the profile on success and prints
the disposable workspace path so it can be inspected and removed afterward.
This checks proxy/server and grammar capture behavior, not editor theme
resolution or native syntax colors.

The Proto extension has a separate live smoke for its API 0.7 guest, grammar,
Buf startup, restart and uninstall:

```sh
AHEAD_LSP_INSTALL_MISSING=1 \
AHEAD_LSP_INSTALL_ZED_PROTO=1 \
AHEAD_LSP_LANGUAGES=proto \
node tests/lsp-smoke.mjs target/debug/ahead
```

Buf is acquired by the extension into the isolated profile. This smoke does
not assert a completion item; Buf returned `null` for the tested message-type
position, so Proto completion remains to be checked in a supported context.

To exercise an installed Zed HTML extension without installing it into
AHEAD, point `ZED_EXTENSIONS` at Zed's extensions directory and use the HTML
server package from that same installation. The isolated user home prevents
the smoke from reading personal `~/.ahead/settings.toml` settings.

```sh
ZED_EXTENSIONS=/path/to/Zed/extensions
smoke_profile="$(mktemp -d "${TMPDIR:-/tmp}/ahead-html.XXXXXX")"
mkdir -p "$smoke_profile/data" "$smoke_profile/user"
AHEAD_DATA_HOME="$smoke_profile/data" \
AHEAD_USER_HOME="$smoke_profile/user" \
AHEAD_ZED_EXTENSION_SOURCE="$ZED_EXTENSIONS/installed/html" \
PATH="$ZED_EXTENSIONS/work/html/node_modules/@zed-industries/vscode-langservers-extracted/bin:$PATH" \
cargo test --locked --offline -p ahead-proxy --lib installed_zed_html_extension_refreshes_settings_and_uses_a_real_server -- --ignored
```

The ignored `installed_zed_html_extension_refreshes_settings_and_uses_a_real_server`
test copies `extension.toml` and `extension.wasm` into a disposable extension
root alongside the HTML language config, then runs Zed's matching HTML server.
It enables embedded-CSS validation in project settings, observes an unknown
property diagnostic, disables validation and verifies the diagnostic clears
without restarting the server. It then requests `<di` completion, formats
unformatted markup and renames a paired tag through LSP. This exercises the
catalog's workspace-settings refresh event, not the native filesystem watcher
or rendered editor. The HTML server registers workspace-folder-change
dynamically; AHEAD currently supplies a static single workspace root. The test
does not modify the source extension or exercise gallery installation.

`node tests/lsp-shutdown-smoke.mjs target/debug/ahead` needs only Node and the
built app on Unix. It creates disposable fake LSP executables to test explicit
shutdown, stdin disconnect, disconnect during initialization and an
unresponsive server. It checks the actual shutdown/exit messages, process
termination and unchanged source files. Both smoke scripts kill only their
own process group if a check fails.

In the app, open Language Servers and select Restart after a server exits, a
manual installation changes, or an automatic npm install fails. The panel
shows initialization, install and process failures. This restarts workspace
language servers, not AHEAD or the dev
watcher. Stop requests use LSP `shutdown` followed by `exit`, with a five-second
deadline before force termination. A sent request that gets no reply fails
after 120 seconds; normal request timeouts send `$/cancelRequest` and keep the
server available for later requests. These defaults follow Zed. Native
restart/timeout feedback and per-server settings remain tracked in `TODO.md`.
On authorized window close, AHEAD releases buffers and shuts down that window's
proxy after recovery is confirmed. The proxy waits for language-server cleanup
before exit, including after an app-side stdin disconnect. Cleanup runs outside
GPUI's foreground thread. Native Cmd+Q/close behavior, blocked extension startup
and non-LSP subprocess cleanup still need the checks listed in `TODO.md`.

Native turn tests in `ahead-agent` and `ahead-proxy` re-enter the built AHEAD
executable for the filesystem sandbox helper; the Rust test harness cannot
serve that protocol. Build the app for the same Cargo profile and target
first. For default debug tests, `just dev` or
`cargo build -p ahead --bin ahead` supplies `target/debug/ahead`; stop the dev
watcher before running tests. `just test-all` and CI build the debug helper
before library tests, including after `cargo clean`. `just test-all` uses
temporary app/user homes and leaves `AHEAD_HOME` unset so managed-runtime tests
exercise the workspace-scoped `.ahead/runtime` default.
The proxy's dev-dependency enables `ahead-agent/test-support`, since Rust's
`cfg(test)` does not propagate to dependencies. Only test executables under
the profile's `deps/` directory use the sibling AHEAD helper; a missing binary
is an explicit error. Normal app builds do not enable this feature, and the
tests retain the production sandbox.

With enough build space, a one-off check can select agent and proxy together:

```sh
cargo test --locked -p ahead-agent -p ahead-proxy --lib -- --test-threads=1
```

This keeps `test-support` enabled across both test targets instead of rebuilding
the native runtime for separate feature sets. `just bacon-test` already groups
these packages for the continuous test loop.

When disk space is constrained, reuse each package's existing test target
instead. A combined selection can require another large agent test executable
because of feature unification. Do not delete caches without authorization or
disable the sandbox to make tests pass.

`cargo test --locked --offline -p ahead-agent --lib lock -- --test-threads=1`
checks MCP and ACP lock exclusion and release with retained duplicate handles.
`cargo test --locked --offline -p ahead-proxy --lib ahead::recovery -- --test-threads=1`
checks recovery ownership and claim leases against a disposable Turso store.
These tests must pass without serial reruns masking an inherited-lock failure;
the old close-only release fails their deterministic duplicate-handle checks.
They do not prove cross-platform or native process-exit behavior.

With the watcher stopped, the shared-feature installer check is
`cargo test --locked --offline -p ahead-agent -p ahead-proxy --lib adapters::tests -- --test-threads=1`.
The focused tests cover missing-cache repair, failed repair, preservation of
leased generations, malformed manifests, path escapes and concurrent installs.
To exercise real Node/npm without registry access, use the same package
selection with the filter
`adapters::tests::npx_generations_launch_locally_packed_packages_offline` and
append `-- --ignored --exact --test-threads=1`. It packs two disposable local
packages, installs them offline with private npm config/cache and lifecycle
scripts disabled, deletes the current fixture's executable, and verifies that
repair leaves both the repaired version and earlier generation runnable.
This does not verify public registry downloads or the native installation UI.

The opt-in `adapters::tests::curated_acp_session_model_options_offline` check
needs the public ACP registry, npm and the Pi CLI on PATH. Despite its name,
only model inference is offline: it downloads registry metadata and installs
the current Pi, Codex and Claude Code packages. It checks package provenance,
ACP initialization, session creation, model selection and saved per-agent
defaults. Authentication failures fail the test rather than silently skipping
model verification. No model prompt or usable provider credential is supplied.
On macOS/Linux, keep npm configuration isolated and lifecycle scripts disabled:

```sh
acp_test_config=$(mktemp -d)
npm_config_userconfig="$acp_test_config/user.npmrc" \
npm_config_globalconfig="$acp_test_config/global.npmrc" \
npm_config_ignore_scripts=true \
cargo test --locked --offline -p ahead-agent -p ahead-proxy --lib \
  adapters::tests::curated_acp_session_model_options_offline \
  -- --ignored --exact --nocapture --test-threads=1
```

Cargo's `--offline` does not block the test's registry/npm downloads. Agent
homes, projects and package caches are temporary; child HTTP proxy settings
point at an unreachable localhost port. This check does not verify a real
provider login, model turn, native picker or install/reconnect UI.

To exercise the app's curated ACP install path against the current public
registry without sending model prompts or using provider credentials, run:

```sh
node tests/agent-acp-install-smoke.mjs target/debug/ahead
```

The smoke installs Pi, Codex and Claude Code adapters into isolated temporary
profiles, launches each ACP process, and verifies that the installed package
provenance and either model/config options or an authentication-required state
reach AHEAD. It needs network access to the ACP registry and npm; Pi additionally
requires the Pi CLI on `PATH`. A missing or stale registry is fetched while the
picker remains in its loading state; if refresh fails, AHEAD uses its last valid
cache or the curated pinned fallback. This check does not verify rendered
picker/login behavior, an authenticated turn, cancellation or resume.

To exercise session-bound FIM through the integrated proxy without an external
model key, run the disposable loopback-provider smoke:

```sh
node tests/agent-fim-smoke.mjs target/debug/ahead
```

It creates temporary AHEAD sessions and a project with root and nested
`AGENTS.md` files, captures actual OpenAI-compatible completion prompts at a
local mock endpoint, and verifies the applicable instructions, session
objective and decisions, editor prefix/suffix, an unsaved related editor buffer,
session-switch isolation, the bound inline-completion response and completion
round-trip. It makes no external provider request and removes its temporary
project and profile. This does not verify native ghost-text rendering or
overlap with a live provider reply.

`just bacon-test` runs the focused agent, proxy, and viewmodel test set.
`just test-all` explicitly runs maintained AHEAD package tests and the retained
patch-library security/fixture tests. Copied runtime unit-test harnesses are not
AHEAD product gates; their production libraries still compile as dependencies.
The recipe sets `RUST_TEST_THREADS=1`: concurrent native mock-model tests can
exhaust their ten-second response windows, while the serial agent suite passed
faster on the development machine.
The command can take
substantially longer and use more disk than a focused regression; run it when
the shared build loop is stopped and workspace build space is available.

The root workspace deliberately shares one `Cargo.lock` and `target/` across
the editor and native agent. `.cargo/config.toml` limits each individual
process to two jobs, but cannot serialize separate processes. If repeated
dependency/profile changes leave many obsolete hashed artifacts and disk space
becomes the bottleneck, `just clean` removes only regenerated workspace build
output; the next build is intentionally cold.

## Building using Docker or Podman

Packages available in releases are built using containers based on multi-stage Dockerfiles. To easily orchestrate builds, there is a `docker-bake.hcl` manifest in root of repository that defines all stages and targets.
If you want to build all packages for ubuntu, you can run `RELEASE_TAG_NAME=nightly docker buildx bake ubuntu` (`RELEASE_TAG_NAME` is a required environment variable used to tell what kind of release is being built as well as baking in the version itself).
To scope in to specific distribution version, you can define target with it's version counterpart from matrix, e.g. to build only Ubuntu Focal package, you can run `RELEASE_TAG_NAME=nightly docker buildx bake ubuntu-focal`.
Additionally to building multiple OS versions at the same time, Docker-based builds will also try to cross-compile Ahead for other architectures.
This does not require QEMU installed as it's done via true cross-compilation meaning `HOST` will run your native OS/CPU architecture and `TARGET` will be the wanted architecture, instead of spawning container that's running OS using `TARGET` architecture.

> ![WARNING]
> Do not run plain targets like `ubuntu` or `fedora` if you don't have very powerful machine, as it will spawn many concurrent jobs
> which will take a long time to build.
