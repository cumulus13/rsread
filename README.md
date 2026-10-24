# rsread

Fast multi-language code reader written in Rust: structure tree, syntax highlighting (32 themes),
duplicate detection and Git change markers. Rust port and extension of
[pyread](https://github.com/cumulus13/pyread).

Structure analysis for **Python, Rust, Go and JavaScript** (tree-sitter, so broken code still parses).
Any other file type gets highlighting only.

## Install

Prebuilt binaries for every release (Linux gnu/musl x86_64 + aarch64 + armv7, macOS Intel + Apple Silicon,
Windows x86_64/i686/arm64, Android) are on the [Releases](https://github.com/licface/rsread/releases) page.

```sh
# Linux / macOS / Termux: detects the platform and installs the right binary
curl -fsSL https://raw.githubusercontent.com/licface/rsread/main/install.sh | sh

# from source
cargo install --git https://github.com/licface/rsread

# with cargo
cargo install rsread
```

### Android / Termux

Use the installer above, or download `rsread-aarch64-linux-android.tar.gz` (`armv7-linux-androideabi` for
32-bit phones). Building inside Termux also works: `pkg install rust git && cargo install --git https://github.com/licface/rsread`.
Clipboard mode (`rsread c`, `-y`) needs `pkg install termux-api` plus the Termux:API app.

### Releasing

Bump `version` in `Cargo.toml`, then `git tag v0.1.1 && git push --tags`. The Release workflow verifies the tag,
builds all platforms, attaches archives plus `SHA256SUMS.txt`, and publishes to crates.io if the
`CARGO_REGISTRY_TOKEN` secret is set.

## Usage

```
rsread src/main.rs                 # structure tree + duplicate warnings + Git markers
rsread app.py -m Class.method      # extract (also Type::method, or re:<regex>), repeatable
rsread app.go -L 25 50             # lines (also -L 25-50)
rsread app.js -c -s nord           # whole file with a theme
rsread src/ -m parse               # search a name across a directory (.gitignore respected)
rsread app.py -d                   # duplicates only (exit code 2 on redefinitions, handy for CI)
rsread c                           # analyze the clipboard, then interactive save
cat x.py | rsread - --json         # structure as JSON
rsread -l                          # themes with live preview
```

Flags: `-m -s -l -t -c -d -L -n/-nl -S --no-git --git-rev --copy/-y --json --no-emoji --color`.
`-S` = compact output (no left padding, extracted code dedented). `RSREAD_STYLE` sets the default theme.

## Beyond pyread

- Rust, Go and JavaScript in addition to Python; nested classes, `impl` blocks, traits, modules
- Duplicate tiers: real redefinitions (ignores `@x.setter`, `@overload`, `#[cfg]`, `try/except` variants),
  identical bodies (copy-paste), same name across scopes (`-d`)
- Per-item Git badges (`+added ~modified -deleted`) in the tree and in every code view
- Signatures, async markers, docstring/doc-comment summaries, line ranges in the tree
- "Did you mean" suggestions, multiple `-m`, multiple files / directories
- UTF-8 BOM, UTF-16 and CRLF files; truecolor with 256-color fallback; ASCII mode
- Termux: clipboard through `termux-api`; pure-Rust regex engine (no C dependency except tree-sitter grammars)

## License

[MIT](LICENSE)

## 👤 Author
        
[Hadi Cahyadi](mailto:cumulus13@gmail.com)
    

[![Buy Me a Coffee](https://www.buymeacoffee.com/assets/img/custom_images/orange_img.png)](https://www.buymeacoffee.com/cumulus13)

[![Donate via Ko-fi](https://ko-fi.com/img/githubbutton_sm.svg)](https://ko-fi.com/cumulus13)
 
[Support me on Patreon](https://www.patreon.com/cumulus13)


