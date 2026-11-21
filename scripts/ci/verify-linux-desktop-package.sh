#!/usr/bin/env bash

set -euo pipefail

target="${1:?usage: verify-linux-desktop-package.sh <rust-target> [max-glibc]}"
# Fork: desktop Linux builds on the bare ubuntu-24.04 runner (tesseract 5 for
# the self-contained OCR bundle), so callers pass the 2.39 floor; upstream's
# 22.04 container builds keep the 2.35 default.
glibc_floor="${2:-2.35}"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
release_dir="target/${target}/release"
shopt -s nullglob
appimages=("$release_dir"/bundle/appimage/*.AppImage)
debs=("$release_dir"/bundle/deb/*.deb)

if [[ ${#appimages[@]} -ne 1 ]]; then
  echo "Expected exactly one AppImage in $release_dir/bundle/appimage, found ${#appimages[@]}" >&2
  exit 1
fi
if [[ ${#debs[@]} -ne 1 ]]; then
  echo "Expected exactly one DEB in $release_dir/bundle/deb, found ${#debs[@]}" >&2
  exit 1
fi

appimage="$(realpath "${appimages[0]}")"
desktop="$(realpath -e "$release_dir/openbitfun-desktop")"
work_dir="$(mktemp -d)"
trap 'rm -rf "$work_dir"' EXIT

# Jammy and Noble use different bzip2 SONAMEs on arm64.
desktop_dynamic="$(LC_ALL=C readelf --dynamic --wide "$desktop")"
if grep -Eq 'Shared library: \[libbz2\.so[^]]*\]' <<< "$desktop_dynamic"; then
  echo "Desktop executable must link bzip2 statically" >&2
  exit 1
fi

(
  cd "$work_dir"
  chmod +x "$appimage"
  "$appimage" --appimage-extract >/dev/null

  test -f squashfs-root/usr/bin/openbitfun-desktop
  # The host Mesa EGL driver can require symbols absent from the build host's Wayland library.
  bundled_wayland="$(find squashfs-root -name 'libwayland-client.so*' -print -quit)"
  if [[ -n "$bundled_wayland" ]]; then
    echo "AppImage must use the host Wayland client library: found $bundled_wayland" >&2
    exit 1
  fi

  tray_sonames="$(jq -er '.[]' "$script_dir/linux-appimage-tray-libraries.json")"
  bundle_lib="$PWD/squashfs-root/usr/lib"
  while IFS= read -r soname; do
    if [[ ! -f "$bundle_lib/$soname" ]]; then
      echo "AppImage is missing the tray runtime library $soname" >&2
      exit 1
    fi
  done <<< "$tray_sonames"
  if ! tray_dependencies="$(LC_ALL=C LD_LIBRARY_PATH="$bundle_lib" ldd -r "$bundle_lib/libayatana-appindicator3.so.1" 2>&1)" \
    || grep -Eq 'not found|undefined symbol' <<< "$tray_dependencies"; then
    echo "AppImage tray runtime could not resolve its dependencies" >&2
    echo "$tray_dependencies" >&2
    exit 1
  fi

  find squashfs-root -type f -print0 > "$work_dir/appimage-files"
  elf_files=()
  while IFS= read -r -d '' candidate; do
    if LC_ALL=C IFS= read -r -N 4 magic < "$candidate" && [[ "$magic" == $'\x7fELF' ]]; then
      elf_files+=("$candidate")
    fi
  done < "$work_dir/appimage-files"
  bash "$script_dir/check-glibc-floor.sh" "$glibc_floor" "$desktop" "$appimage" "${elf_files[@]}"

  module_path="$(find squashfs-root -name 'im-fcitx5.so' -print -quit)"
  if [[ -z "$module_path" ]]; then
    echo "AppImage does not contain the fcitx5 GTK3 input method module" >&2
    exit 1
  fi

  cache_path=""
  while IFS= read -r candidate; do
    if grep -qi 'fcitx' "$candidate"; then
      cache_path="$candidate"
      break
    fi
  done < <(find squashfs-root -name 'immodules.cache' -type f -print)

  if [[ -z "$cache_path" ]]; then
    echo "AppImage GTK immodules.cache does not register fcitx5" >&2
    exit 1
  fi

  echo "Verified AppImage fcitx5 GTK module: module=${module_path}, cache=${cache_path}"
)

# Resolve the generated control metadata against the build host's apt
# sources, including alternative dependencies such as OCR.
deb_depends="$(dpkg-deb --field "${debs[0]}" Depends)"
if [[ -z "$deb_depends" ]]; then
  echo "Missing Depends field in ${debs[0]}" >&2
  exit 1
fi
if ! grep -Eq '(^|,)[[:space:]]*libayatana-appindicator3-1[[:space:]]*(,|$)' <<< "$deb_depends"; then
  echo "DEB does not declare the Ayatana tray runtime dependency" >&2
  exit 1
fi
apt-get --simulate satisfy "$deb_depends"
