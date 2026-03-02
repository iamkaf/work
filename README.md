# work

A CLI that scans a directory for git repos and prints your recent commits across all of them. Useful when you want a quick sense of "how much did I actually ship lately?"

```
$ work ~/code
2026-02-13 19:02  apps/dashboard  1a2b3c4  +12 -3  fix: make sidebar sticky
2026-02-13 18:11  tools/dirty      8d9e0f1  +48 -10 chore: add tests for nested repos
2026-02-12 22:40  libs/ui-kit      44aa991  +120 -7 feat: new button variant

50 commits shown (last 7 days)
Total LoC: +180 -20
```

## Install

```sh
cargo install --git https://github.com/iamkaf/work
```

## Usage

```
work <path>                 # recent commits (default: last 7 days, limit 50)
work --today <path>          # commits since local midnight
work --month <path>          # commits since the start of the local calendar month
work --last-month <path>     # commits since the start of the previous local calendar month
work --days 1 <path>         # just today-ish (rolling 24h window)
work -l 200 --days 30 <path> # longer window
work --remote <path>         # fetch before scanning (slower)
work --all <path>            # don't filter to your author identity
work -r <path>               # raw TSV for piping
```

| Flag | Short | Default | Description |
|------|-------|---------|-------------|
| `--depth` | `-L` | `3` | Max directory depth to search for repos |
| `--days` |  | `7` | How many days back to look |
| `--today` |  | off | Shortcut for commits since local midnight |
| `--month` |  | off | Shortcut for commits since the start of the local calendar month |
| `--last-month` |  | off | Shortcut for commits since the start of the previous local calendar month |
| `--limit` | `-l` | `50` | Max number of commits to print (across all repos) |
| `--remote` |  | off | Fetch from remotes before scanning |
| `--all` |  | off | Show commits by anyone (ignore your author identity filter) |
| `--merges` |  | off | Include merge commits (skipped by default) |
| `--raw` | `-r` | off | Tab-separated output: `time\trepo\thash\t+ins\t-del\tsubject` |

## How it works

1. Walks directories up to the specified depth looking for `.git` folders
2. Opens each repo (in parallel) and walks commits from `HEAD`, newest-first
3. Filters to commits authored by your configured git identity (`user.email` / `user.name`) unless you pass `--all`
4. Optionally runs `git fetch --prune` per repo when `--remote` is enabled
