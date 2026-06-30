#!/usr/bin/env sh
set -eu

REPO="${STARWEAVER_GITHUB_REPO:-${STARWEAVER_REPO:-Wh1isper/starweaver}}"
VERSION="${STARWEAVER_VERSION:-latest}"
if [ -n "${STARWEAVER_INSTALL_DIR:-}" ]; then
  INSTALL_DIR="$STARWEAVER_INSTALL_DIR"
elif command -v id >/dev/null 2>&1 && [ "$(id -u)" = "0" ]; then
  INSTALL_DIR="/usr/local/bin"
else
  INSTALL_DIR="$HOME/.local/bin"
fi
COMPONENTS="${STARWEAVER_COMPONENTS:-cli}"
NO_MODIFY_PATH="${STARWEAVER_NO_MODIFY_PATH:-0}"
TMP_DIR="${TMPDIR:-/tmp}/starweaver-install-$$"

log() {
  printf '%s\n' "$*"
}

fail() {
  printf 'starweaver install error: %s\n' "$*" >&2
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

fetch() {
  url="$1"
  output="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$output"
  elif command -v wget >/dev/null 2>&1; then
    wget -q "$url" -O "$output"
  else
    fail "install curl or wget"
  fi
}

fetch_stdout() {
  url="$1"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url"
  elif command -v wget >/dev/null 2>&1; then
    wget -q "$url" -O -
  else
    fail "install curl or wget"
  fi
}

resolve_version() {
  if [ "$VERSION" != "latest" ]; then
    case "$VERSION" in
      v*) printf '%s\n' "$VERSION" ;;
      *) printf 'v%s\n' "$VERSION" ;;
    esac
    return
  fi
  tag=""
  if command -v curl >/dev/null 2>&1; then
    tag="$(curl -fsSLI "https://github.com/$REPO/releases/latest" 2>/dev/null | awk -F/ '/^Location:/ {gsub(/\r/, "", $NF); print $NF; exit}' || true)"
  elif command -v wget >/dev/null 2>&1; then
    tag="$(wget --server-response --spider "https://github.com/$REPO/releases/latest" 2>&1 | awk -F/ '/Location:/ {gsub(/\r/, "", $NF); print $NF; exit}' || true)"
  else
    fail "install curl or wget"
  fi
  if [ -z "$tag" ]; then
    tag="$(fetch_stdout "https://github.com/$REPO/releases" 2>/dev/null | sed -n 's/.*href="[^"]*\/releases\/tag\/\([^"?/#]*\)[^"]*".*/\1/p' | head -n 1 || true)"
  fi
  printf '%s\n' "$tag"
}

detect_target() {
  os="$(uname -s | tr '[:upper:]' '[:lower:]')"
  arch="$(uname -m)"
  case "$arch" in
    x86_64|amd64) arch="x86_64" ;;
    arm64|aarch64) arch="aarch64" ;;
    *) fail "unsupported architecture: $arch" ;;
  esac
  case "$os" in
    linux) printf '%s-unknown-linux-gnu\n' "$arch" ;;
    darwin) printf '%s-apple-darwin\n' "$arch" ;;
    mingw*|msys*|cygwin*) printf 'x86_64-pc-windows-msvc\n' ;;
    *) fail "unsupported operating system: $os" ;;
  esac
}

verify_checksum_if_available() {
  archive="$1"
  checksum_file="$2"
  if [ ! -f "$checksum_file" ]; then
    return
  fi
  base="$(basename "$archive")"
  expected="$(awk -v name="$base" '$2 == name {print $1}' "$checksum_file" | head -n 1)"
  if [ -z "$expected" ]; then
    return
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$archive" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$archive" | awk '{print $1}')"
  else
    fail "missing sha256sum or shasum for checksum verification"
  fi
  [ "$actual" = "$expected" ] || fail "checksum mismatch for $base"
}

extract_archive() {
  archive="$1"
  dest="$2"
  case "$archive" in
    *.tar.gz)
      need tar
      tar -xzf "$archive" -C "$dest"
      ;;
    *.zip)
      need unzip
      unzip -q "$archive" -d "$dest"
      ;;
    *) fail "unknown archive format: $archive" ;;
  esac
}

