#!/usr/bin/env bash
# Build dist/depweave-<version>-x86_64.AppImage (+ .sha256).
#
# Requirements (Arch): rust, plus appimagetool. appimagetool is not in the Arch
# repos; use the AUR package or the upstream release:
#   https://github.com/AppImage/appimagetool/releases  (appimagetool-x86_64.AppImage)
# Optionally pin the AppImage runtime instead of letting appimagetool fetch it:
#   RUNTIME_FILE=/path/to/runtime-x86_64  (https://github.com/AppImage/type2-runtime/releases)
#
# Environment:
#   APPIMAGETOOL   path to appimagetool (default: first one in PATH)
#   RUNTIME_FILE   AppImage type2 runtime to embed (optional)
#   SKIP_BUILD=1   reuse target/release/depweave
#
# Library policy: depweave is Arch-only and uses Slint's software renderer, so
# nothing is bundled. The binary links only glibc / libgcc_s; winit and the
# font code dlopen libwayland-client, libxkbcommon(-x11), libX11/libxcb/
# libXcursor/libXi/libXrandr and fontconfig/freetype from the host at runtime.
# Those (and glibc / GPU drivers) must come from the host; they are part of
# any Arch desktop install. The script fails if the binary grows an
# unexpected hard library dependency that would need bundling.
set -euo pipefail

cd "$(dirname "$0")"
VERSION="$(cargo pkgid -p depweave-gui | sed 's/.*[#@]//')"
APPDIR="$PWD/AppDir"
OUT="dist/depweave-${VERSION}-x86_64.AppImage"
BIN=target/release/depweave

if [[ "${SKIP_BUILD:-0}" != 1 ]]; then
    cargo build --release --locked -p depweave-gui
fi

# --- dependency check: only host-provided core libraries may be NEEDED --------
allowed='^(libfontconfig\.so\.1|libc\.so\.6|libm\.so\.6|libgcc_s\.so\.1|libdl\.so\.2|libpthread\.so\.0|librt\.so\.1|ld-linux-x86-64\.so\.2)$'
bad=0
while read -r lib; do
    if [[ ! "$lib" =~ $allowed ]]; then
        echo "error: unexpected NEEDED library $lib (bundle it or extend the allow-list)" >&2
        bad=1
    fi
done < <(readelf -d "$BIN" | sed -n 's/.*Shared library: \[\(.*\)\]/\1/p')
[[ $bad == 0 ]] || exit 1

# --- validate packaging metadata when the validators are available -----------
if command -v desktop-file-validate >/dev/null; then
    desktop-file-validate packaging/depweave.desktop
fi
if command -v appstreamcli >/dev/null; then
    appstreamcli validate --no-net packaging/io.github.depweave.depweave.metainfo.xml
fi

rm -rf "$APPDIR"
install -Dm755 "$BIN" "$APPDIR/usr/bin/depweave"
install -Dm644 packaging/depweave.desktop "$APPDIR/usr/share/applications/depweave.desktop"
install -Dm644 packaging/depweave.svg "$APPDIR/usr/share/icons/hicolor/scalable/apps/depweave.svg"
install -Dm644 packaging/io.github.depweave.depweave.metainfo.xml \
    "$APPDIR/usr/share/metainfo/io.github.depweave.depweave.metainfo.xml"
install -Dm644 LICENSE "$APPDIR/usr/share/licenses/depweave/LICENSE"

# AppImage top-level entries
ln -sf usr/share/applications/depweave.desktop "$APPDIR/depweave.desktop"
ln -sf usr/share/icons/hicolor/scalable/apps/depweave.svg "$APPDIR/depweave.svg"
ln -sf depweave.svg "$APPDIR/.DirIcon"
cat > "$APPDIR/AppRun" <<'RUN'
#!/bin/sh
# Pass all arguments through, so `depweave-*.AppImage why bash` runs the CLI.
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/usr/bin/depweave" "$@"
RUN
chmod +x "$APPDIR/AppRun"

TOOL="${APPIMAGETOOL:-$(command -v appimagetool || command -v appimagetool-x86_64.AppImage || true)}"
if [[ -z "$TOOL" ]]; then
    echo "AppDir ready at $APPDIR, but appimagetool was not found (set APPIMAGETOOL)." >&2
    exit 1
fi
args=()
if [[ -n "${RUNTIME_FILE:-}" ]]; then
    args+=(--runtime-file "$RUNTIME_FILE")
fi

mkdir -p dist
rm -f "$OUT" "$OUT.sha256"
# APPIMAGE_EXTRACT_AND_RUN lets appimagetool (itself an AppImage) run without FUSE (CI containers)
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$TOOL" --no-appstream "${args[@]}" "$APPDIR" "$OUT"
(cd dist && sha256sum "$(basename "$OUT")" > "$(basename "$OUT").sha256")
echo "built $OUT ($(du -h "$OUT" | cut -f1))"
cat "$OUT.sha256"
