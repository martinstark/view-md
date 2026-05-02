#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

BIN_DIR="${HOME}/.local/bin"
DESKTOP_DIR="${HOME}/.local/share/applications"
SRC_BIN="${PWD}/target/release/vmd"
LINK="${BIN_DIR}/vmd"

echo "==> building release binary"
cargo build --release

if [[ ! -x "${SRC_BIN}" ]]; then
  echo "build did not produce ${SRC_BIN}" >&2
  exit 1
fi

mkdir -p "${BIN_DIR}" "${DESKTOP_DIR}"

echo "==> installing ${LINK}"
rm -f "${LINK}"
ln -sf "${SRC_BIN}" "${LINK}"

echo "==> installing desktop entry"
cp vmd.desktop "${DESKTOP_DIR}/vmd.desktop"
update-desktop-database "${DESKTOP_DIR}" 2>/dev/null || true
xdg-mime default vmd.desktop text/markdown 2>/dev/null || true

echo
echo "done. try: vmd ${PWD}/examples/test.md"
echo "        or: vmd --licenses"
