#!/bin/sh
# orbit installer — installs or updates the `orbit` binary.
#
#   curl -fsSL https://raw.githubusercontent.com/AmenAdem/Orbit/main/install.sh | sh
#
# Grabs the prebuilt binary for this platform from the latest GitHub release,
# falling back to `cargo install --git` when no release (or no matching asset)
# exists. Running it again updates in place.
#
# Environment overrides:
#   ORBIT_INSTALL_DIR   install destination (default: ~/.local/bin)
set -eu

REPO="AmenAdem/Orbit"
INSTALL_DIR="${ORBIT_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
err() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

detect_target() {
    case "$(uname -s)" in
    Darwin)
        case "$(uname -m)" in
        arm64) echo aarch64-apple-darwin ;;
        x86_64) echo x86_64-apple-darwin ;;
        *) return 1 ;;
        esac
        ;;
    Linux)
        case "$(uname -m)" in
        x86_64) echo x86_64-unknown-linux-musl ;;
        aarch64 | arm64) echo aarch64-unknown-linux-musl ;;
        *) return 1 ;;
        esac
        ;;
    *) err "orbit runs on macOS and Linux only (the daemon needs unix sockets)" ;;
    esac
}

install_from_release() {
    url="https://github.com/$REPO/releases/latest/download/orbit-$1.tar.gz"
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    say "downloading $url"
    curl -fsSL "$url" -o "$tmp/orbit.tar.gz" || return 1
    tar -xzf "$tmp/orbit.tar.gz" -C "$tmp"
    mkdir -p "$INSTALL_DIR"
    install -m 755 "$tmp/orbit" "$INSTALL_DIR/orbit"
}

install_from_source() {
    command -v cargo >/dev/null 2>&1 ||
        err "no prebuilt binary available and cargo is not installed — get Rust from https://rustup.rs and re-run"
    say "building from source (this takes a few minutes)…"
    cargo install --git "https://github.com/$REPO" orbit --locked --force
}

main() {
    command -v curl >/dev/null 2>&1 || err "curl is required"

    installed=""
    if target=$(detect_target); then
        if install_from_release "$target"; then
            installed="$INSTALL_DIR/orbit"
        else
            reason="couldn't download orbit-$target from the latest release"
        fi
    else
        reason="no prebuilt binary for $(uname -s) $(uname -m)"
    fi

    if [ -z "$installed" ]; then
        say "$reason — building from source instead"
        install_from_source
        installed="$HOME/.cargo/bin/orbit"
    fi

    version=$("$installed" --version 2>/dev/null || echo orbit)
    say "installed $version → $installed"

    bin_dir=$(dirname "$installed")
    case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) say "note: $bin_dir is not on your PATH — add: export PATH=\"$bin_dir:\$PATH\"" ;;
    esac

    # Replacing the file doesn't touch an already-running daemon: sessions keep
    # running on the old binary until it's restarted. `orbit reload` restarts
    # it in place, sessions and all; a daemon from before reload existed can
    # only be restarted by `orbit kill`, which stops every session. `orbit
    # upgrade` handles this itself and suppresses the note via
    # ORBIT_UPGRADE_HANDOFF.
    if [ -z "${ORBIT_UPGRADE_HANDOFF:-}" ] && pgrep -f 'orbit daemon' >/dev/null 2>&1; then
        say "note: a orbit daemon from the previous version is still running."
        say "      run 'orbit reload' to move it onto the new one (sessions keep running);"
        say "      a daemon too old for that needs 'orbit kill' (stops all sessions)."
    fi
}

main
