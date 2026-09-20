//! The fzf-style matcher behind every list filter — the FILE FINDER, the
//! TREE BROWSER, the DIFF VIEWER's file list, the `/` PALETTE, the BRANCH
//! SWITCHER. It lives in the `nebula-fuzzy` crate so that a dev build can
//! compile it optimised: it is the one loop in the TUI that runs over every
//! path of a checkout on every keystroke, and at this crate's opt-level 0
//! that was 13 to 27 ms a character over ten thousand paths.
pub use nebula_fuzzy::*;
