#!/bin/sh
# Installs Norte Code: downloads the binary of the latest release, checks its
# checksum and puts it in your PATH.
#
#   curl -fsSL https://raw.githubusercontent.com/JeanVydes/NorteCode/dev/scripts/install.sh | sh
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

os="$(uname -s)"
arch="$(uname -m)"
windows=0

case "$os" in
Linux) os="unknown-linux-gnu" ;;
Darwin) os="apple-darwin" ;;
MINGW* | MSYS* | CYGWIN*) os="pc-windows-msvc" windows=1 ;;
*) die "unsupported system: $os" ;;
esac

case "$arch" in
x86_64 | amd64) arch="x86_64" ;;
arm64 | aarch64) arch="aarch64" ;;
*) die "unsupported architecture: $arch" ;;
esac

target="$arch-$os"

# The release publishes a tarball for Unix and a zip for Windows, and the
# binary inside is named after the platform. The unpacker has to match the
# package, so it is required here and not before.
if [ "$windows" = 1 ]; then
	asset="norte-code-$target.zip"
	binary="norte-code.exe"
	need unzip
else
	asset="norte-code-$target.tar.gz"
	binary="norte-code"
	need tar
fi

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

# The checksum tells the published binary apart from a damaged download. The
# SHA256SUMS file lists every platform's asset, so the line for this one is
# read out and compared: checking the whole file would fail on the assets this
# run did not download.
file="$tmp/$asset"
if command -v sha256sum >/dev/null 2>&1; then
	actual="$(sha256sum "$file" | awk '{ print $1 }')"
elif command -v shasum >/dev/null 2>&1; then
	actual="$(shasum -a 256 "$file" | awk '{ print $1 }')"
elif command -v openssl >/dev/null 2>&1; then
	actual="$(openssl dgst -sha256 "$file" | awk '{ print $NF }')"
else
	die "no sha256 tool found (sha256sum, shasum or openssl)"
fi
expected="$(awk -v name="$asset" '$2 == name || $2 == "*" name { print $1 }' "$tmp/SHA256SUMS")"
[ -n "$expected" ] || die "SHA256SUMS does not name $asset"
[ "$actual" = "$expected" ] || die "the checksum of $asset does not match: the download may be damaged"

if [ "$windows" = 1 ]; then
	unzip -q "$file" -d "$tmp" || die "could not unpack the package"
else
	tar -xzf "$file" -C "$tmp" || die "could not unpack the package"
fi

if [ -n "${NORTE_INSTALL:-}" ]; then
	dest="$NORTE_INSTALL"
	mkdir -p "$(dirname "$dest")"
else
	dest="$PREFIX/bin/$binary"
	mkdir -p "$PREFIX/bin"
fi

mv "$tmp/norte-code/$binary" "$dest" || die "could not install into $dest"
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
