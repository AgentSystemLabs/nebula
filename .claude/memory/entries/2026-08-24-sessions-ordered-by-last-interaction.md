# Sessions Ordered By Last Interaction — 2026-08-24

**Asked:** "order the sessions by last interaction date, also display a time last interacted next to the
session title to right but left of harness name, so the workflow is a session runs goes to top of list,
if anything else iteracts it would go top. when displaying the last interaction time just show '23m ago…'"
Follow-up: "commit and push, then release with good change log with detials on what changed, make release
skill when done to follow these steps." Related earlier ask (2c58d9c1): running / awaiting-feedback
sessions always pin to the top of the Recent list.

**Did:** Sessions sort by last-interaction timestamp (RECENCY ORDER) with a relative age label (the AGO BADGE); released as `c340baf`
(v0.2.0).
