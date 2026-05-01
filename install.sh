#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

BIN_DIR="${HOME}/.local/bin"
DESKTOP_DIR="${HOME}/.local/share/applications"
SRC_BIN="${PWD}/src-tauri/target/release/md-view"
LINK="${BIN_DIR}/mdv"

echo "==> installing deps"
pnpm install --frozen-lockfile 2>/dev/null || pnpm install

echo "==> building release binary"
pnpm tauri build --no-bundle

if [[ ! -x "${SRC_BIN}" ]]; then
  echo "build did not produce ${SRC_BIN}" >&2
  exit 1
fi

mkdir -p "${BIN_DIR}" "${DESKTOP_DIR}"

echo "==> writing wrapper ${LINK}"
rm -f "${LINK}"
cat > "${LINK}" <<EOF
#!/bin/sh
# WEBKIT_DISABLE_DMABUF_RENDERER=1 cuts NVIDIA+wayland cold start ~4× and
# silences the libEGL warning storm. Remove if it causes visual issues.
exec env WEBKIT_DISABLE_DMABUF_RENDERER=1 ${SRC_BIN} "\$@"
EOF
chmod +x "${LINK}"

echo "==> installing desktop entry"
cp mdv.desktop "${DESKTOP_DIR}/mdv.desktop"
update-desktop-database "${DESKTOP_DIR}" 2>/dev/null || true
xdg-mime default mdv.desktop text/markdown 2>/dev/null || true

echo
echo "done. try: mdv ${PWD}/examples/test.md"
