---
id: review_links
name: Code review links and verdicts
summary: open or copy the forge's link of a merge request, thread or comment, withdraw your approval, and tag threads the forge calls outdated
status: open
priority: 0
dependencies: [mr_review, review_fixes]
---

Three small things the forge data already allows, in one commit.

  - Needs: `jed/src/url.rs` `open` (blocking: `spawn_blocking`; records the URL under test), the clipboard (`App::clipboard`), the panel's context menu (`app/context_menu.rs` `ReviewSpot`), `forge::{MergeRequest::url, Comment::url, Thread::outdated, Approval}`, `Forge` and its adapters, `review_write::Job`, `ui/review.rs` (the CONVERSATIONS rows' tags), `review_blocks.rs` (the thread header)
  - Links: "Code Review: Open on Forge" (`review.openOnForge`) and "Code Review: Copy Link" (`review.copyLink`), unbound, in the palette and the panel's and the thread block's context menus. The target is the selected CONVERSATIONS row's thread (its first comment's `url`) or comment, else the thread at the caret in a review diff or thread tab, else the merge request (`mr.url`). Only an `https://` URL (or `http://` when the host's `forges.toml` entry allows http) whose host is the review's configured host is opened: anything else is refused with "The forge sent no usable link" (the URL is remote text). An empty URL (GitLab without `web_url`) says the same.
  - Withdrawing a verdict: "Code Review: Withdraw My Review" (`review.withdraw`), shown when the approval line names the user. Check each forge's API first and record it in [mr_review](mr_review.md): GitLab `POST …/merge_requests/{iid}/unapprove`; GitHub and Gitea dismiss a review by id (`PUT` / `POST …/pulls/{n}/reviews/{id}/dismissals`, with a message; who may dismiss differs), so `Approval` needs the review's id (`Option<String>`). `Forge::withdraw(repo, number, approval)`, `Job::Withdraw`, a confirm prompt, then a refresh. Where the forge refuses (no permission), its message is shown as any failed write's.
  - Outdated: `Thread::outdated` (the forge's own flag) is read by all three adapters but only Claude's tool sees it; jed places threads itself. Tag a thread the forge calls outdated but jed placed (`outdated on <forge>` dimmed, in its CONVERSATIONS row and its block's header), so a thread that looks current here and hidden on the web page is explained. GitLab's flag is jed's own guess (`position.head_sha` differs from the head: every thread made before the latest push): don't tag on GitLab unless [review_forge_fixes](review_forge_fixes.md) found a better signal.
  - Tests in `jed/src/todo_tests.rs` (`mod review_links`): open and copy for a thread, a comment and the merge request (`url::opened`); a link to another host refused; withdraw against each fake; the outdated tag drawn. Unit tests for the URL check.
  - Done when: the three commands work on Gitea, GitHub and GitLab fakes as above, a foreign or non-HTTPS link is never opened, and `scripts/check` is green.
