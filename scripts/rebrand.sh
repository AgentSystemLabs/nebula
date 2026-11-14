#!/bin/sh
# Rebrand the upstream nebula tree to orbit: paths, identifiers, env vars,
# data dirs, managed-hook markers, release asset names and install URLs.
#
# Idempotent — a tree with nothing named nebula left is untouched — so it is
# also how upstream changes come in:
#
#   git fetch upstream
#   git checkout -b sync-upstream upstream/main
#   scripts/rebrand.sh && git commit -am "Rebrand upstream to orbit"
#   git checkout main && git merge sync-upstream
#
# LICENSE is never rewritten: it carries the upstream copyright notice, which
# the MIT license requires us to keep. This script exempts itself too.
set -eu
cd "$(git rev-parse --show-toplevel)"

# 1. Paths. Crate directories first, then any tracked file whose own name
#    still says nebula (the index already reflects the directory moves).
for d in crates/nebula*; do
    [ -d "$d" ] || continue
    git mv "$d" "$(printf '%s' "$d" | sed 's/nebula/orbit/')"
done
git ls-files | grep 'nebula' | grep -v '^scripts/rebrand\.sh$' | while IFS= read -r f; do
    base=$(basename "$f")
    case "$base" in *nebula*) ;; *) continue ;; esac
    git mv "$f" "$(dirname "$f")/$(printf '%s' "$base" | sed 's/nebula/orbit/g')"
done

# 2. Contents of every tracked text file that mentions nebula.
git ls-files | grep -v -e '^LICENSE$' -e '^scripts/rebrand\.sh$' | while IFS= read -r f; do
    [ -f "$f" ] && [ ! -L "$f" ] || continue
    grep -Iqi 'nebula' "$f" 2>/dev/null || continue
    # A line carrying `rebrand:keep` (the README's credit to upstream) is
    # left exactly as it is.
    perl -pi -e '
        next if /rebrand:keep/;
        s{AgentSystemLabs/nebula}{AmenAdem/Orbit}g;
        s/nebula/orbit/g;
        s/Nebula/Orbit/g;
        s/NEBULA/ORBIT/g;
    ' "$f"
done

# 3. "orbit" is a character shorter than "nebula", so some wrapped lines now
#    fit on one: let rustfmt rejoin them or `make ci`'s format check fails.
if command -v cargo >/dev/null 2>&1; then
    cargo fmt --all
else
    echo "warning: cargo not found — run \`cargo fmt --all\` before \`make ci\`" >&2
fi

left=$(git grep -Ii nebula -- ':!LICENSE' ':!scripts/rebrand.sh' | grep -v 'rebrand:keep' | cut -d: -f1 | sort -u | wc -l | tr -d ' ')
echo "rebrand done; files still mentioning nebula: $left"
