#!/usr/bin/env bash
# release.sh: the files a GitHub release carries (issue #52), so a package can track omabox by a
# release asset and its checksum instead of GitHub's generated archives.
#
#   ./release.sh            dist/omabox-X.Y.Z.tar.gz from the tag vX.Y.Z (X.Y.Z is VERSION) and
#                           dist/SHA256SUMS
#   ./release.sh --upload   the same, then attach both to that tag's GitHub release
#
# The tarball is `git archive` of the tag, so it holds exactly what the tag does, minus what
# .gitattributes marks export-ignore (demo media, spike/, .github/). git gzips with no timestamp:
# the same tag gives the same bytes, and the checksum can be checked against a rebuild.
set -euo pipefail

ROOT=$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")" && pwd)
die() { echo "release.sh: $*" >&2; exit 1; }

upload=0
case ${1:-} in --upload) upload=1 ;; "") ;; *) die "usage: ./release.sh [--upload]" ;; esac

v=$(cat "$ROOT/VERSION")
[[ $v =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "VERSION is not X.Y.Z: $v"
tag=v$v
# An annotated tag, as every release has: a lightweight one is a local mistake more often than not.
[ "$(git -C "$ROOT" cat-file -t "$tag" 2>/dev/null)" = tag ] || die "no annotated tag $tag (git tag -a $tag -m 'omabox $v')"

name=omabox-$v.tar.gz
out=$ROOT/dist
mkdir -p "$out"
git -C "$ROOT" archive --format=tar.gz --prefix="omabox-$v/" -o "$out/$name" "$tag"
(cd "$out" && sha256sum "$name" > SHA256SUMS)
echo "$out/$name ($(du -h "$out/$name" | cut -f1))"
cat "$out/SHA256SUMS"

[ $upload = 1 ] || exit 0
gh release view "$tag" >/dev/null 2>&1 || die "no GitHub release $tag yet (gh release create $tag ...)"
gh release upload "$tag" "$out/$name" "$out/SHA256SUMS" --clobber
echo "attached to release $tag"
