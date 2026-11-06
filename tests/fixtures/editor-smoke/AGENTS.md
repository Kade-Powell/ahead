# Smoke project instructions

This project is disposable test data. Do not access real user projects or
credentials. Keep examples offline. Run `just verify` from this project's root
for its Rust, TypeScript/JavaScript and Python checks. For live editor testing,
start AHEAD from the repository root with `just dev /absolute/path/to/this-copy`.
Stop that dev watcher with Ctrl-C before `just verify` (the Rust lane runs
Cargo), then restart `just dev` on this same copy.
