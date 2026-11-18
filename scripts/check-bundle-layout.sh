#!/usr/bin/env bash
# Install / unpack the built installers and assert that every helper the hub can pick
# (crates/marsad-agent/src/hub/transport.rs HELPER_TARGETS) sits where the installed app looks
# for it: <resources>/agents/<triple>/marsad-agent[.exe], with <resources> as Tauri computes it,
# plus the sidecar daemon next to the app binary.
#
# usage: check-bundle-layout.sh <dir with the installers> <os: linux|windows|macos>
set -euo pipefail
dir="$(cd "$1" && pwd)"; os="$2"
triples=(x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-apple-darwin aarch64-apple-darwin x86_64-pc-windows-msvc)
work="$(mktemp -d)"
fail=0

# check <resources dir> <dir holding the app binary> <label>
check() {
  local res="$1" bin="$2" label="$3" t exe
  echo "== $label: resources $res, binaries $bin"
  ls -la "$bin" | head -20 || true
  for t in "${triples[@]}"; do
    exe=marsad-agent; case "$t" in *windows*) exe=marsad-agent.exe;; esac
    if [ -s "$res/agents/$t/$exe" ]; then echo "ok   agents/$t/$exe"; else echo "MISSING $res/agents/$t/$exe"; fail=1; fi
  done
  exe=marsad-agent; [ "$os" = windows ] && exe=marsad-agent.exe
  if [ -s "$bin/$exe" ]; then echo "ok   sidecar $exe"; else echo "MISSING sidecar $bin/$exe"; fail=1; fi
}

case "$os" in
  linux)
    # deb/rpm: the app is /usr/bin/marsad-desktop, resources /usr/lib/<productName>.
    for f in "$dir"/*.deb; do
      d="$work/deb"; mkdir -p "$d"; dpkg-deb -x "$f" "$d"
      check "$d/usr/lib/Marsad" "$d/usr/bin" "deb $(basename "$f")"
    done
    for f in "$dir"/*.rpm; do
      d="$work/rpm"; mkdir -p "$d"; (cd "$d" && rpm2cpio "$f" | cpio -idm --quiet)
      check "$d/usr/lib/Marsad" "$d/usr/bin" "rpm $(basename "$f")"
    done
    for f in "$dir"/*.AppImage; do
      d="$work/appimage"; mkdir -p "$d"; cp "$f" "$d/a.AppImage"; chmod +x "$d/a.AppImage"
      (cd "$d" && ./a.AppImage --appimage-extract >/dev/null)
      check "$d/squashfs-root/usr/lib/Marsad" "$d/squashfs-root/usr/bin" "AppImage $(basename "$f")"
    done
    ;;
  windows)
    # Tauri on Windows: resources = the directory of the app binary.
    for f in "$dir"/*-setup.exe; do
      d="$(cygpath -w "$work/nsis")"
      powershell.exe -NoProfile -Command "Start-Process -Wait -FilePath '$(cygpath -w "$f")' -ArgumentList '/S','/D=$d'"
      check "$work/nsis" "$work/nsis" "NSIS $(basename "$f")"
    done
    for f in "$dir"/*.msi; do
      d="$(cygpath -w "$work/msi")"
      powershell.exe -NoProfile -Command "Start-Process -Wait msiexec -ArgumentList '/a','\"$(cygpath -w "$f")\"','/qn','TARGETDIR=\"$d\"'"
      app="$(dirname "$(find "$work/msi" -iname 'marsad-desktop.exe' -o -iname 'Marsad.exe' | head -1)")"
      check "$app" "$app" "MSI $(basename "$f")"
    done
    ;;
  macos)
    for f in "$dir"/*.dmg; do
      m="$work/mnt-$(basename "$f" .dmg)"; mkdir -p "$m"
      hdiutil attach -nobrowse -readonly -mountpoint "$m" "$f" >/dev/null
      check "$m/Marsad.app/Contents/Resources" "$m/Marsad.app/Contents/MacOS" "dmg $(basename "$f")"
      hdiutil detach "$m" >/dev/null || true
    done
    ;;
  *) echo "unknown os $os" >&2; exit 2;;
esac
[ "$fail" = 0 ] || { echo "bundle layout check FAILED" >&2; find "$work" -maxdepth 4 | head -80; exit 1; }
echo "bundle layout OK"
