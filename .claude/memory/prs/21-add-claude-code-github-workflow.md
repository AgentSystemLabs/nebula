# PR #21 — Add Claude Code GitHub Workflow

- **URL:** https://github.com/AgentSystemLabs/nebula/pull/21
- **Author:** @webdevcody
- **Merged:** 2026-08-29T01:20:15Z by @webdevcody (`8de245bc675d`)
- **Opened:** 2026-08-29T01:20:08Z
- **Branch:** `add-claude-github-actions-1787966398621` → `main`
- **Diff:** +95 −0 across 2 file(s)

## Description

> ## 🤖 Installing Claude Code GitHub App
>
> This PR adds a GitHub Actions workflow that enables Claude Code integration in our repository.
>
> ### What is Claude Code?
>
> [Claude Code](https://claude.com/claude-code) is an AI coding agent that can help with:
> - Bug fixes and improvements  
> - Documentation updates
> - Implementing new features
> - Code reviews and suggestions
> - Writing tests
> - And more!
>
> ### How it works
>
> Once this PR is merged, we'll be able to interact with Claude by mentioning @claude in a pull request or issue comment.
> Once the workflow is triggered, Claude will analyze the comment and surrounding context, and execute on the request in a GitHub action.
>
> ### Important Notes
>
> - **This workflow won't take effect until this PR is merged**
> - **@claude mentions won't work until after the merge is complete**
> - The workflow runs automatically whenever Claude is mentioned in PR or issue comments
> - Claude gets access to the entire PR or issue context including files, diffs, and previous comments
>
> ### Security
>
> - Our Anthropic API key is securely stored as a GitHub Actions secret
> - Only users with write access to the repository can trigger the workflow
> - All Claude runs are stored in the GitHub Actions run history
> - Claude's default tools are limited to reading/writing files and interacting with our repo by creating comments, branches, and commits.
> - We can add more allowed tools by adding them to the workflow file like:
>
> ```
> allowed_tools: Bash(npm install),Bash(npm run build),Bash(npm run lint),Bash(npm run test)
> ```
>
> There's more information in the [Claude Code action repo](https://github.com/anthropics/claude-code-action).
>
> After merging this PR, let's try mentioning @claude in a comment on any PR to get started!

## Changed files (2)

- `.github/workflows/claude-code-review.yml` +45 −0
- `.github/workflows/claude.yml` +50 −0

## Commits (2)

- `3d37e3a08e83` "Claude PR Assistant workflow" — @webdevcody
- `c8cfba84e950` "Claude Code Review workflow" — @webdevcody

## Conversation (0)

_(no issue comments)_

## Reviews (0)

_(no review submissions)_

## Inline review comments (0)

_(no inline comments)_
