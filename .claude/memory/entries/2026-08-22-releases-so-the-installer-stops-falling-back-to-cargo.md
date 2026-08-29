# Releases So The Installer Stops Falling Back To Cargo — 2026-08-22

**Asked:** "no prebuilt binary for this platform yet — falling back to cargo... fix. also update readme to
walk user how to use this"

**Did:** Cut real GitHub releases with binaries (`bcaa104`, then `4ddcc7e` v0.1.1, `0c178e2` v0.1.2) so
`install.sh` finds an artifact instead of building from source.

**Gotchas:**
- Two `gh` accounts are logged in. `webdevcody` is the admin; `codyseibert` has only READ on
  `AgentSystemLabs/nebula` and fails write calls with "must be a collaborator (createPullRequest)".
  **As of 2026-08-24 `webdevcody` is the active account** (it was `codyseibert` on 08-22, so check
  rather than assume): `gh auth status`, and `gh auth switch --hostname github.com --user webdevcody`
  if it has drifted back. `git push` is unaffected either way: it goes over SSH, not the gh token.
