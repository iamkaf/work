use clap::Parser;
use git2::{Config, Oid, Repository};
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Parser, Debug)]
#[command(about = "Show your recent commits across many git repos")]
struct Args {
    /// Directory to scan
    path: PathBuf,

    /// Max depth to search for repos
    #[arg(short = 'L', default_value = "3")]
    depth: usize,

    /// How many days back to look
    #[arg(long, default_value = "7", conflicts_with = "today")]
    days: i64,

    /// Shortcut for "commits since local midnight"
    #[arg(long, conflicts_with = "days")]
    today: bool,

    /// Max number of commits to print (across all repos)
    #[arg(short, long, default_value = "50")]
    limit: usize,

    /// Fetch from remotes before scanning (slower)
    #[arg(long)]
    remote: bool,

    /// Don't filter to your author identity
    #[arg(long)]
    all: bool,

    /// Include merge commits
    #[arg(long)]
    merges: bool,

    /// Raw output for piping (tab-separated)
    #[arg(short, long)]
    raw: bool,
}

#[derive(Clone, Debug)]
struct Identity {
    name: Option<String>,
    email: Option<String>,
}

#[derive(Clone, Debug)]
struct CommitLine {
    repo: PathBuf,
    time: i64,
    oid: Oid,
    summary: String,
    insertions: usize,
    deletions: usize,
}

fn find_repos(base: &Path, max_depth: usize) -> Vec<PathBuf> {
    let mut repos = Vec::new();
    collect_repos(base, max_depth, 0, &mut repos);
    repos.sort();
    repos
}

fn collect_repos(dir: &Path, max_depth: usize, depth: usize, repos: &mut Vec<PathBuf>) {
    if depth > max_depth {
        return;
    }
    if dir.join(".git").exists() {
        repos.push(dir.to_path_buf());
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && !path.is_symlink() {
            collect_repos(&path, max_depth, depth + 1, repos);
        }
    }
}

fn default_identity() -> Identity {
    let cfg = Config::open_default().ok();
    let name = cfg
        .as_ref()
        .and_then(|c| c.get_string("user.name").ok());
    let email = cfg
        .as_ref()
        .and_then(|c| c.get_string("user.email").ok());
    Identity { name, email }
}

fn fetch_repo(path: &Path) {
    // Keep it simple and compatible with whatever auth the user already has.
    let _ = Command::new("git")
        .args(["fetch", "--quiet", "--prune"])
        .current_dir(path)
        .status();
}

fn matches_identity(id: &Identity, author_name: Option<&str>, author_email: Option<&str>) -> bool {
    if id.name.is_none() && id.email.is_none() {
        // No configured identity; don't accidentally filter everything out.
        return true;
    }

    if let (Some(want), Some(got)) = (id.email.as_deref(), author_email) {
        if want.eq_ignore_ascii_case(got) {
            return true;
        }
    }

    if let (Some(want), Some(got)) = (id.name.as_deref(), author_name) {
        if want == got {
            return true;
        }
    }

    false
}

fn diff_stats(repo: &Repository, commit: &git2::Commit) -> (usize, usize) {
    let commit_tree = match commit.tree() {
        Ok(t) => t,
        Err(_) => return (0, 0),
    };

    let parent_tree = if commit.parent_count() >= 1 {
        commit
            .parent(0)
            .ok()
            .and_then(|p| p.tree().ok())
    } else {
        None
    };

    let diff = match repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&commit_tree), None) {
        Ok(d) => d,
        Err(_) => return (0, 0),
    };

    let stats = match diff.stats() {
        Ok(s) => s,
        Err(_) => return (0, 0),
    };

    (stats.insertions(), stats.deletions())
}

fn collect_commits(repo_path: &Path, since: i64, id: &Identity, args: &Args) -> Vec<CommitLine> {
    if args.remote {
        fetch_repo(repo_path);
    }

    let Ok(repo) = Repository::open(repo_path) else {
        return Vec::new();
    };

    let head = match repo.head() {
        Ok(h) => h,
        Err(_) => return Vec::new(),
    };

    let oid = match head.target() {
        Some(oid) => oid,
        None => return Vec::new(),
    };

    let mut walk = match repo.revwalk() {
        Ok(w) => w,
        Err(_) => return Vec::new(),
    };
    if walk.push(oid).is_err() {
        return Vec::new();
    }
    let _ = walk.set_sorting(git2::Sort::TIME);

    let mut out = Vec::new();
    for item in walk.flatten() {
        let Ok(commit) = repo.find_commit(item) else {
            continue;
        };

        let t = commit.time().seconds();
        if t < since {
            // Since revwalk is time-sorted, we can stop early.
            break;
        }

        if !args.merges && commit.parent_count() > 1 {
            continue;
        }

        if !args.all {
            let author = commit.author();
            if !matches_identity(id, author.name(), author.email()) {
                continue;
            }
        }

        let (insertions, deletions) = diff_stats(&repo, &commit);

        let summary = commit
            .summary()
            .unwrap_or("(no message)")
            .trim()
            .to_string();

        out.push(CommitLine {
            repo: repo_path.to_path_buf(),
            time: t,
            oid: commit.id(),
            summary,
            insertions,
            deletions,
        });
    }

    out
}

fn format_time_local(ts: i64) -> String {
    use chrono::{Local, TimeZone};
    let dt = Local.timestamp_opt(ts, 0).single();
    dt.map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| ts.to_string())
}

