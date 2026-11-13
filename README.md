# depweave

A native, **read-only** dependency graph explorer for Arch Linux / pacman, written in Rust
with a [Slint](https://slint.dev) UI. It never modifies the system and never needs root.

## Install (AppImage)

Download `depweave-<version>-x86_64.AppImage` and its `.sha256` from the
[releases page](https://github.com/legendarylolo318-cloud/depweave/releases), then:

```
sha256sum -c depweave-0.1.0-x86_64.AppImage.sha256
chmod +x depweave-0.1.0-x86_64.AppImage
./depweave-0.1.0-x86_64.AppImage            # GUI
./depweave-0.1.0-x86_64.AppImage why bash   # CLI
```

The AppImage needs FUSE 3 (`fuse3`, present on most Arch desktops); without it, run it with
`--appimage-extract-and-run`. Libraries such as glibc, fontconfig, freetype, libxkbcommon and the
Wayland / X11 client libraries come from the host and are not bundled.

## Features

* **Pure-Rust database reader** – parses `/var/lib/pacman/local/*/desc` and the sync
  databases (`*.db`, tar + gzip/zstd, detected by magic bytes) directly. No libalpm, so one
  binary works across pacman versions. `DBPath` / `RootDir` and the repository list are read
  from `/etc/pacman.conf`; xz-compressed custom repo databases are supported too.
* **Correct dependency resolution** – version constraints (`>= <= = < >`, with a port of
  libalpm's `vercmp`), virtual packages via `provides` (`sh`, `java-runtime`), soname
  provides (`libfoo.so=1-64`), cycles handled.
* **"Why is this installed?"** – every chain `explicit → … → package`, shortest first, one
  chain per explicit root before alternatives, "show more". Orphans are flagged with size and
  a copy-only `sudo pacman -Rns <pkg>` command. Optional dependencies show who suggests them.
* **Verified against pacman** – orphans, explicit/foreign lists, direct dependencies and reverse
  dependencies, `-Rs` removal sets and installed sizes are checked against `pacman`, `pactree` and
  `pacman -Rsp` by `scripts/verify-pacman.py` (100% match on all 1589 packages of the dev machine).
* **Graph view** – software-rendered with tiny-skia into one image (no per-node UI
  elements), `rstar` spatial index for hover/click, zoom with scroll, pan with drag, labels
  only above a zoom threshold. Force-directed layout (Barnes–Hut, computed in a background
  thread, progressive, cached in `~/.cache/depweave`) and a layered layout for focused
  subgraphs. Focus mode with 1–5 hops and dependency/dependent/both direction.
* Colour by install reason / repository (core, extra, multilib, foreign, custom) / size;
  optional size-scaled nodes; dark and light themes.
* Fuzzy search, sidebar lists (orphans, explicit, foreign/AUR, largest, largest removal
  impact), stats, auto-reload when the pacman database changes (inotify via `notify`).
* Export current view to **PNG / SVG**, whole graph or focused subgraph to **DOT / JSON**
  (files go to `~/.cache/depweave/exports/`, override with `DEPWEAVE_EXPORT_DIR`).
* **CLI** (same binary, plain text):

```
depweave why <pkg> [--limit N]
depweave orphans [--long]                    # == pacman -Qdtq
depweave list [-e] [-d] [-m] [-n] [-t]       # like pacman -Qq with the same filters
depweave deps <pkg> [--all-providers] [--optional]   # == pactree -d1
depweave rdeps <pkg> [--optional]            # == pactree -r -d1
depweave info <pkg>...                       # version, repo, reason, installed size
depweave impact <pkg> [--quiet]              # what pacman -Rs would remove
depweave export --format dot|json [--root <pkg> --depth N --direction deps|dependents|both] [--optional] [-o FILE]
```

Run `depweave` with no subcommand to start the GUI. `--dbpath DIR` points at another
pacman database (handy for tests).

## Build

```
sudo pacman -S --needed rust fontconfig freetype2 libxkbcommon wayland libx11 ttf-dejavu
cargo build --release
./target/release/depweave
```

AppImage: `./build-appimage.sh` (needs `appimagetool`; see the header of the script). Tagging
`v*` builds and attaches the AppImage in GitHub Actions.

Verify against the real pacman on your machine (read-only; `pacman -Rsp` only prints):

```
python3 scripts/verify-pacman.py target/release/depweave          # sampled
python3 scripts/verify-pacman.py target/release/depweave --all    # every package
```

Checks (also run in CI on `archlinux:latest`):

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Layout: `crates/core` (data layer, no UI), `crates/render` (tiny-skia renderer, hit-testing,
SVG), `crates/cli`, `crates/gui` (Slint app + `depweave` binary).
`cargo run --release -p depweave-gui --example layout_png -- out.png` renders the layout of
your system to a PNG and prints frame timings (dev tool).

Debug aid: `DEPWEAVE_SELFTEST=1 DEPWEAVE_SNAPSHOT=/tmp/shot depweave` drives the UI through a
scripted scenario, writes snapshots and exits.

## Comparison

| | pactree | pacgraph | depweave |
|-|-|-|-|
| Interface | CLI tree / graphviz output | generates a static SVG/PNG | interactive GUI + CLI |
| "Why installed" chains | `-r` reverse tree (all dependents) | no | explicit-rooted chains, shortest first |
| Orphans / removal impact | no | no | yes (`-Qdtq`-compatible, `-Rs` simulation) |
| Interactive zoom/focus | no | no | yes |
| Data source | libalpm | libalpm (pyalpm) | direct desc/sync-db parsing |

## Notes and limitations

* Arch Linux only. Sync databases compressed with bzip2 are reported as unsupported.
* depweave only writes below `~/.cache/depweave` (sync-name and layout caches, exports). It never
  runs pacman and never needs root.
* With several installed providers of one virtual dependency (e.g. `ttf-font`), the graph draws an
  edge to each (that is what pacman's "required by" counts); `deps` shows the single package pacman
  resolves to, `deps --all-providers` shows them all.
* The window needs about 1320 logical px of width; tiling window managers that force a narrower
  window clip the right panel.
* Fonts for graph labels are discovered at runtime (`DEPWEAVE_FONT` overrides; DejaVu/Noto
  are tried first). Without any font, labels are skipped.
* Touchpad pinch is not wired up; two-finger scroll zooms (as does the mouse wheel).

## License

MIT, see `LICENSE`.
