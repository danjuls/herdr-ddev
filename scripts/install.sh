#!/bin/sh
# Build step for `herdr plugin install`: fetch the release binary for this platform, verify
# its checksum, or build from source when there is no matching release.
set -eu

repo="danjuls/herdr-ddev"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' herdr-plugin.toml | head -n 1)

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) target=aarch64-apple-darwin ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  Linux-x86_64) target=x86_64-unknown-linux-musl ;;
  Linux-aarch64 | Linux-arm64) target=aarch64-unknown-linux-musl ;;
  *) target="" ;;
esac

mkdir -p bin

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f 1
  else
    shasum -a 256 "$1" | cut -d ' ' -f 1
  fi
}

if [ -n "$target" ] && command -v curl >/dev/null 2>&1; then
  archive="herdr-ddev-$target.tar.gz"
  base="https://github.com/$repo/releases/download/v$version"
  tmp=$(mktemp -d)
  if curl -fsSL "$base/$archive" -o "$tmp/$archive" \
    && curl -fsSL "$base/$archive.sha256" -o "$tmp/$archive.sha256"; then
    expected=$(cut -d ' ' -f 1 "$tmp/$archive.sha256")
    if [ "$expected" = "$(sha256 "$tmp/$archive")" ]; then
      tar -xzf "$tmp/$archive" -C bin herdr-ddev
      chmod +x bin/herdr-ddev
      rm -rf "$tmp"
      echo "herdr-ddev: installed release v$version ($target)"
      exit 0
    fi
    echo "herdr-ddev: checksum mismatch for $archive, building from source" >&2
  fi
  rm -rf "$tmp"
fi

if command -v cargo >/dev/null 2>&1; then
  cargo build --release --locked
  cp target/release/herdr-ddev bin/herdr-ddev
  echo "herdr-ddev: built from source"
  exit 0
fi

echo "herdr-ddev: no release binary for $(uname -s)-$(uname -m) v$version and no cargo." >&2
echo "Install Rust from https://rustup.rs and reinstall the plugin." >&2
exit 1
