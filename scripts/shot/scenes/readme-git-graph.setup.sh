# The README's git graph: the hero's projects, with a little history on orbit-api's branches.
. "$HERE/scenes/readme-grid.setup.sh"
c() { # c <dir> <author> <message>
  echo "$3" >> "$1/HISTORY"; git -C "$1" add -A
  git -C "$1" -c commit.gpgsign=false -c user.name="$2" -c user.email=dev@example.invalid commit -q -m "$3"
}
API="$WORK/orbit-api"
c "$API" "Sam Rivera" "Add request logging middleware"
c "$API" "Noor Haddad" "Bump tokio to 1.48"
c "$FEAT" "Lea Martin" "Store tokens in sqlite instead of a JSON file"
c "$FEAT" "Lea Martin" "Index tokens by expires_at"
c "$WORK/orbit-api-worktrees/fix-rate-limits" "Sam Rivera" "Rate limit the public endpoints"
c "$API" "Noor Haddad" "Release v0.9.0"
git -C "$API" -c tag.gpgsign=false -c commit.gpgsign=false tag v0.9.0
c "$FEAT" "Lea Martin" "Refresh tokens before they expire"
