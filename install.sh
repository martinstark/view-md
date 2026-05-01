#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

BIN_DIR="${HOME}/.local/bin"
DESKTOP_DIR="${HOME}/.local/share/applications"
SRC_BIN="${PWD}/target/release/mdv"
LINK="${BIN_DIR}/mdv"

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
cp mdv.desktop "${DESKTOP_DIR}/mdv.desktop"
update-desktop-database "${DESKTOP_DIR}" 2>/dev/null || true
xdg-mime default mdv.desktop text/markdown 2>/dev/null || true

echo
echo "done. try: mdv ${PWD}/examples/test.md"
echo "        or: mdv --licenses"
