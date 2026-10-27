#!/usr/bin/env bash
# Package one release build of `covenant` for one target, as the release
# workflow does for each of its targets. Run from the repository root:
#
#   package.sh <target> <the built binary> <out dir>
#
# Writes <out dir>/covenant-<target>.tar.gz (a .zip for Windows) holding one
# directory, covenant-<target>/, with the binary and the LICENSE, NOTICE and
# README.md it ships under, and prints the archive's path. The Homebrew
# formula (formula.sh) and `cargo binstall` (the binstall metadata in
# Cargo.toml) look for exactly these names, so change them together.
set -euo pipefail

die() {
    echo "package.sh: $*" >&2
    exit 2
}

[ $# -eq 3 ] || die "usage: package.sh <target> <binary> <out dir>"
target=$1
binary=$2
out=$3

[[ $target =~ ^[a-z0-9_]+(-[a-z0-9_]+){2,3}$ ]] || die "not a target triple: $target"
[ -f "$binary" ] || die "no binary at $binary"
for doc in LICENSE NOTICE README.md; do
    [ -f "$doc" ] || die "no $doc here: run from the repository root"
done

name="covenant-$target"
case $target in
*windows*) exe=covenant.exe ;;
*) exe=covenant ;;
esac

mkdir -p "$out"
out=$(cd "$out" && pwd)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir "$stage/$name"
cp "$binary" "$stage/$name/$exe"
chmod 755 "$stage/$name/$exe"
cp LICENSE NOTICE README.md "$stage/$name/"

case $target in
*windows*)
    archive="$out/$name.zip"
    rm -f "$archive"
    if command -v 7z >/dev/null 2>&1; then
        (cd "$stage" && 7z a -tzip "$archive" "$name" >/dev/null)
    elif command -v zip >/dev/null 2>&1; then
        (cd "$stage" && zip -qr "$archive" "$name")
    else
        die "neither 7z nor zip is installed"
    fi
    ;;
*)
    archive="$out/$name.tar.gz"
    # COPYFILE_DISABLE: macOS's tar would otherwise add ._ resource files.
    (cd "$stage" && COPYFILE_DISABLE=1 tar -czf "$archive" "$name")
    ;;
esac

echo "$archive"
