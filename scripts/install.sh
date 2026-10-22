#!/bin/sh
# Installs Norte Code: downloads the binary of the latest release, checks its
# checksum and puts it in your PATH.
#
#   curl -fsSL https://norte.vyde.dev/install.sh | sh
#
# Environment:
#   NORTE_VERSION   the release to install (the latest one by default)
#   PREFIX          where to install it ($HOME/.local by default)
#   NORTE_INSTALL   a full path for the binary, if you would rather not use PREFIX

set -eu

REPO="JeanVydes/NorteCode"
BASE="https://github.com/$REPO/releases"
PREFIX="${PREFIX:-$HOME/.local}"
VERSION="${NORTE_VERSION:-latest}"

say() { printf '%s\n' "$*"; }
die() {
	printf 'norte-code: %s\n' "$*" >&2
	exit 1
}

need() {
	command -v "$1" >/dev/null 2>&1 || die "$1 is required"
}

need uname
need tar

os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
Linux) os="unknown-linux-gnu" ;;
Darwin) os="apple-darwin" ;;
MINGW* | MSYS* | CYGWIN*) os="pc-windows-msvc" ;;
*) die "unsupported system: $os" ;;
esac

case "$arch" in
x86_64 | amd64) arch="x86_64" ;;
arm64 | aarch64) arch="aarch64" ;;
*) die "unsupported architecture: $arch" ;;
esac

target="$arch-$os"
asset="norte-code-$target.tar.gz"

if [ "$VERSION" = "latest" ]; then
	url="$BASE/latest/download/$asset"
	sums="$BASE/latest/download/SHA256SUMS"
else
	url="$BASE/download/$VERSION/$asset"
	sums="$BASE/download/$VERSION/SHA256SUMS"
fi

say "norte-code: installing $target ($VERSION)"

tmp="$(mktemp -d 2>/dev/null || mktemp -d -t norte-code)"
trap 'rm -rf "$tmp"' EXIT INT TERM

if command -v curl >/dev/null 2>&1; then
	curl -fsSL "$url" -o "$tmp/$asset" 2>/dev/null || die "could not download $url"
	curl -fsSL "$sums" -o "$tmp/SHA256SUMS" 2>/dev/null || die "could not download $sums"
elif command -v wget >/dev/null 2>&1; then
	wget -q "$url" -O "$tmp/$asset" || die "could not download $url"
	wget -q "$sums" -O "$tmp/SHA256SUMS" || die "could not download $sums"
else
	die "curl or wget is required"
fi

# The checksum tells the published binary apart from a damaged download.
(cd "$tmp" && sha256sum -c --status SHA256SUMS) 2>/dev/null ||
	(shasum -a 256 -c --status SHA256SUMS) 2>/dev/null ||
	die "the checksum does not match: the download may be damaged"

tar -xzf "$tmp/$asset" -C "$tmp" || die "could not unpack the package"

if [ -n "${NORTE_INSTALL:-}" ]; then
	dest="$NORTE_INSTALL"
	mkdir -p "$(dirname "$dest")"
else
	dest="$PREFIX/bin/norte-code"
	mkdir -p "$PREFIX/bin"
fi

mv "$tmp/norte-code/norte-code" "$dest" || die "could not install into $dest"
chmod +x "$dest"

say "norte-code: installed in $dest"

case ":$PATH:" in
*":$PREFIX/bin:"*) ;;
*) say "norte-code: add $PREFIX/bin to your PATH" ;;
esac

say ""
say "Next:"
say "  norte-code models      # the models you can use"
say "  norte-code 'hello'     # a conversation"
