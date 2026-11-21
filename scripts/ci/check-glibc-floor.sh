#!/usr/bin/env bash
#
# Assert that released Linux binaries do not require a newer glibc than we
# promise to support.
#
# Usage: check-glibc-floor.sh <max-glibc> <binary> [binary...]
#
# Why this exists: the release matrix used to build x86_64 on ubuntu-22.04
# (glibc 2.35) but arm64 on ubuntu-24.04-arm (2.39). The arm64 relay therefore
# required GLIBC_2.38 and could not start at all in the deploy runtime image
# (debian:bookworm-slim, 2.36) — nor on a Debian 12 / Ubuntu 22.04 arm64 host
# for anyone running the tarball directly. Nothing anywhere declared the
# supported floor, so the mismatch was invisible until a container exited with a
# loader error on stderr that the deploy script discarded.
#
# The floor is a promise to users. Make it explicit and machine-checked, so
# raising it becomes a deliberate decision rather than a side effect of a runner
# image bump.
#
# `.gnu.version_r` is the authoritative list of versioned symbols a binary needs
# from its shared libraries; `readelf -V` prints it.

set -euo pipefail

MAX="${1:?usage: check-glibc-floor.sh <max-glibc> <binary> [binary...]}"
shift
if [ "$#" -eq 0 ]; then
  echo "ERROR: no binaries given" >&2
  exit 2
fi

# Without readelf every binary would look requirement-free and the check would
# pass everything — a silent false negative is worse than no check at all.
if ! command -v readelf >/dev/null 2>&1; then
  echo "ERROR: readelf not found (install binutils); refusing to skip the check" >&2
  exit 2
fi

# Read version requirements only. A shared library can also define symbols.
max_required_glibc() {
  LC_ALL=C readelf --version-info --wide "$1" |
    awk '
      /^Version .* section / { needs = /^Version needs section / }
      needs && match($0, /Name: GLIBC_[0-9]+\.[0-9]+(\.[0-9]+)?/) {
        print substr($0, RSTART + length("Name: "), RLENGTH - length("Name: "))
      }
    ' |
    LC_ALL=C sort -u -V |
    tail -n 1
}

status=0
for bin in "$@"; do
  if [ ! -f "$bin" ]; then
    echo "ERROR: $bin does not exist" >&2
    status=1
    continue
  fi
  if ! required="$(max_required_glibc "$bin")"; then
    echo "ERROR: could not read ELF version information from $bin" >&2
    status=1
    continue
  fi
  if [ -z "$required" ]; then
    echo "ok    $(basename "$bin"): no versioned glibc requirement"
    continue
  fi
  required="${required#GLIBC_}"
  # sort -V puts the larger version last: if that is not MAX, MAX was exceeded.
  highest="$(printf '%s\n%s\n' "$MAX" "$required" | sort -V | tail -n 1)"
  if [ "$highest" != "$MAX" ]; then
    echo "FAIL  $bin: requires glibc $required, above the $MAX floor" >&2
    status=1
  else
    echo "ok    $(basename "$bin"): requires glibc $required (floor $MAX)"
  fi
done

exit "$status"
