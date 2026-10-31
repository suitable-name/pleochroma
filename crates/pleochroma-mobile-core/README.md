# pleochroma-mobile-core

The UI-free core of the Pleochroma mobile app: the local design library, the shared
`pleochroma_editor::EditorSession`, settings, the cutting-instruction rows, the property and
check models, and the `Platform` trait that hides everything the operating system provides
(files, sharing, printing, clipboard, keep-awake, thermal state).

It has no UI toolkit and no Android code, so it is shared by the Android app
(`apps/pleochroma-mobile-android`) and any later iOS app, and it is tested natively on the
host with `cargo test -p pleochroma-mobile-core` against a mock platform.
