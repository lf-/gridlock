use std::path::{Path, PathBuf};

use color_eyre::eyre::eyre;
use gridlock::{
    plan_update, read_lockfile, write_lockfile, GitHubClient, Lock, Lockfile, LockfileChange,
    OnlineGitHubClient,
};
use owo_colors::OwoColorize;

#[derive(clap::Parser)]
struct Args {
    /// Lockfile to update
    #[clap(long)]
    lockfile: PathBuf,

    #[clap(subcommand)]
    subcommand: Subcommand,
}

#[derive(clap::Parser)]
struct Update {
    /// Package name to update. If not specified, everything will be updated.
    package_name: Option<String>,
}

#[derive(clap::Parser)]
struct Add {
    /// Owner/repo pair. For example, `lf-/gridlock`.
    repo_ref: String,
    /// Branch to use. By default we will use the default branch.
    branch: Option<String>,
    /// Name to use for this package. Defaults to the repository name.
    name: Option<String>,
}

#[derive(clap::Parser)]
enum Subcommand {
    Update(Update),
    Show,
    Add(Add),
    Init,
}

fn boldprint(head: &str, f: impl std::fmt::Display) {
    println!("  {}: {}", head.bold(), f);
}

async fn do_show(lockfile_path: &Path) -> color_eyre::Result<()> {
    let lockfile = read_lockfile(lockfile_path).await?;

    for (name, package) in lockfile.packages {
        println!("{name}");
        boldprint("Branch", &package.branch);
        boldprint("Rev", &package.rev);
        boldprint(
            "Last updated",
            package
                .last_updated
                .map(|v| {
                    v.0.with_timezone(&chrono::Local)
                        .format("%F %T")
                        .to_string()
                })
                .unwrap_or("Unknown".into()),
        );
        boldprint(
            "Web link",
            format!(
                "https://github.com/{}/{}/tree/{}",
                package.owner, package.repo, package.rev
            ),
        );
    }
    Ok(())
}

async fn do_update(lockfile_path: &Path, update: Update) -> color_eyre::Result<()> {
    let mut lockfile = read_lockfile(lockfile_path).await?;
    let client = OnlineGitHubClient::new()?;

    let plan = plan_update(
        &client,
        &lockfile,
        update.package_name.as_ref().map(String::as_str),
    )
    .await?;

    println!("Plan: {plan:?}");

    for change in plan {
        match change {
            LockfileChange::UpdateRev(name, rev) => {
                let p = lockfile.packages.get_mut(&name).unwrap();
                let new_lock = client
                    .create_lock(&p.owner, &p.repo, &p.branch, &rev)
                    .await?;
                *p = Lock {
                    extra: std::mem::take(&mut p.extra),
                    ..new_lock
                };
            }
        }
    }

    write_lockfile(lockfile_path, &lockfile).await?;

    Ok(())
}

async fn do_add(lockfile_path: &Path, add: Add) -> color_eyre::Result<()> {
    let client = OnlineGitHubClient::new()?;

    let mut lockfile = read_lockfile(lockfile_path).await?;

    let (owner, repo) = add
        .repo_ref
        .split_once('/')
        .ok_or_else(|| eyre!("Repo ref should be formatted like 'owner/repo'"))?;

    let (head, branch_name) = client
        .branch_head(owner, repo, add.branch.as_deref())
        .await?;

    let item_name = add.name.unwrap_or_else(|| repo.to_string());

    println!("Adding {owner}/{repo} at {branch_name}: {head}");
    let lock = client.create_lock(owner, repo, &branch_name, &head).await?;

    // FIXME: should "add" update?
    lockfile.packages.insert(item_name, lock);

    write_lockfile(lockfile_path, &lockfile).await?;

    Ok(())
}

async fn do_init(lockfile_path: &Path) -> color_eyre::Result<()> {
    let lockfile = Lockfile::default();
    write_lockfile(lockfile_path, &lockfile).await?;

    Ok(())
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let args = <Args as clap::Parser>::parse();

    match args.subcommand {
        Subcommand::Update(u) => do_update(&args.lockfile, u).await,
        Subcommand::Show => do_show(&args.lockfile).await,
        Subcommand::Add(a) => do_add(&args.lockfile, a).await,
        Subcommand::Init => do_init(&args.lockfile).await,
    }
}
