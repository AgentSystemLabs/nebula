# NEBULA TUNNEL "Asks For A Password" Is ssh Login Auth, Not Nebula — 2026-08-28

**Asked:** "when I run ssh tunnel, it seems to require a password? why is this? isn't a tunnel secure enough
where I wouldn't need a password" → picked: Which password, and why — explain which layer prompts (ssh
login / key passphrase / ttyd) and what setup stops it; change nothing.

**Did:** Diagnosis only. NEBULA TUNNEL (`crates/nebula/src/tunnel.rs::spawn_ssh`) runs a plain
`ssh -tt -o ExitOnForwardFailure=yes -L 127.0.0.1:L:127.0.0.1:R -- HOST 'sh -c …'` and its remote tail is
`nebula browser --no-open --port R` with **no** `--credential`, so ttyd never asks and nebula supplies no
password of its own. The prompt is OpenSSH's `user@host's password:` — publickey was refused by the remote
and sshd fell back to password auth. The tunnel's security *is* that authentication; the forward only opens
after it, so "the tunnel is secure" is the reason ssh insists on proving who you are, not a reason to skip it.
Fix is on the ssh side: put the local pubkey in the remote's `~/.ssh/authorized_keys` (`ssh-copy-id`) or
name the right key in `~/.ssh/config` (`IdentityFile`); `ssh -o BatchMode=yes HOST true` printing nothing is
the check, and NEBULA TUNNEL / NEBULA SSH pass HOST verbatim so config aliases apply.

**Gotchas:**
- On this Mac all three keys (`id_ed25519`, `id_rsa`, `stn_ed25519`) are passphrase-free, there is no
  `~/.ssh/config`, and the agent is empty — so ssh only offers the default-named keys. A box whose
  `authorized_keys` holds `stn_ed25519.pub` (or a cloud `.pem`) gets password auth every time until an
  `IdentityFile` line names it.
- Could not verify live: both hosts in `known_hosts` (`107.21.158.162`, `10.0.0.213`) timed out /
  "Host is down" on 2026-08-28, and no `ssh_hosts.json` exists in the DATA DIR or `~/.nebula-dev`.
- ssh writes the password prompt to `/dev/tty`, so it shows even though NEBULA TUNNEL pipes ssh's stderr
  through `forward_ssh_stderr`.
- Follow-up: the user reported plain `ssh` to the same box works with a key, and asked whether the NEBULA
  BROWSER `--bind`/`--public` work (commit `8698abf`) could be the cause. It cannot: those flags only widen
  the bind and print a warning (`browser.rs::warn_if_exposed`), the tunnel's remote tail never passes them,
  and nothing in any crate prompts for a password (`grep -rni password crates/*/src` → only the clap
  `--credential` value name). Left open until the user reports the exact command and prompt text; the
  discriminator is the tunnel's own ssh line with an `echo` tail (see the reply of 2026-08-28).
