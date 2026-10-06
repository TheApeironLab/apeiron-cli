#!/bin/sh
# Install a checksum-verified standalone Apeiron CLI. No sudo, Node.js or Bun.
set -eu

main() {
  version=${APEIRON_VERSION:-}
  install_dir=${APEIRON_INSTALL_DIR:-"$HOME/.local/bin"}
  base=${APEIRON_DOWNLOAD_BASE:-https://apeiron-bj-cli-downloads.oss-cn-beijing.aliyuncs.com/apeiron-cli}
  modify_path=1
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --version|--install-dir)
        [ "$#" -ge 2 ] || { printf 'Missing value for %s\n' "$1" >&2; return 2; }
        if [ "$1" = --version ]; then version=$2; else install_dir=$2; fi
        shift 2 ;;
      --no-modify-path) modify_path=0; shift ;;
      --help)
        printf '%s\n' 'Usage: sh install.sh [--version VERSION] [--install-dir /absolute/path] [--no-modify-path]'
        return ;;
      *) printf 'Unknown option: %s\n' "$1" >&2; return 2 ;;
    esac
  done
  case "$base" in https://*) ;; *) printf '%s\n' 'Download base must use HTTPS.' >&2; return 2 ;; esac
  case "$install_dir" in /*) ;; *) printf '%s\n' 'Install directory must be absolute.' >&2; return 2 ;; esac
  for tool in curl tar awk mktemp; do
    command -v "$tool" >/dev/null 2>&1 || { printf 'Required command is missing: %s\n' "$tool" >&2; return 1; }
  done
  if command -v sha256sum >/dev/null 2>&1; then checksum=sha256sum
  elif command -v shasum >/dev/null 2>&1; then checksum=shasum
  else printf '%s\n' 'Install sha256sum or shasum first.' >&2; return 1; fi
  case "$(uname -s)" in Darwin) os=darwin ;; Linux) os=linux ;; *) printf '%s\n' 'Supported systems: macOS and Linux.' >&2; return 1 ;; esac
  case "$(uname -m)" in arm64|aarch64) arch=arm64 ;; x86_64|amd64) arch=x64 ;; *) printf '%s\n' 'Supported architectures: ARM64 and AMD64.' >&2; return 1 ;; esac
  # Prefer the native binary when an Apple Silicon terminal runs under Rosetta.
  if [ "$os" = darwin ] && [ "$(sysctl -n hw.optional.arm64 2>/dev/null || true)" = 1 ]; then arch=arm64; fi
  temporary=$(mktemp -d "${TMPDIR:-/tmp}/apeiron-install.XXXXXXXX")
  staged=
  trap 'rm -rf "$temporary"; if [ -n "$staged" ]; then rm -f "$staged"; fi' 0
  trap 'exit 130' 1 2 3 15
  download() {
    curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' --tlsv1.2 \
      --connect-timeout 15 --max-time 300 --retry 3 --output "$2" "$1"
  }
  if [ -z "$version" ]; then
    download "${base%/}/latest.txt" "$temporary/latest.txt"
    version=$(cat "$temporary/latest.txt")
  fi
  # Reject traversal, shell fragments and unexpected channel metadata.
  if ! printf '%s\n' "$version" | LC_ALL=C awk 'NR != 1 { exit 1 } /^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9]+([.-][A-Za-z0-9]+)*)?$/ { ok=1 } END { if (!ok) exit 1 }'; then
    printf '%s\n' 'Invalid release version.' >&2; return 1
  fi
  archive="apeiron-${version}-${os}-${arch}.tar.gz"
  release="${base%/}/releases/${version}"
  printf 'Downloading Apeiron %s (%s/%s)…\n' "$version" "$os" "$arch"
  download "$release/SHA256SUMS" "$temporary/SHA256SUMS"
  download "$release/$archive" "$temporary/$archive"
  expected=$(awk -v file="$archive" '$2 == file { digest=$1; count++ } END { if (count != 1) exit 1; print digest }' "$temporary/SHA256SUMS") || {
    printf '%s\n' 'Release checksum entry is missing or duplicated.' >&2; return 1;
  }
  case "$expected" in *[!0-9a-f]*|'') printf '%s\n' 'Invalid SHA-256 checksum.' >&2; return 1 ;; esac
  [ "${#expected}" -eq 64 ] || { printf '%s\n' 'Invalid SHA-256 checksum length.' >&2; return 1; }
  if [ "$checksum" = sha256sum ]; then actual=$(sha256sum "$temporary/$archive" | awk '{print $1}')
  else actual=$(shasum -a 256 "$temporary/$archive" | awk '{print $1}'); fi
  [ "$actual" = "$expected" ] || { printf '%s\n' 'Checksum mismatch; existing installation was not changed.' >&2; return 1; }
  # Extract only binary bytes to a chosen path, never paths supplied by the tar.
  [ "$(tar -tzf "$temporary/$archive" | awk '$0=="apeiron" {n++} END {print n+0}')" -eq 1 ] || {
    printf '%s\n' 'Archive must contain exactly one apeiron executable.' >&2; return 1;
  }
  tar -xOzf "$temporary/$archive" apeiron > "$temporary/apeiron"
  chmod 755 "$temporary/apeiron"
  [ "$("$temporary/apeiron" --version)" = "$version" ] || {
    printf '%s\n' 'Downloaded executable cannot run or reports a different version.' >&2; return 1;
  }
  [ ! -d "$install_dir/apeiron" ] || { printf '%s\n' 'Destination apeiron is a directory.' >&2; return 1; }
  mkdir -p "$install_dir"
  staged=$(mktemp "$install_dir/.apeiron.XXXXXXXX")
  cp "$temporary/apeiron" "$staged"
  chmod 755 "$staged"
  mv -f "$staged" "$install_dir/apeiron"
  staged=
  case ":$PATH:" in *":$install_dir:"*) on_path=1 ;; *) on_path=0 ;; esac
  profile=
  if [ "$on_path" = 0 ] && [ "$modify_path" = 1 ] && [ "$install_dir" = "$HOME/.local/bin" ]; then
    case "${SHELL:-}" in
      */zsh) profile="$HOME/.zshrc" ;;
      */bash) profile="$HOME/.bashrc"; [ "$os" != darwin ] || profile="$HOME/.bash_profile" ;;
      */sh|'') profile="$HOME/.profile" ;;
      *) profile= ;;
    esac
    if [ -n "$profile" ] && ! grep -F '# Apeiron CLI PATH' "$profile" >/dev/null 2>&1; then
      printf '\n%s\n%s\n' '# Apeiron CLI PATH' 'export PATH="$HOME/.local/bin:$PATH"' >> "$profile"
      printf 'Added ~/.local/bin to %s for new terminals.\n' "$profile"
    fi
  fi
  printf '\nInstalled: %s/apeiron\n' "$install_dir"
  if [ "$on_path" = 1 ]; then printf '%s\n' 'Start setup: apeiron init'
  else
    printf 'Start setup now: "%s/apeiron" init\n' "$install_dir"
    if [ -n "$profile" ]; then printf '%s\n' 'In a new terminal, use: apeiron init'
    else printf 'Add %s to PATH to use the apeiron command.\n' "$install_dir"; fi
  fi
}

main "$@"
