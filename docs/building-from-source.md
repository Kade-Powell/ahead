## Building from source

It is easy to build Ahead from source on a GNU/Linux distribution. Cargo handles the build process, all you need to do, is ensure the correct dependencies are installed.

1. Install the Rust compiler and Cargo using [`rustup.rs`](https://rustup.rs/). If you already have the toolchain, ensure you are using latest Rust version.

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

For day-to-day development, `cargo run -p ahead --bin ahead` (or `just dev` for the hot-reloading loop) runs the editor and its re-executed proxy process from one executable. `just dev` waits one second after a burst of file events before restarting, reducing rebuild churn while agents or formatters touch several files. Its macOS dev bundle uses the system's copy-on-write file copy to avoid duplicating the large debug executable on APFS; the system falls back to a regular copy on filesystems without cloning. On macOS, `just app` bundles it as `target/release-lto/macos/Ahead.app`.

### Agent development loop

From the repository root, start `just dev /absolute/path/to/disposable-project`
in the shared terminal for agent testing; the path must be a test project, not a
real workspace. `tests/fixtures/editor-smoke/` is the reusable template: copy
it into a disposable directory and open the copy without precreating `.ahead/`
to exercise the editor's first-time setup. Its README has the smoke matrix.
Plain `just dev` opens the AHEAD repository itself. Use a fresh disposable copy
when the app reports an unsupported session-database schema. This hard fork
does not migrate or reset older databases, and a workspace storage failure
disables agent sessions instead of substituting an in-memory session.
The editor's file tools remain available; old database files are left intact.
On Unix, `.ahead` must be owned by the current user and not writable by other
users. New session databases use mode `0600`; supported databases and their
sidecars are tightened on reopen. Linked/special database paths and orphaned
sidecars produce a storage error instead of being followed or replaced. Keep
rejected files intact for recovery with the matching build.

The recipe launches the app once, then rebuilds
and relaunches it when watched Rust sources, Cargo manifests, or build inputs
change. It also watches repository-level files embedded at compile time,
including `.ahead/.gitignore`, the default settings/themes, and app icons. The
watcher targets `.ahead/.gitignore` rather than the whole `.ahead` directory,
so changing runtime session state does not trigger a rebuild. Language-extension
host sources and WIT interfaces are watched too. On macOS, the private launch
recipe builds the root `ahead` binary into a stable development bundle at
`target/debug/macos/Ahead.app`, then runs that bundle's executable in the
watcher's process tree. Reuse that app identity and the same disposable workspace
for native UI checks instead of making a new temporary bundle for each test.
This does not bypass OS or computer-use approval requirements. Other platforms
use the package-qualified run command. Both select the root `ahead` package; bare
`cargo run --bin ahead` is ambiguous in this workspace.
Test-only `tests/` trees and `*_tests.rs` files, plus source `README.md` and
`AGENTS.md` files, are ignored so editing tests or guidance does not rebuild the
editor. Embedded Markdown prompts and bundled skill files remain watched.

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

All agent tasks in a checkout share the same `target/`. Keep one broad Cargo
build loop active: do not start a second `just dev`, `bacon run`, or workspace-wide
Cargo build/check/test from another task at the same time. Before a one-off Cargo
check or test, coordinate with the loop owner and stop the watcher with Ctrl-C;
restart it with `just dev` afterward. Prefer a focused check such as
`just ahead-check` when that is sufficient.

After `cargo clean`, build the root executable through `just dev` before
running `ahead-proxy`'s native-agent tests: they need `target/debug/ahead` as
the sandbox helper. Stop the watcher before that one-off test and resume it
afterward. On 2026-10-01, running the app and proxy test packages together
rebuilt many shared agent crates for the combined feature set; separate
package-focused runs reused their existing test artifacts more effectively.

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

`node tests/agent-shutdown-smoke.mjs target/debug/ahead` checks an active managed
turn across real proxy exit and Turso reopen. It uses a loopback mock model in a
disposable project, streams a partial answer, then closes the proxy both explicitly
and through stdin EOF. Reopen must retain the partial answer and cancelled status.
No provider credentials or downloads are needed. The fixture remains on disk;
forced process cleanup is used only when the test fails. This does not verify
authenticated tools, external ACP descendants or native window interaction.

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
`node tests/lsp-smoke.mjs target/debug/ahead` with `rust-analyzer`, `vtsls` and
`basedpyright-langserver` on PATH. It copies the multi-language fixture into
a temporary directory, then checks completion, definition, diagnostics,
auto-import resolution, close/reopen and language-server restart through the
real proxy. Restart must preserve unsaved text without saving it, keep TS/JS
on one replacement server, and reject completion items from the old process.
The script also checks graceful proxy exit and that its language-server
process group has stopped. It does not download servers or call a model.

`node tests/lsp-shutdown-smoke.mjs target/debug/ahead` needs only Node and the
built app on Unix. It creates disposable fake LSP executables to test explicit
shutdown, stdin disconnect, disconnect during initialization and an
unresponsive server. It checks the actual shutdown/exit messages, process
termination and unchanged source files. Both smoke scripts kill only their
own process group if a check fails.

In the app, open Language Servers and select Restart after a server exits or
after installing a missing server. The panel shows initialization and process
failures. This restarts workspace language servers, not AHEAD or the dev
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
watcher before running tests. CI also builds the app before tests.
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

`just bacon-test` runs the focused agent, proxy, and viewmodel test set.
`just test-all` runs maintained workspace test targets and excludes the copied
`codex-core` and `codex-mcp` unit-test harnesses, which depend on pruned upstream
test support and are not AHEAD product gates. Those libraries remain workspace
dependencies and are still compiled when required. The command can take
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
