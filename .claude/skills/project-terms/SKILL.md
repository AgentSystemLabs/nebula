---
name: project-terms
description: "Keep TERMS.md — nebula's shared glossary of ALL-CAPS canonical names for its features, panels, keys, CLI commands, hook routes, daemon mechanisms, statuses and dev workflows — true after a task. Use at the end of every task, right after nebula-memory. Every run detects the vocabulary the task surfaced and runs terms_check.py to prune: only a concept that has recurred across separate tasks is promoted to a TERM, the rest wait in the Candidates ledger; dead TERMS, merge pairs, stale pointers and overdue candidates are merged, retired or pruned with --merge / --retire / --prune. Aliases the user typed for an existing TERM, renames, and retirements land immediately. Also use when the user says \"add this to terms\", \"what do we call this\", \"name this\", \"promote this\", or \"update the glossary\"."
user-invocable: true
---

Nebula keeps a shared, committed glossary at **`TERMS.md`** (repo root). Every agent reads it before
starting and speaks in its terms — see `AGENTS.md`. Your job here is to keep it true after the task you
just finished, so the next agent and the next teammate call things by the same name you did.

The MEMORY LOG records *what happened*; `TERMS.md` records *what things are called*. Do not put a work log
in the glossary, and do not put definitions in the work log.

## Detect every session, promote only what recurred

A TERM is a name the team actually uses. A name that appeared once — coined in one reply, one commit, one
session's design — is a guess at a name, and a glossary that takes every guess fills up with rows nobody
says twice, stops being scannable, and stops being trusted. So this skill runs in two tiers:

- **Sightings** — every task, notice the vocabulary it surfaced: a thing that got a name, a word the
  user typed for a thing, a name you used in a reply, a commit, or a MEMORY LOG entry. Sightings are
  cheap and you record all of them.
- **Promotion** — a concept becomes a TERM (a row in a numbered section) only when it has been used in
  **more than one separate task**. Until then it waits in the **Candidates** ledger at the bottom of
  `TERMS.md`, with where it was seen.

"Separate task" means a different MEMORY LOG entry, a different session, or a different commit from the
one that first sighted it. You coining a name in your reply and then writing it in your own commit is
*one* sighting. The user typing it in a later prompt, a later task's memory entry using it, or a later
commit message using it is the second — and that is what promotes it.

Two shortcuts, both the user's call, not yours: if the user says **"name this"**, **"add this to
terms"**, or **"promote X"**, promote on the spot; and if the user *names* a thing in their prompt that
has no TERM, that is still only a first sighting — the TERM comes when they, or a later task, use it
again.

## What lands immediately

These are not gated, because they are not guesses:

- **The user used a word for an existing TERM that its row does not list.** Their prompt said "top nav"
  and they meant the WORKSPACES BAR; "locked layer" and they meant the WALK EDGE. Add the phrase,
  verbatim, to that TERM's *Also called* cell and to the **Alias index**. The user's own word for a
  thing that already has a name is canonical the moment they type it. This is the highest-value edit the
  glossary gets: it is how the next agent reads the next prompt correctly on the first try.
- **A word turned out to be ambiguous.** One alias, two TERMS ("done" → UNSEEN vs FINISHED). Record the
  split under both rows and in the Alias index, so the ambiguity is named before it costs a turn again.
- **A TERM changed its name or meaning.** A status was renamed, a badge's wording changed, a key moved,
  a mechanism was superseded. Edit the existing row in place — never leave the old definition standing
  next to the new one.
- **A TERM's thing was removed.** `terms_check.py --retire A [--into B]` moves its row to **Retired**
  with the date and what replaced it (and repoints every reference), so old prompts and old MEMORY LOG
  entries stay readable.
- **A *Where* cell went stale.** The symbol moved or was renamed; fix the pointer.

## What waits: new TERMS

A thing that got a name this task — a new panel, overlay, badge, key, CLI subcommand, flag, hook route,
daemon mechanism, config key, env var, Makefile target, test harness, or skill that someone will refer to
out loud — goes to the **Candidates** ledger, not to a numbered section. One row:

