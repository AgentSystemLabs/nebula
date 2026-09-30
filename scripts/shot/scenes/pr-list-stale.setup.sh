# The PULL REQUESTS MODAL on a list GitHub stopped answering for (#106): the fixtures are
# pr-row-conflicts's — #42 conflicting, #37 with a failing check, #39 a draft — but the stand-in gh
# answers the open list once and fails every later ask the way GitHub's 504 did. The first answer
# lands at boot; the modal's Ctrl+r asks again and fails, so the rows are the last list that worked.
. "$HERE/scenes/pr-row-conflicts.setup.sh"
echo 1 > "$FIXTURES/pr-list.ok-count"
