---
description: Cut a release tag (v<N>) on main and push it plus main to GitHub, triggering the release pipeline
---

Cut a release for the jed editor. Versions are simple incrementing integers (not major.minor.patch); the release tag for version N is `vN`.

The OpenCode twin of this command is `.opencode/commands/release.md`; a change to the steps belongs in both.

Remote release tags right now:

!`git ls-remote --tags github 'v*'`

Do these steps in order, all against the `github` remote:

1. **Work out the next version.** Find the highest existing `v<number>` release tag (from the list above, and re-check locally after `git fetch github --tags`). The next tag is `v<number+1>`. If there are no `v*` tags yet, this is the first release and the next tag is `v1`.
2. **State the tag.** Announce the tag you are about to create and the commit it points at (`git rev-parse HEAD`), then proceed.
3. **Push main, then wait for CI.** If local `main` is ahead of `github/main` (`git rev-list --count github/main..main` is non-zero), push it — `git push github main` (a no-op when already up to date). Then wait for the `Build` workflow on that commit to finish and pass with `scripts/wait-ci`, which exits non-zero if it fails or never starts. If it fails, stop and report the failing run — do not tag.
4. **Write the changelog and tag main's HEAD.** Generate the changelog — the tasks completed since the previous release plus every commit since it — and put it in the annotated tag message. Use `scripts/changelog <previous-tag>` with the highest existing tag from step 1; for the first release (no `v*` tags) run `scripts/changelog` with no argument to include the full history. For example:
   ```
   notes=$(mktemp)
   { echo "Release v<N>"; echo; scripts/changelog v<N-1>; } > "$notes"
   git tag -a v<N> -F "$notes"
   ```
   Do not overwrite a tag that already exists; if `v<N>` is taken, stop and report.
5. **Push the tag.** `git push github v<N>`. The tag push is what triggers the release pipeline.
6. **Report.** Confirm the tag and pushed refs with `git ls-remote --tags github 'v*'`.
