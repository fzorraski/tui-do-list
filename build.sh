#!/usr/bin/env bash
# Build script for tui-do-list.
#
#   ./build.sh            check + lint + test + release build
#   ./build.sh install    all of the above, then `cargo install` and verify PATH
#   ./build.sh quick      release build only (skip lint and tests)
#   ./build.sh clean      remove the target/ directory
#   ./build.sh install-timer   enable the systemd user timer for desktop reminders
#
# Every step stops the script on failure.

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")"

BIN_NAME="tui-do-list"

# --- helpers -----------------------------------------------------------------

if [[ -t 1 ]]; then
    bold=$'\e[1m'; green=$'\e[32m'; yellow=$'\e[33m'; red=$'\e[31m'; reset=$'\e[0m'
else
    bold=""; green=""; yellow=""; red=""; reset=""
fi

step() { printf '\n%s==> %s%s\n' "$bold" "$*" "$reset"; }
ok()   { printf '%s✔ %s%s\n' "$green" "$*" "$reset"; }
warn() { printf '%s! %s%s\n' "$yellow" "$*" "$reset"; }
die()  { printf '%s✘ %s%s\n' "$red" "$*" "$reset" >&2; exit 1; }

require() {
    command -v "$1" >/dev/null 2>&1 || die "'$1' not found. Install Rust from https://rustup.rs"
}

# --- steps -------------------------------------------------------------------

do_fmt() {
    step "Checking formatting (cargo fmt)"
    if cargo fmt --version >/dev/null 2>&1; then
        cargo fmt --all -- --check && ok "formatting OK"
    else
        warn "rustfmt not installed, skipping (rustup component add rustfmt)"
    fi
}

do_clippy() {
    step "Linting (cargo clippy)"
    if cargo clippy --version >/dev/null 2>&1; then
        cargo clippy --all-targets -- -D warnings && ok "no lint warnings"
    else
        warn "clippy not installed, skipping (rustup component add clippy)"
    fi
}

do_test() {
    step "Running tests (cargo test)"
    cargo test && ok "tests passed"
}

do_build() {
    step "Building release binary (cargo build --release)"
    cargo build --release
    ok "binary: target/release/$BIN_NAME ($(du -h "target/release/$BIN_NAME" | cut -f1))"
}

do_install() {
    step "Installing (cargo install --path .)"
    cargo install --path . --locked
    local bin_dir="${CARGO_HOME:-$HOME/.cargo}/bin"
    ok "installed to $bin_dir/$BIN_NAME"

    if command -v "$BIN_NAME" >/dev/null 2>&1; then
        ok "'$BIN_NAME' is on your PATH — run it with: $BIN_NAME"
    else
        warn "'$bin_dir' is not on your PATH, so '$BIN_NAME' won't be found."
        warn "Add this line to ~/.bashrc (or ~/.zshrc) and open a new terminal:"
        printf '\n    export PATH="%s:$PATH"\n\n' '$HOME/.cargo/bin'
        warn "Until then you can run it as: $bin_dir/$BIN_NAME"
    fi
}

do_clean() {
    step "Cleaning"
    cargo clean && ok "target/ removed"
}

do_install_timer() {
    step "Installing systemd user timer for desktop reminders"
    command -v systemctl >/dev/null 2>&1 || die "systemctl not found — this needs systemd"
    command -v notify-send >/dev/null 2>&1 || warn "notify-send not found; install libnotify or no notification will appear"
    command -v "$BIN_NAME" >/dev/null 2>&1 || warn "'$BIN_NAME' is not on your PATH; the unit uses ~/.cargo/bin/$BIN_NAME directly"
    local unit_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
    mkdir -p "$unit_dir"
    cp contrib/tui-do-list-notify.service contrib/tui-do-list-notify.timer "$unit_dir/"
    systemctl --user daemon-reload
    systemctl --user enable --now tui-do-list-notify.timer
    ok "timer enabled: $(systemctl --user is-active tui-do-list-notify.timer) (runs '$BIN_NAME notify' every minute)"
    warn "To remove it: systemctl --user disable --now tui-do-list-notify.timer"
}

# --- main --------------------------------------------------------------------

require cargo

case "${1:-}" in
    "")
        do_fmt; do_clippy; do_test; do_build
        ;;
    install)
        do_fmt; do_clippy; do_test; do_build; do_install
        ;;
    quick)
        do_build
        ;;
    clean)
        do_clean
        ;;
    install-timer)
        do_install_timer
        ;;
    -h|--help|help)
        sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'
        exit 0
        ;;
    *)
        die "unknown command '$1' (try: ./build.sh --help)"
        ;;
esac

printf '\n%sDone.%s\n' "$green" "$reset"
