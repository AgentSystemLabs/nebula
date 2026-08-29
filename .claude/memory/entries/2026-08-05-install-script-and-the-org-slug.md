# Install Script And The Org Slug — 2026-08-05

**Asked:** "if I wanted to provide one command for anyone to install or update this cli tool, what's the
best way? a .sh script in the repo? I don't want to use some third party registery at this point" →
"do the curl approach and put in the readme" → "why did you make the readme say webdevcody,,, this is part
of the agentsystemlabs org"

**Did:** `install.sh` + README one-liner (`95ac3da`), then `nebula upgrade` (`1c87c06`).

**Gotchas:**
- The repo slug is **`AgentSystemLabs/nebula`**, never `webdevcody/<repo>`. It is hardcoded in
  `install.sh` (`REPO=`) and the README. Assume other repos under `~/Workspace/AgentSystemLabs/` are
  org repos too.
