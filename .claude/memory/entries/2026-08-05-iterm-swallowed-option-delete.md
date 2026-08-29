# iTerm Swallowed Option+Delete — 2026-08-05

**Asked:** "when I have a session focused, option + delete doesn't seem to work to backspace by words when
I have nebula opened in iterm, fix"

**Did:** Fixed outside the codebase — set left Option → Esc+ in iTerm's Default profile.

**Gotchas:**
- iTerm2 3.5.10 in kitty mode only reports Option as the alt modifier when the profile's Option key is
  **Esc+** (`Option Key Sends` = 2). With "Normal" (the user's old setting) Option+Delete arrives as a
  plain Backspace and word-delete silently breaks.
- iTerm must **not** be running when editing its plist or it clobbers the write on quit. Its quit-confirm
  dialog can't be dismissed via osascript without accessibility permission — SIGTERM works and skips the
  pref flush.