install_required_file() {
  name="$1"
  src="$2"
  [ -f "$src" ] || fail "archive missing expected binary: $name"
  dst="$INSTALL_DIR/$name"
  tmp="$INSTALL_DIR/.$name.tmp.$$"
  rm -f "$tmp"
  cp "$src" "$tmp" || { rm -f "$tmp"; fail "failed to stage binary: $name"; }
  chmod 0755 "$tmp" 2>/dev/null || true
  mv -f "$tmp" "$dst" || { rm -f "$tmp"; fail "failed to replace installed binary: $name"; }
  log "installed $dst"
}

ensure_path_hint() {
  case ":$PATH:" in
    *":$INSTALL_DIR:"*) return ;;
  esac
  if [ "$NO_MODIFY_PATH" = "1" ]; then
    log "add $INSTALL_DIR to PATH"
    return
  fi
  if [ "$(id -u 2>/dev/null || printf 1)" = "0" ]; then
    log "ensure $INSTALL_DIR is on PATH for target users"
    return
  fi
  profile=""
  if [ -n "${SHELL:-}" ]; then
    case "$SHELL" in
      */zsh) profile="$HOME/.zshrc" ;;
      */bash) profile="$HOME/.bashrc" ;;
      */fish) profile="$HOME/.config/fish/config.fish" ;;
    esac
  fi
  if [ -n "$profile" ]; then
    mkdir -p "$(dirname "$profile")"
    if [ ! -f "$profile" ] || ! grep -q "$INSTALL_DIR" "$profile"; then
      case "$profile" in
        *config.fish) printf '\nset -gx PATH %s $PATH\n' "$INSTALL_DIR" >> "$profile" ;;
        *) printf '\nexport PATH="%s:$PATH"\n' "$INSTALL_DIR" >> "$profile" ;;
      esac
      log "updated PATH in $profile"
    fi
  else
    log "add $INSTALL_DIR to PATH"
  fi
}

install_component() {
  component="$1"
  target="$2"
  tag="$3"
  case "$component" in
    cli|starweaver-cli)
      asset="starweaver-cli-$tag-$target"
      install_names="starweaver starweaver-cli sw starweaver-rpc"
      ;;
    *) fail "unknown component: $component" ;;
  esac
  ext="tar.gz"
  case "$target" in
    *windows*) ext="zip" ;;
  esac
  archive_name="$asset.$ext"
  archive_url="https://github.com/$REPO/releases/download/$tag/$archive_name"
  archive="$TMP_DIR/$archive_name"
  log "downloading $archive_url"
  fetch "$archive_url" "$archive"
  checksum="$TMP_DIR/checksums.txt"
  fetch "https://github.com/$REPO/releases/download/$tag/checksums.txt" "$checksum" 2>/dev/null || true
  verify_checksum_if_available "$archive" "$checksum"
  extract_dir="$TMP_DIR/$asset"
  mkdir -p "$extract_dir"
  extract_archive "$archive" "$extract_dir"
  for name in $install_names; do
    case "$target" in
      *windows*) install_required_file "$name.exe" "$extract_dir/$name.exe" ;;
      *) install_required_file "$name" "$extract_dir/$name" ;;
    esac
  done
  if [ "$component" = "cli" ] || [ "$component" = "starweaver-cli" ]; then
    if [ -f "$INSTALL_DIR/starweaver" ]; then
      rm -f "$INSTALL_DIR/sw"
      ln -s "starweaver" "$INSTALL_DIR/sw" 2>/dev/null || cp "$INSTALL_DIR/starweaver" "$INSTALL_DIR/sw"
    fi
  fi
}

main() {
  need uname
  tag="$(resolve_version)"
  [ -n "$tag" ] || fail "could not resolve latest release tag"
  target="$(detect_target)"
  mkdir -p "$TMP_DIR" "$INSTALL_DIR"
  trap 'rm -rf "$TMP_DIR"' EXIT INT TERM
  old_ifs="$IFS"
  IFS=','
  set -- $COMPONENTS
  IFS="$old_ifs"
  for component do
    install_component "$component" "$target" "$tag"
  done
  ensure_path_hint
  log "starweaver installed from $tag"
}

main "$@"
