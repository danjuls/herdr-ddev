#!/bin/sh
# Fail unless herdr-plugin.toml and Cargo.toml both carry the given version.
set -eu

wanted="$1"
manifest=$(sed -n 's/^version = "\(.*\)"/\1/p' herdr-plugin.toml | head -n 1)
cargo=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)

if [ "$manifest" != "$wanted" ] || [ "$cargo" != "$wanted" ]; then
  echo "version mismatch: wanted=$wanted herdr-plugin.toml=$manifest Cargo.toml=$cargo" >&2
  exit 1
fi
echo "version $wanted ok"
