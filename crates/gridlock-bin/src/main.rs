use std::{
    collections::{btree_map::Entry, HashSet},
    path::{Path, PathBuf},
};

use color_eyre::eyre::eyre;
use gridlock::{
    plan_update, read_lockfile, write_lockfile, GitHubClient, Lock, Lockfile, LockfileChange,
    OnlineGitHubClient, Value,
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
    #[clap(long)]
    branch: Option<String>,

    /// Name to use for this package. Defaults to the repository name.
    #[clap(long)]
    name: Option<String>,
}

#[derive(clap::Parser)]
struct MetaSetInsert {
    /// Package name to edit.
    package_name: String,
    /// Name of the metadata entry to modify.
    meta_name: String,
    /// Things to insert into the set.
    #[clap(num_args = 1..)]
    value: Vec<String>,
}

#[derive(clap::Parser)]
struct MetaSet {
    /// Package name to edit.
    package_name: String,
    /// Name of the metadata item.
    meta_name: String,
    /// String to set it to.
    value: String,
}

#[derive(clap::Parser)]
enum Meta {
    /// Insert into an exclusive set maintained as a list.
    SetInsert(MetaSetInsert),
    /// Sets a metadata item to a string.
    Set(MetaSet),
}

#[derive(clap::Parser)]
enum Subcommand {
    Update(Update),
    Show,
    Add(Add),
    Init,
    #[clap(subcommand)]
    Meta(Meta),
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

    let old = lockfile.packages.entry(item_name);
    // Maintain extra information across multiple adds.
    match old {
        Entry::Vacant(ve) => {
            ve.insert(lock);
        }
        Entry::Occupied(occ) => {
            let (k, prev) = occ.remove_entry();
            lockfile.packages.insert(
                k,
                Lock {
                    extra: prev.extra,
                    ..lock
                },
            );
        }
    }

    write_lockfile(lockfile_path, &lockfile).await?;

    Ok(())
}

async fn do_init(lockfile_path: &Path) -> color_eyre::Result<()> {
    let lockfile = Lockfile::default();
    write_lockfile(lockfile_path, &lockfile).await?;

    Ok(())
}

async fn do_meta(lockfile_path: &Path, meta: Meta) -> color_eyre::Result<()> {
    let mut lockfile = read_lockfile(lockfile_path).await?;

    match meta {
        Meta::SetInsert(MetaSetInsert {
            package_name,
            meta_name,
            value,
        }) => {
            let entry = lockfile
                .packages
                .get_mut(&package_name)
                .ok_or_else(|| eyre!("Specified package does not exist"))?;
            let mut temp = Value::Array(vec![]);
            let val = entry
                .extra
                .get_mut(&meta_name)
                .unwrap_or(&mut temp)
                .as_array_mut()
                .ok_or_else(|| eyre!("Wrong type of metadata, expected array"))?;
            let content = std::mem::take(val);
            let set = content
                .into_iter()
                .filter_map(|v| match v {
                    Value::String(s) => Some(s),
                    _ => None,
                })
                .collect::<HashSet<String>>();
            let to_insert = value.into_iter().collect::<HashSet<String>>();

            let set = set.union(&to_insert).cloned().collect::<HashSet<String>>();

            *val = set.into_iter().map(Value::String).collect::<Vec<_>>();
        }
        Meta::Set(MetaSet {
            package_name,
            meta_name,
            value,
        }) => {
            let entry = lockfile
                .packages
                .get_mut(&package_name)
                .ok_or_else(|| eyre!("Specified package does not exist"))?;
            entry.extra.insert(meta_name, Value::String(value));
        }
    }

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
        Subcommand::Meta(meta) => do_meta(&args.lockfile, meta).await,
    }
}
