#!/bin/sh
# Throwaway ddev projects with made-up names, for README screenshots without client names.
#
#   sh scripts/demo.sh up     create acme-shop (running), northwind (paused), blue-harbor (stopped)
#   sh scripts/demo.sh down   delete them again: containers, volumes and folders
#
# The projects are plain PHP with no database container, so they stay light.
# HERDR_DDEV_DEMO_DIR overrides the folder (default ~/herdr-ddev-demo).
set -eu

root="${HERDR_DDEV_DEMO_DIR:-$HOME/herdr-ddev-demo}"
marker="$root/.herdr-ddev-demo"
projects="acme-shop northwind blue-harbor"

make_project() {
  dir="$root/$1"
  mkdir -p "$dir"
  if [ ! -f "$dir/.ddev/config.yaml" ]; then
    printf '<?php echo "%s";\n' "$1" > "$dir/index.php"
    (cd "$dir" && ddev config --project-name="$1" --project-type=php --omit-containers=db) \
      >/dev/null
  fi
}

up() {
  command -v ddev >/dev/null 2>&1 || { echo "ddev not found" >&2; exit 1; }
  command -v docker >/dev/null 2>&1 || { echo "docker not found" >&2; exit 1; }
  mkdir -p "$root"
  touch "$marker"
  for name in $projects; do
    make_project "$name"
  done
  echo "Starting acme-shop..."
  (cd "$root/acme-shop" && ddev start >/dev/null)
  echo "Starting northwind, then stopping only its web container (shows as paused)..."
  (cd "$root/northwind" && ddev start >/dev/null)
  docker stop ddev-northwind-web >/dev/null
  cat <<EOF

Demo projects are in $root:
  acme-shop    running
  northwind    paused
  blue-harbor  stopped

Next:
  1. herdr --session demo
  2. Open one workspace per project folder, for example:
       herdr workspace create --cwd "$root/acme-shop" --label "Acme Shop" --focus
  3. In the picker, type "demo" first: it filters on folders too, so only these show.
  4. When done: sh scripts/demo.sh down
EOF
}

down() {
  if [ ! -d "$root" ]; then
    echo "Nothing to remove: $root does not exist."
    return
  fi
  # Only delete a folder this script created.
  [ -f "$marker" ] || { echo "Refusing: $root was not created by demo.sh" >&2; exit 1; }
  for name in $projects; do
    ddev delete --omit-snapshot --yes "$name" >/dev/null 2>&1 || true
  done
  rm -rf "$root"
  echo "Removed the demo projects and $root."
}

case "${1:-}" in
  up) up ;;
  down) down ;;
  *) echo "usage: sh scripts/demo.sh up|down" >&2; exit 2 ;;
esac
