use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    io::{BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

use clap::Parser;
use color_eyre::{
    eyre::{bail, eyre, Context},
    Result,
};
use nyarr::tar::StripRoot;

#[derive(Parser, Debug)]
struct Tar2nar {
    /// Tar file to convert
    tarfile: PathBuf,
    /// Output file
    narfile: PathBuf,
    /// Skip verifying against the Nix encoder
    no_verify: bool,
    /// Whether to strip the root directory out of the extraction
    strip_root: bool,
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

fn check_status(status: ExitStatus) -> Result<()> {
    if !status.success() {
        Err(eyre!("command failed {:?}", status))
    } else {
        Ok(())
    }
}

fn extract_to_temp(file: &Path, strip_root: StripRoot) -> Result<tempfile::TempDir> {
    let temp = tempfile::tempdir()?;
    let mut one_top_level_arg = OsString::from("--one-top-level=");
    one_top_level_arg.push(temp.path().as_os_str());

    let strip_root_arg = if strip_root == StripRoot::StripRoot {
        ["--strip-components=1"].as_slice()
    } else {
        [].as_slice()
    };

    check_status(
        Command::new("tar")
            .args([&OsString::from("-xf"), file.as_os_str(), &one_top_level_arg])
            .args(strip_root_arg)
            .status()?,
    )?;
    Ok(temp)
}

fn nix_nar(file: &Path, strip_root: StripRoot) -> Result<Vec<u8>> {
    let extracted = extract_to_temp(file, strip_root)?;
    let out = Command::new("nix-store")
        .args([&OsString::from("--dump"), extracted.path().as_os_str()])
        .output()?;
    check_status(out.status)?;
    Ok(out.stdout)
}

fn tar2nar(args: Tar2nar) -> Result<()> {
    let strip_root = if args.strip_root {
        StripRoot::StripRoot
    } else {
        StripRoot::DontStripRoot
    };

    let mut reader = BufReader::new(File::open(&args.tarfile).context("opening tar file")?);
    let mut writer = BufWriter::new(
        OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open(args.narfile)
            .context("opening output nar file")?,
    );
    let mut out = Vec::new();

    nyarr::tar::tar_to_nar(&mut reader, &mut out, strip_root)
        .map_err(|e| eyre!(e))
        .context("error converting from tar to nar")?;

    writer.write_all(&out)?;

    if !args.no_verify {
        let nix_result = nix_nar(&args.tarfile, strip_root)?;
        if &out != &nix_result {
            bail!("Mismatched NAR results! This is a bug. Reproduce with `nix-store --dump EXTRACTED_DIR`.");
        }
    }

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
