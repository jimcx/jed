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
- An unknown key, or a wrong type, is reported as an error for that entry.
- Redirects are not followed and only HTTPS is used unless `api_url` says `http://`.
