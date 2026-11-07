# Compatibility

What TileIDE can take from other editors, and what it can't. Updated with each release.

| What | Works | Notes |
|---|---|---|
| VS Code **colour themes** (`.json`, `.vsix` from Open VSX) | ✅ | Import in Themes. Workbench, syntax (TextMate scopes, approximated) and terminal colours are mapped. Semantic token colours, icon themes and product icon themes are ignored. |
| VS Code **extensions** (JavaScript, the `vscode` API) | ❌ | Not run, and not planned: TileIDE is not built on VS Code. |
| VS Code TextMate **grammars** | ❌ | Not yet. Highlighting comes from GtkSourceView's own language files. |
| VS Code **snippets** | ❌ | Not yet. |
| VS Code **keybindings** | ❌ | Not yet. Rebind keys with `tileide.bind` in `tileide.lua`. |
| Language servers (LSP) | ✅ | Built in for Rust, C/C++, Python, JS/TS, Java; any server you install. Only clangd is tested on every release. |
| Toolchains | ✅ | Per-workspace pins (Node.js, Java, Go), mise (`mise env`), asdf / `.tool-versions` / `mise.toml`. Maven and Gradle builds for Java run and debug (both tested here). |
| Debug adapters (DAP) | ✅ partly | gdb (C, C++, Rust), debugpy (Python), TileIDE's jdb bridge (Java) and js-debug (JavaScript, TypeScript through Node's type stripping or source maps), tested here. |
| Jupyter notebooks (`.ipynb`, nbformat 4) | ✅ | Kernels through your `jupyter_client` + `ipykernel`; tested with ipykernel 7 / jupyter_client 8.10. Markdown not rendered; HTML outputs shown as text. |
| TileIDE extensions (`extension.json`, any language) | ✅ | See [extensions.md](extensions.md). Tested: the bundled Python example. |
