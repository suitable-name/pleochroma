# pleochroma-ui-kit

The shared Slint UI kit of the Pleochroma apps (Pleochroma Cut, the coordinator, the Android app).

- `ui/theme.slint`: the `Theme` tokens.
- `ui/shared/`: `PleoPalette`, `PleoFigures`, `PleoGlass`, `PleoRigThumb`.
- `ui/components/kit/`: the `Kit*` widgets (`kit.slint` re-exports all of them).

An app build script registers the folder as the library `pleochroma-kit`:

```rust
slint_build::CompilerConfiguration::new().with_library_paths(pleochroma_ui_kit::library_paths())
```

and its `.slint` files import `@pleochroma-kit/theme.slint`, `@pleochroma-kit/components/kit/kit.slint` and so on.
Pleochroma Cut keeps stub files at the old `apps/pleochroma-cut/ui/...` paths that re-export from the library.
