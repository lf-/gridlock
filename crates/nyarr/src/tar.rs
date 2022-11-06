//! Making a nar from a tar file.

use std::{
    collections::BTreeMap,
    io::{Read, Seek, Write},
};

use crate::{ConstByteStream, Directory, Executable, FileName, FsObject};

/// FIXME: maybe don't use a ConstByteStream since it forces reading the entire
/// thing into memory. It's unclear how to do this with Seek, since the `tar`
/// docs say that:
///
/// > "Note that care must be taken to consider each entry within
/// > an archive in sequence. If entries are processed out of sequence (from what
/// > the iterator returns), then the contents read for each entry may be
/// > corrupted."
///
/// This seems to suggest that you can't random-read, sequential-write. Odd. So
/// I guess we are reading it all into memory.
pub fn tar_to_fsobject(
    tar: impl Read + Seek,
) -> Result<FsObject<ConstByteStream>, Box<dyn std::error::Error>> {
    let mut archive = tar::Archive::new(tar);
    let mut tree = Directory(BTreeMap::default());

    for member in archive.entries_with_seek()? {
        let mut member = member?;
        let entry_type = member.header().entry_type();

        let obj = if entry_type.is_dir() {
            FsObject::Directory(Directory::default())
        } else if entry_type.is_file() {
            let mut v = Vec::new();
            member.read_to_end(&mut v)?;

            FsObject::File(
                if member.header().mode()? & 0o111 != 0 {
                    Executable::IsExecutable
                } else {
                    Executable::NotExecutable
                },
                ConstByteStream(v),
            )
        } else if entry_type.is_symlink() {
            let name = member.link_name_bytes().ok_or("empty link name")?;
            FsObject::Symlink(FileName::try_from(name.as_ref())?)
        } else {
            // idk what that is, let's skip it
            continue;
        };

        let name = &FileName::try_from(member.path_bytes().as_ref());

        if let Err(_) = name {
            // it's just a ./ entry. we can ignore it.
        } else if let Ok(name) = name {
            tree.insert(name, obj)?;
        }
    }

    Ok(FsObject::Directory(tree))
}

pub fn tar_to_nar(
    tar: impl Read + Seek,
    mut into: impl Write,
) -> Result<(), Box<dyn std::error::Error>> {
    let fso = tar_to_fsobject(tar)?;

    fso.serialise_toplevel(&mut into)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::tests::basic_tree;

    use super::*;
    use std::io::Cursor;

    #[test]
    fn basic_tar() {
        let mut tarfile = Cursor::new(include_bytes!("testdata/test1.tar"));
        let fso = tar_to_fsobject(&mut tarfile).unwrap();
        assert_eq!(fso, basic_tree());
    }

    #[test]
    fn matches_nix_nar() {
        let mut tarfile = Cursor::new(include_bytes!("testdata/test1.tar"));
        let expected = include_bytes!("testdata/test1.nar");

        let mut nar = Vec::new();
        tar_to_nar(&mut tarfile, &mut nar).unwrap();
        assert_eq!(nar, expected);
    }
}
