# AHEAD GPUI patch

This is the Apache-2.0 `gpui-pre 0.3.6` crate resolved by `gpui-kit 0.6.6`,
with two narrowly scoped fixes from Zed:

- `zed-industries/zed` commit `e4a3732b94eb0881e1d954201ed41ffd2abf18f6`:
  retain per-font character-width caches without pooling `LineWrapper`s that
  hold an `Arc<TextSystem>`.
- `zed-industries/zed` commit
  `370591f3d178718e3a822cfbf2a5879cdcccfad1`:
  defer native window cleanup until the app is no longer borrowed, with a test
  platform callback to cover the reentrant-close case.
- Bundle the two licensed fonts used by GPUI's SVG renderer tests under
  `resources/fonts`, so tests do not depend on Zed's repository-root `assets`.
  Their original IBM Plex Sans and Lilex license files are kept beside them.

Keep the crate version and upstream license intact. When a published
`gpui-pre` release contains both fixes, prefer upgrading the dependency and
removing this patch. Do not edit Cargo's registry cache.