```markdown
| **CANDIDATE NAME** | What it seems to be — one sentence. | 2026-08-28 prompt · 2026-08-28 MEMORY "Entry Title" | `file.rs::symbol` |
```

- The candidate name is ALL CAPS, the same shape a TERM would have. Prefer the name the code, README, or
  the user already used; invent only when none of them has one.
- **Seen** lists every sighting as `date source` — `prompt`, `commit abc1234`, `MEMORY "Entry Title"`,
  `README`, `reply` — so the next run can tell whether a new sighting is a separate task or the same one.
- **Where** is the greppable pointer, verified, same as a TERM's.

Do **not** ledger: internal helpers nobody will say out loud (`fn row_badges`), a restated diff, a name
already present as a TERM or a candidate under a different spelling (grep first), or anything `README.md`
already defines and nobody has ever misnamed.

## Promotion and pruning

Every run, walk the ledger once:

1. **Add sightings.** For each candidate, check whether *this* task used it — in the user's prompt, in
   the memory entry you just wrote, in a commit you made — and append the sighting. `git log --oneline
   --since=<first sighting date> --grep='<candidate>' -i` catches commits from other sessions.
2. **Promote** any candidate whose *Seen* now holds sightings from two or more separate tasks. Write the
   full row in the section it belongs to (format below), carry the candidate's aliases into *Also
   called* and the **Alias index**, and delete the ledger row.
3. **Prune** any candidate whose only sighting is older than 30 days, or whose thing has been removed —
   `terms_check.py --prune` does the first (it knows today's date); delete the second by hand. Do not
   retire a candidate; it was never a TERM.

If a candidate's thing was already removed before it ever recurred, that is the system working: a name
that never got a second use was not vocabulary.

## Prune and merge with `terms_check.py`

`python3 .claude/memory/terms_check.py` (`make terms-check`, part of `make ci`) reads the glossary
against everything that speaks it — the MEMORY LOG, `CLAUDE.md`, README, the skills, code comments —
and reports what to act on. Run it every time this skill runs, after the sightings, and **act on the
report yourself; do not just relay it.** Each line has a rule:

| Report line | Rule | Do |
|---|---|---|
| `stale` — a `Where` pointer no longer greps | fix now, always | grep the symbol's new home and edit the cell |
| `dangling` — an Alias-index target names no row | fix now, always | point it at the row it means, or delete the alias row |
| `overdue` — a candidate past 30 days · `unmentioned` — a Retired row nothing in `.claude/memory/` says | delete, always | `terms_check.py --prune` |
| `dead` — never written in caps outside `TERMS.md` | retire if its thing is gone or its `Where` is stale; merge if another row already says what it says; **otherwise leave it** — a live feature nobody has discussed yet is not noise | `--retire A` · `--merge A --into B` |
| `once` | same rule as `dead`, weaker evidence — act only when the merge target is obvious | `--merge A --into B` |
| `merge?` — A is only ever said on lines that also say B | merge when B's row already covers A's meaning, or A is B's internal state / data (PENDING ACTION → CONFIRM DIALOG, METRICS SNAPSHOT → MEMORY MODAL); keep two rows when the user could mean one without the other (CHORD vs KEYMAP, HANDSHAKE vs VERSION SKEW) | `--merge A --into B` |
| `collide` — an alias shared by 3+ TERMS | keep only a deliberate ambiguity ("done", the status colors); otherwise leave the alias on the one TERM the user meant and remove it from the others | edit the rows and the Alias index |

`--merge A --into B` and `--retire A [--into B]` move A's row to Retired, make A's name and aliases
aliases of B, and rewrite every reference — the Alias index, other rows' cross-references, the MEMORY
LOG index cells, the `**A**` keys in `gotchas.md` — then run RECALL EVAL and refuse the change if
retrieval dropped. A merge prints A's meaning: fold it into B's row by hand if B does not already say
it, and keep B to two sentences. `--dry-run` shows the plan first; `git checkout TERMS.md
.claude/MEMORY.md .claude/memory/gotchas.md` undoes a write. Entry files are never rewritten — a
Retired row is what keeps the old name readable in them.

Merges and retirements are glossary edits like any other: name them in the one-line confirmation
(step 6), and note why in the MEMORY LOG entry when the reason was a judgment call.

## The format of a TERM

Every TERM is one row in a numbered section table:

```markdown
| **TERM** | What it is — one or two sentences, present tense, concrete. | "alias", "another alias" | `file.rs::symbol` · `key` · `nebula sub` |
```

Rules for a row:

- **The TERM is ALL CAPS**, a short noun phrase, and the same everywhere it appears — in this file, in
  `CLAUDE.md`, in replies, in memory entries. `PANEL WALK`, never "panel walk" or "the Panel Walk".
  Prefer the name the code or README already uses; invent only when neither has one.
- **One TERM per thing, one thing per TERM.** If two names exist for the same thing, one is the TERM and
  the other goes in *Also called*.
- **Meaning is definitional, not historical.** "The daemon-owned flag that marks a finished turn nobody
  has looked at" — not "added on 2026-08-26 because the user asked for counters." History lives in
  the MEMORY LOG; link to it by entry title if the why matters.
- **Aliases are verbatim.** Quote the user's words exactly as typed, lower-case, in double quotes,
  comma-separated. An alias that is just the TERM in different casing is not worth a slot.
- **Where it lives is greppable.** A path and symbol (`crates/nebula-tui/src/ui.rs::status_dot`), a
  default key (`Shift+M`), a subcommand (`nebula tunnel`), a route (`/api/hooks/stop`), a config key
  (`"prewarm_agents"`), an env var (`NEBULA_CLOUD_MIRROR_SECS`). Verify it exists before you write it.
- **Cross-reference other TERMS in caps.** "Stops at the TERMINAL PANE going forward; see LOCKED PANE."
  A reader should be able to follow the caps from any row to every row it depends on. A candidate is
  not a TERM: do not cross-reference it in caps from a TERM row until it is promoted.

Put the row in the section it belongs to (the section headers are fixed — do not add sections without a
reason that will survive review), keep each section's rows in the order that reads best (usually the
order a user meets them, not alphabetical), and add or update the row in the **Alias index** whenever
*Also called* changed.

