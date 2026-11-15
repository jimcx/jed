---
id: review_find_mr
name: Finding merge requests
summary: the Code Review panel finds a fork's merge request on the upstream repository, shows a branch's merged or closed merge request, and maps an SSH host alias to its forge
status: open
priority: 0
dependencies: [mr_review, review_forge_fixes]
---

The lookup asks only the checked-out branch's own remote for open merge requests (`review::lookup`, `Forge::find_open`). Three gaps, three subtasks in order, one commit each.

  - Needs: the lookup (`jed/src/review.rs`: `lookup`, `Request`, `Empty`, `Review::candidates`), `git::range::local_info` (the branch's remote and URL), the forge layer (`jed/src/forge/mod.rs` `Forge::find_open`, `MergeRequest::head_repo`, `forge/url.rs` `parse_remote`, `forge/config.rs`, the three adapters, `fake.rs`), the panel's summary and switcher (`jed/src/ui/review.rs`, `app/review.rs` `review_switch`), writes (`app/review_write.rs`: what a closed merge request refuses). [review_forge_fixes](review_forge_fixes.md)'s subtask c fills `head_repo` on every forge and makes `find_open` match the head repository; this task builds on it.
  - Rules for every subtask: tests in `jed/src/review_find_mr_tests.rs` (a `//!` doc, a `#[cfg(test)] mod` line in `main.rs`), named `review_find_mr_*`; adapters against `forge::fake`; repositories from `git::test_repo`. The security rule stands: a host, API URL or token variable comes only from `forges.toml` — a remote's URL picks *which configured host* is asked, never an unconfigured one, and a repository file can't add an alias. jed still never fetches. Docs kept in step (`docs/forges.md`, conventions' Forges section). No real hostnames.
  - Done when:
    - in a fork's clone (the branch pushes to the fork's remote, the merge request is on the upstream repository), the panel finds the merge request and Submit Review, Reply and Resolve go to the upstream repository
    - on a branch whose merge request was merged or closed (and has no open one), the panel shows it as `merged` / `closed` with its files and threads; Submit Review is refused with a message that says why
    - a remote `git@work:alice/project.git`, with `ssh_hosts = ["work"]` on a `forges.toml` entry, is that entry's forge
    - `scripts/check` is green
  - Subtasks (one agent and one commit each):
    - review_find_mr-a: a fork's merge request on the upstream repository
        - First check each forge's API for listing merge requests by head repository and branch: GitHub `GET /repos/{upstream}/pulls?head={fork owner}:{branch}`; Gitea's list has no head filter (filter `head.repo.full_name` and `head.ref`); GitLab `GET /projects/{upstream}/merge_requests?source_branch=` then `source_project_id` against the fork's project (or the fork's `forked_from_project`). Record it in mr_review.md.
        - `Forge::find_open(repo, head_repo, head_branch)`: the open merge requests on `repo` whose head is `head_branch` of `head_repo` (the same repository in the common case).
        - The lookup asks the branch's own repository first (today's behaviour); with nothing found it tries the clone's other remotes that parse to a repository on the same or another *configured* host (`upstream` first, then by name), with `head_repo` = the branch's own repository. At most 3 remotes, each one request, logged in the Review tool log. `Review::repo` is the repository the merge request lives in (writes and drafts key on it), and the summary's branch line shows `alice:feature → main` when the head is a fork's.
        - The merge base still comes from the forge or the local clone (`range::merge_base` given the *upstream* remote's name for the base branch); a missing base says "run `git fetch upstream`".
        - Tests: a `test_repo` with `origin` (the fork) and `upstream` remotes, the fake serving the merge request on upstream only, for each forge; a submit lands on the upstream path; a clone with only `origin` asks once.
    - review_find_mr-b: a merged or closed merge request
        - Today `find_open` is the only entry, so once a merge request is merged the panel says "No open merge request for <branch>" and the `closed` / `merged` words in the summary (`ui/review.rs:94`) can never show. With no open merge request, look for the branch's most recent merged or closed one (`Forge::find_recent(repo, head_repo, head_branch) -> Result<Option<MergeRequest>>`: Gitea / GitHub `state=closed` sorted by update, newest first, first page only; GitLab `state=merged` then `closed`, `order_by=updated_at`, `per_page=1`) whose head sha is local `HEAD` or an ancestor of it (so a reused branch name doesn't bring back an unrelated old one).
        - It is shown read-only: files against its merge base, threads, conversation, approvals; the state word in the summary styled (`review.merged` / `review.closed` theme keys); Add Comment, Submit Review and the suggestion and draft commands say "The merge request is merged" (reply and resolve still work on a closed-not-merged one if the forge allows it: check, else refuse the same way). Existing drafts stay in PENDING with that reason.
        - The 60 s poll for a merged or closed one drops to the slow path: refresh only on demand and when HEAD moves.
        - Tests: per forge, a merged merge request found and drawn as `merged`; a submit refused; an old closed one on an unrelated head not shown; an open one still wins.
    - review_find_mr-c: SSH host aliases
        - `parse_remote("git@work:alice/project.git")` gives the host `work` (an `~/.ssh/config` alias), which matches no entry. Add `ssh_hosts = ["work", "ssh.git.example.com"]` (optional, list of bare host names) to a `[[forge]]` entry: `ForgeConfig::resolve(host)` matches `host` or one of them and resolves to the entry's own host and API URL. Reading `~/.ssh/config` itself is out (parsing `Host` patterns, `Include`, `Match` is its own project): say so in `docs/forges.md`, with the `ssh_hosts` example.
        - `github.com` / `gitlab.com` with an alias need an explicit entry (`host = "github.com"`, `kind = "github"`, `ssh_hosts = [...]`); the detected defaults stay as they are. `RepoRef::host` becomes the entry's host (so drafts and viewed marks made through an alias and through the real name share a key). A duplicate alias across entries is that entry's error.
        - `Empty::UnknownHost`'s text names `ssh_hosts` when the remote was scp-style or `ssh://`.
        - Tests: `config.rs` (parse, validation, duplicate, `mcp_read_only` through an alias), a lookup through an aliased remote against the fake.
        - Then run the task's done-when, fill in "What it changed", "Deferred" and "Still to check by hand", and mark the task ✅.
  - Deferred: reviewing a merge request without checking out its branch; a picker of the repository's open merge requests; creating or merging merge requests; reading `~/.ssh/config`.
