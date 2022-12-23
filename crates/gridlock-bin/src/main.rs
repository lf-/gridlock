use std::path::{Path, PathBuf};

use gridlock::{plan_update, read_lockfile, OnlineGitHubClient};

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
enum Subcommand {
    Update(Update),
}

async fn do_update(lockfile: &Path, update: Update) -> color_eyre::Result<()> {
    let lockfile = read_lockfile(lockfile)?;
    let client = OnlineGitHubClient::new()?;

    let plan = plan_update(
        &client,
        &lockfile,
        update.package_name.as_ref().map(String::as_str),
    )
    .await;

    println!("Plan: {plan:?}");

    Ok(())
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let args = <Args as clap::Parser>::parse();

    match args.subcommand {
        Subcommand::Update(u) => do_update(&args.lockfile, u).await,
    }
}
