# Configuring forges

jed's merge-request review talks to a forge (Gitea, GitHub or GitLab). Hosts are configured in
`~/.config/jed/forges.toml`. This file is the only place a host, API URL or token variable can
come from: `.workspace`, the settings and repositories can't set them, so a repository can't
point your token at a server it picks.

`github.com` and `gitlab.com` need no entry; just provide a token (`GITHUB_TOKEN` / `GITLAB_TOKEN`).
Any other host (a self-hosted Gitea, GitLab, …) needs one `[[forge]]` entry:

```toml
[[forge]]
host = "git.example.com"
kind = "gitea"                               # gitea | github | gitlab
api_url = "https://git.example.com/api/v1"   # optional
token_env = "GITEA_TOKEN"                    # optional
token = "…"                                  # optional
mcp_read_only = true                         # optional
```

| Key             | Required | Meaning |
|-----------------|----------|---------|
| `host`          | yes      | The host name of the remote (with the port for an `http(s)://` remote). |
| `kind`          | yes      | `gitea`, `github` or `gitlab`. |
| `api_url`       | no       | API root. Defaults to the host's standard one over HTTPS. `http://` is used only if you write it out here. |
| `token_env`     | no       | Environment variable holding the token. Default `GITEA_TOKEN`, `GITHUB_TOKEN` or `GITLAB_TOKEN` by `kind`. |
| `token`         | no       | The token itself, used instead of the environment variable. |
| `mcp_read_only` | no       | Default `false`. When `true`, Claude's review tools may read the review but not apply suggestions or draft replies. |

Add one `[[forge]]` table per host.

## Tokens

Prefer an environment variable. If you put a `token` in the file, run `chmod 600 ~/.config/jed/forges.toml`:
the whole file is ignored while it holds a token and group or others can read it. Tokens are never logged.

## Troubleshooting

- The file is ignored (with an error shown) if group or others can write to it.
- A syntax error, an unknown key or a wrong type (except for `mcp_read_only`) is reported as an error and the whole
  file is ignored: no host is configured.
- An invalid `host`, `kind`, `api_url` or `token_env` value, or a mistyped `mcp_read_only`, skips just that entry.
- While the file has any error, Claude's review tools are read-only on hosts without a valid entry (including github.com / gitlab.com); configured hosts keep their own `mcp_read_only`.
- Redirects are not followed and only HTTPS is used unless `api_url` says `http://`.

## Suggestions

In jed a plain ` ```suggestion ` fence in a comment replaces the lines the comment is on, and
`suggestion:-N+M` the lines from N above to M below its last line (up to 100 each way). GitLab reads a
plain fence as its last line only, so on GitLab jed shows a plain fence in someone else's comment as
`suggestion:-0+0` and posts yours (new, edited or a reply) as `suggestion:-N+0` over the thread's lines:
what you see in jed is what GitLab's page offers.

## Approvals and change requests

The panel names who approved and who asked for changes: each reviewer's latest review counts, and a
dismissed review leaves that reviewer with nothing (not their earlier verdict). A Gitea approval marked
stale (a push came after it) still shows as approved. GitLab's change requests come from the merge
request's reviewers (`requested_changes`); an older GitLab without that shows approvals only. Request
Changes with no comments is sent with the body "Changes requested", which Gitea and GitHub require.
