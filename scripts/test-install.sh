#!/bin/sh
# Check scripts/install.sh without network or a real build: stub cargo and curl, then make
# sure the binary is replaced by a new file (rename), never rewritten in place. Rewriting a
# running binary fails with "Text file busy" on Linux and gets it killed on macOS.
set -eu

repo=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

mkdir -p "$work/plugin/scripts" "$work/plugin/bin" "$work/stubs"
cp "$repo/scripts/install.sh" "$work/plugin/scripts/"
cp "$repo/herdr-plugin.toml" "$work/plugin/"

cat > "$work/stubs/curl" <<'STUB'
#!/bin/sh
exit 22
STUB
cat > "$work/stubs/cargo" <<'STUB'
#!/bin/sh
mkdir -p target/release
printf '#!/bin/sh\necho new\n' > target/release/herdr-ddev
chmod +x target/release/herdr-ddev
STUB
chmod +x "$work/stubs/curl" "$work/stubs/cargo"

printf '#!/bin/sh\necho old\n' > "$work/plugin/bin/herdr-ddev"
chmod +x "$work/plugin/bin/herdr-ddev"
before=$(ls -i "$work/plugin/bin/herdr-ddev" | awk '{print $1}')

(cd "$work/plugin" && PATH="$work/stubs:/usr/bin:/bin" sh scripts/install.sh >/dev/null)

after=$(ls -i "$work/plugin/bin/herdr-ddev" | awk '{print $1}')
[ "$("$work/plugin/bin/herdr-ddev")" = "new" ] || { echo "FAIL: binary not updated" >&2; exit 1; }
[ "$before" != "$after" ] || { echo "FAIL: binary rewritten in place (same inode)" >&2; exit 1; }
[ -z "$(ls -A "$work/plugin/bin" | grep -v '^herdr-ddev$' || true)" ] \
  || { echo "FAIL: temp files left in bin/" >&2; exit 1; }
echo "install.sh ok: binary replaced by rename"