## Steps

1. **Collect this task's sightings.** The nouns in the user's prompt (the **Asked** line you just wrote
   in the MEMORY LOG), the names in your **Did** and **Gotchas** lines, your commit messages, and any
   symbol, key, command, route, or env var the task added.
2. **Sort each one** with `grep -n -i '<the thing>' TERMS.md`:
   - it is a TERM → is the user's word for it already in *Also called*? If not, add the alias (immediate).
   - it is a candidate → append the sighting; promote if this is a separate task.
   - it is neither, and someone will say it out loud → add a candidate row.
   - the task renamed, removed, or moved it → edit the row, retire it, or fix *Where* (immediate).
3. **Walk the ledger** — sightings, promotions, pruning — per the section above. Get today's date with
   `date +%F`; do not guess it.
4. **Run `python3 .claude/memory/terms_check.py`** and act on every line per "Prune and merge with
   `terms_check.py`": `--prune` what is overdue or unmentioned, fix what is stale or dangling, and
   `--merge` / `--retire` what the rules say — leave the rest.
5. **Update the Alias index** for every alias you added or promoted.
6. **Reread the rows you touched** once, as the next agent would: can they tell what it is, what the
   user calls it, and where to grep, from that row alone?
7. **Confirm to the user in one line**, in the TERMS themselves: TERMS promoted / changed / retired /
   merged (by name), candidates added or pruned (by name), aliases recorded — or "no glossary changes"
   when nothing moved.
   Most runs record a sighting or an alias and promote nothing; that is the expected outcome, not a
   failure to find something.

## Size discipline

`TERMS.md` is read at the start of every session alongside `.claude/MEMORY.md` and
`.claude/memory/gotchas.md`, so it has to stay scannable.
Keep meanings to two sentences. When a section grows past what fits on one screen, tighten rows before
adding more — `terms_check.py` names the merge pairs (`merge?`) and the TERMS nobody says (`dead`,
`once`); `--merge` folds one into the other. Retired TERMS get one line each and `--prune` drops them
once nothing under `.claude/memory/` mentions them. The Candidates ledger should stay under about twenty
rows: if it is longer, you are sighting helpers, not vocabulary — prune harder and ledger less.
