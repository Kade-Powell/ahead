# gpui-base 0.6.6 fork

This is the Apache-2.0-licensed `gpui-base` 0.6.6 source from the Cargo
registry, kept in the root AHEAD workspace. Its public editor API exposes one
selection, although its editing and Undo paths already support multiple
selections. AHEAD needs that existing batch behavior for repeated LSP snippet
tabstops, following Zed's multi-cursor snippet flow.

The AHEAD-specific change is in `src/input/base/state.rs`: `selected_ranges`
and `set_selected_ranges` expose the internal selections, with a focused GPUI
test for one edit and one Undo across two selections. The Cargo manifest omits
upstream tests, benchmarks and unused dev dependencies that are not copied
here. All runtime source remains from version 0.6.6.

Run `cargo test -p gpui-base programmatic_multi_selection_edits_and_undoes_as_one_batch`
to check the boundary, then use the shared `just dev` loop for AHEAD.
