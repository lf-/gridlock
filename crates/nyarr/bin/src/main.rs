use std::{
    fs::{File, OpenOptions},
    io::{BufReader, BufWriter},
    path::PathBuf,
};

use clap::Parser;
use color_eyre::{eyre::Context, Result};

#[derive(Parser, Debug)]
struct Tar2nar {
    /// Tar file to convert
    tarfile: PathBuf,
    /// Output file
    narfile: PathBuf,
}

#[derive(Parser, Debug)]
enum Subcommand {
    /// Convert a tar file to nar
    Tar2nar(Tar2nar),
}
#[derive(Parser, Debug)]
struct Args {
    #[clap(subcommand)]
    subcommand: Subcommand,
}

fn tar2nar(args: Tar2nar) -> Result<()> {
    let mut reader = BufReader::new(File::open(args.tarfile).context("opening tar file")?);
    let mut writer = BufWriter::new(
        OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open(args.narfile)
            .context("opening output nar file")?,
    );
    // FIXME: this is not eyre-able for Send + Sync something reasons??
    nyarr::tar::tar_to_nar(&mut reader, &mut writer).expect("error converting from tar to nar");
    Ok(())
}

fn main() -> Result<()> {
    color_eyre::install()?;
    let args = Args::parse();
    match args.subcommand {
        Subcommand::Tar2nar(t2n) => tar2nar(t2n),
    }?;
    Ok(())
}