fn run(args: Args) -> Result<(), String> {
    let base = args
        .path
        .canonicalize()
        .map_err(|_| format!("work: cannot access '{}'", args.path.display()))?;

    let repos = find_repos(&base, args.depth);
    if repos.is_empty() {
        return Err(format!("No git repos found in {}", base.display()));
    }

    let id = default_identity();
    let since = if args.today {
        use chrono::{Local, TimeZone};
        let now = Local::now();
        let midnight = now
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| "Failed to compute local midnight".to_string())?;
        Local
            .from_local_datetime(&midnight)
            .single()
            .ok_or_else(|| "Failed to resolve local midnight".to_string())?
            .timestamp()
    } else {
        chrono::Local::now()
            .timestamp()
            .saturating_sub(args.days.saturating_mul(24 * 60 * 60))
    };
    let mut commits: Vec<CommitLine> = repos
        .par_iter()
        .flat_map_iter(|r| collect_commits(r, since, &id, &args))
        .collect();

    commits.sort_by_key(|c| -c.time);

    if commits.is_empty() {
        let window = if args.today {
            "today".to_string()
        } else {
            format!("the last {} days", args.days)
        };
        return Err(if args.all {
            format!("No commits found in {window}")
        } else {
            format!("No commits found for your identity in {window} (try --all)")
        });
    }

    let commits = commits.into_iter().take(args.limit).collect::<Vec<_>>();

    let mut total_ins: usize = 0;
    let mut total_del: usize = 0;

    // For pretty alignment we compute widths from the *displayed* commits.
    let repo_width = commits
        .iter()
        .map(|c| {
            c.repo
                .strip_prefix(&base)
                .unwrap_or(&c.repo)
                .display()
                .to_string()
                .len()
        })
        .max()
        .unwrap_or(0);

    let ins_width = commits
        .iter()
        .map(|c| c.insertions.to_string().len())
        .max()
        .unwrap_or(1);
    let del_width = commits
        .iter()
        .map(|c| c.deletions.to_string().len())
        .max()
        .unwrap_or(1);

    for c in &commits {
        let rel_repo = c.repo.strip_prefix(&base).unwrap_or(&c.repo);
        let rel_repo_s = rel_repo.display().to_string();
        let t = format_time_local(c.time);
        let short = c.oid.to_string();
        let short = &short[..7.min(short.len())];

        total_ins = total_ins.saturating_add(c.insertions);
        total_del = total_del.saturating_add(c.deletions);

        if args.raw {
            // time\trepo\thash\t+ins\t-del\tsummary
            println!(
                "{t}\t{}\t{short}\t+{}\t-{}\t{}",
                rel_repo.display(),
                c.insertions,
                c.deletions,
                c.summary
            );
        } else {
            // Colors:
            // - repo: bold
            // - hash: dim
            // - +ins: green
            // - -del: red
            let repo_padded = format!("{rel_repo_s:<repo_width$}", repo_width = repo_width);
            let repo_fmt = format!("\x1b[1m{repo_padded}\x1b[0m");
            let hash_fmt = format!("\x1b[2m{short}\x1b[0m");
            let plus_fmt = format!(
                "\x1b[32m+{:>ins_width$}\x1b[0m",
                c.insertions,
                ins_width = ins_width
            );
            let minus_fmt = format!(
                "\x1b[31m-{:>del_width$}\x1b[0m",
                c.deletions,
                del_width = del_width
            );

            println!(
                "{t}  {repo}  {hash}  {plus} {minus}  {msg}",
                repo = repo_fmt,
                hash = hash_fmt,
                plus = plus_fmt,
                minus = minus_fmt,
                msg = c.summary
            );
        }
    }

    if !args.raw {
        if args.today {
            println!("\n{} commits shown (today)", commits.len());
        } else {
            println!(
                "\n{} commits shown (last {} days)",
                commits.len(),
                args.days
            );
        }
        println!(
            "Total LoC: \x1b[32m+{}\x1b[0m \x1b[31m-{}\x1b[0m",
            total_ins, total_del
        );
    }

    Ok(())
}

fn main() {
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    if let Err(e) = run(Args::parse()) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn init_repo(tmp: &Path, name: &str) -> PathBuf {
        let dir = tmp.join(name);
        fs::create_dir_all(&dir).unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(&dir)
            .status()
            .unwrap();
        // Configure identity locally so commits have predictable author.
        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(&dir)
            .status()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(&dir)
            .status()
            .unwrap();
        dir
    }

    fn commit(dir: &Path, msg: &str) {
        fs::write(dir.join("file.txt"), msg).unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir)
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", msg, "-q"])
            .current_dir(dir)
            .status()
            .unwrap();
    }

    #[test]
    fn finds_repos_respects_depth() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path(), "a");
        init_repo(tmp.path(), "deep/nested/b");
        assert_eq!(find_repos(tmp.path(), 1).len(), 1);
        assert_eq!(find_repos(tmp.path(), 3).len(), 2);
    }

    #[test]
    fn collects_commits() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(tmp.path(), "a");
        commit(&repo, "one");
        commit(&repo, "two");

        let args = Args {
            path: tmp.path().to_path_buf(),
            depth: 3,
            days: 7,
            today: false,
            limit: 50,
            remote: false,
            all: true,
            merges: false,
            raw: true,
        };

        let since = chrono::Local::now().timestamp() - 7 * 24 * 60 * 60;
        let got = collect_commits(&repo, since, &Identity { name: None, email: None }, &args);
        assert!(got.len() >= 2);
    }
}
