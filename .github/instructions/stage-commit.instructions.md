---
description: "Use at the end of any task that changed files. Stage the relevant changes and prepare a draft commit message WITHOUT running git commit."
applyTo: "**"
---
# Prepare a Staged Commit at End of Task

When you finish a task that created, modified, or deleted files, prepare the change as a staged commit. **Stage only — never commit.**

## Do

- Stage only the files that belong to the completed task with `git add <paths>`. Add files explicitly by path; avoid `git add -A` / `git add .`.
- Write a draft commit message to `.git/COMMIT_EDITMSG` so it is ready for the user to commit:
  ```bash
  git diff --cached --stat   # confirm what is staged
  printf '%s\n' "<message>" > "$(git rev-parse --git-dir)/COMMIT_EDITMSG"
  ```
- Use Conventional Commits for the message: `type(scope): summary` (e.g. `feat(ui): add capsule slider`), with a short body explaining the why when useful.
- After staging, report a one-line summary of what was staged and show the prepared message.

## Do not

- Do **not** run `git commit`, `git commit -m`, `git commit --amend`, or any command that creates a commit.
- Do **not** stage unrelated or pre-existing changes that are not part of this task.
- Do **not** run `git push`, `git reset --hard`, or other destructive/remote git operations.
- Do **not** stage build artifacts, secrets, or files normally ignored (respect `.gitignore`).

## Leave the actual commit to the user

The user reviews the staged change and the prepared message, then commits themselves.
