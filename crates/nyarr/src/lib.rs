//! Virtual filesystem backed NAR file library.
//!
//! See Figure 5.2 of Eelco's thesis for details.
pub mod hash;
pub mod tar;

use std::{collections::BTreeMap, fmt, io::Write};

pub type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Executable {
    IsExecutable,
    NotExecutable,
}

/// Generic byte stream. Allows for lazily reading it out, for instance by
/// indexing a tar file then reading contents later.
pub trait ByteStream {
    fn write_into(&self, w: &mut dyn Write) -> WriteResult;
    fn len(&self) -> usize;
}

type PathComponent = Vec<u8>;

/// Abstract path, not system dependent.
#[derive(PartialOrd, Ord, PartialEq, Eq, Clone)]
pub struct FileName(Vec<PathComponent>);

impl fmt::Debug for FileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("FileName")
            .field(&self.0.iter().map(|e| DebugU8(e)).collect::<Vec<_>>())
            .finish()
    }
}

impl FileName {
    pub fn singleton(item: PathComponent) -> Self {
        Self(vec![item])
    }

    pub fn file_name(&self) -> Option<&PathComponent> {
        self.0.last()
    }

    pub fn parent(&self) -> Option<FileName> {
        if self.0.len() <= 1 {
            None
        } else {
            Some(Self(self.0[..self.0.len() - 1].to_vec()))
        }
    }

    pub fn drop_first(&self) -> Option<FileName> {
        if self.0.len() <= 1 {
            None
        } else {
            Some(Self(self.0[1..].to_vec()))
        }
    }

    pub fn to_path(&self) -> Vec<u8> {
        self.0.join(&b'/')
    }
}

impl TryFrom<&[u8]> for FileName {
    type Error = &'static str;
    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let bits: Vec<_> = value
            .split(|&c| c == b'/')
            .map(|a| a.to_vec())
            .filter(|e| e != b"" && e != b".")
            .collect();
        if bits.len() == 0 {
            Err("empty file name")
        } else {
            Ok(Self(bits))
        }
    }
}

#[derive(PartialOrd, Ord, PartialEq, Eq, Clone)]
pub struct ConstByteStream(Vec<u8>);

impl ByteStream for ConstByteStream {
    fn write_into(&self, w: &mut dyn Write) -> WriteResult {
        w.write_all(&self.0)
    }

    fn len(&self) -> usize {
        self.0.len()
    }
}

pub struct Directory<T: ByteStream>(BTreeMap<PathComponent, Box<FsObject<T>>>);
impl<T: ByteStream> Default for Directory<T> {
    fn default() -> Self {
        Directory(BTreeMap::default())
    }
}

impl<T: PartialEq + ByteStream> PartialEq for Directory<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<T: ByteStream> fmt::Debug for Directory<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut dbg = f.debug_map();
        for (k, v) in self.0.iter() {
            dbg.entry(&DebugU8(k), v);
        }
        dbg.finish()
    }
}

/// See figure 5.1 of Eelco's thesis for details.
pub enum FsObject<T: ByteStream> {
    File(Executable, T),
    Directory(Directory<T>),
    Symlink(FileName),
}

impl<T: PartialEq + ByteStream> PartialEq for FsObject<T> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (FsObject::File(a1, b1), FsObject::File(a2, b2)) => a1 == a2 && b1 == b2,
            (FsObject::Directory(d1), FsObject::Directory(d2)) => d1 == d2,
            (FsObject::Symlink(s1), FsObject::Symlink(s2)) => s1 == s2,
            _ => false,
        }
    }
}

impl<T: ByteStream> fmt::Debug for FsObject<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FsObject::File(ex, _) => f.debug_tuple("File").field(ex).finish(),
            FsObject::Directory(list) => list.fmt(f),
            FsObject::Symlink(to) => f.debug_tuple("Symlink").field(to).finish(),
        }
    }
}

struct DebugU8<'a>(&'a [u8]);
impl<'a> fmt::Debug for DebugU8<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        String::from_utf8_lossy(self.0).fmt(f)
    }
}

type WriteResult = Result<(), std::io::Error>;

trait Serializable {
    /// Serialises this thing.
    fn serialise_just(&self, w: &mut impl Write) -> WriteResult;
}

impl<T: ByteStream> Serializable for T {
    fn serialise_just(&self, w: &mut impl Write) -> WriteResult {
        let padding_bytes = [0u8; 8];
        let len = self.len() as u64;
        w.write_all(&len.to_le_bytes())?;

        self.write_into(w)?;
        let padding_num = if len & 0x7 == 0 { 0 } else { 8 - (len & 0x7) };
        w.write_all(&padding_bytes[..padding_num as usize])
    }
}

impl Serializable for &[u8] {
    /// str(..) from fig 5.1
    fn serialise_just(&self, w: &mut impl Write) -> WriteResult {
        let padding_bytes = [0u8; 8];
        let len = self.len() as u64;
        w.write_all(&len.to_le_bytes())?;

        w.write_all(self)?;
        let padding_num = if len & 0x7 == 0 { 0 } else { 8 - (len & 0x7) };
        w.write_all(&padding_bytes[..padding_num as usize])?;

        Ok(())
    }
}

fn type_(s: &[u8], w: &mut impl Write) -> WriteResult {
    (b"type".as_ref()).serialise_just(w)?;
    s.serialise_just(w)
}

fn str(s: &[u8], w: &mut impl Write) -> WriteResult {
    s.serialise_just(w)
}

impl<T: ByteStream> Directory<T> {
    pub fn insert(&mut self, path: &FileName, obj: FsObject<T>) -> Result<(), Error> {
        match &path.0.as_slice() {
            &[one] => {
                // base case, a file at the root relative to me
                self.0.insert(one.to_vec(), Box::new(obj));
            }
            &[top, ref rest @ ..] => {
                assert_ne!(top, b"");
                let fso = self
                    .0
                    .entry(top.clone())
                    .or_insert_with(|| Box::new(FsObject::Directory(Directory::default())));

                if let FsObject::Directory(d) = &mut **fso {
                    d.insert(&FileName(rest.to_vec()), obj)?;
                } else {
                    return Err("attempt to insert into a non directory".into());
                };
            }
            &[] => {
                panic!("how did we get an empty path?");
            }
        }
        Ok(())
    }
}

impl<T: ByteStream> FsObject<T> {
    /// Equivalent to `serialise`
    pub fn serialise_toplevel(&self, w: &mut impl Write) -> WriteResult {
        str(b"nix-archive-1", w)?;
        self.serialise_wrapped(w)
    }

    /// Equivalent to `serialise'`.
    pub fn serialise_wrapped(&self, w: &mut impl Write) -> WriteResult {
        str(b"(", w)?;
        self.serialise_one(w)?;
        str(b")", w)
    }

    /// Serialises one [`FsObject`] into a writer.
    ///
    /// This is equivalent to `serialise''` in Figure 5.2
    pub fn serialise_one(&self, w: &mut impl Write) -> WriteResult {
        match self {
            FsObject::File(exec, content) => {
                type_(b"regular", w)?;
                if let Executable::IsExecutable = exec {
                    str(b"executable", w)?;
                }
                str(b"contents", w)?;
                content.serialise_just(w)?;
            }
            FsObject::Directory(entries) => {
                type_(b"directory", w)?;

                // FIXME: assert that the thing is sorted

                for (name, v) in &entries.0 {
                    str(b"entry", w)?;
                    str(b"(", w)?;
                    str(b"name", w)?;
                    str(&name, w)?;
                    str(b"node", w)?;
                    v.serialise_wrapped(w)?;
                    str(b")", w)?;
                }
            }
            FsObject::Symlink(name) => {
                type_(b"symlink", w)?;
                str(b"target", w)?;
                name.to_path().as_slice().serialise_just(w)?;
            }
        }
        Ok(())
    }
}

pub fn dir_entry(
    name: &[u8],
    obj: FsObject<ConstByteStream>,
) -> (PathComponent, Box<FsObject<ConstByteStream>>) {
    (name.to_vec(), Box::new(obj))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(inp: &[u8], expected: &[u8]) {
        let mut out = Vec::new();
        inp.serialise_just(&mut out).unwrap();
        string_props(&out);

        assert_eq!(&out, expected);
    }

    fn string_props(v: &[u8]) {
        let (len, rest) = v.split_at(8);
        let len = u64::from_le_bytes(len.try_into().unwrap());
        assert!(rest.len() as u64 >= len);
        assert_eq!(rest.len() & 0x7, 0);
    }

    #[test]
    fn string_padding() {
        check(b"", b"\x00\x00\x00\x00\x00\x00\x00\x00");
        check(
            b"\x10\x12",
            b"\x02\x00\x00\x00\x00\x00\x00\x00\x10\x12\x00\x00\x00\x00\x00\x00",
        );
        check(
            b"\x10\x11\x12\x13\x14\x15\x16\x17",
            b"\x08\x00\x00\x00\x00\x00\x00\x00\x10\x11\x12\x13\x14\x15\x16\x17",
        );
    }

    fn assert_bytes_eq(expected: &[u8], val: &[u8]) {
        if val != expected {
            panic!(
                "Value did not match expected.\n\
                   Lengths: expected: {}, actual: {}\n\
                   Expected:\n{}\n\
                   Actual:\n{}\
                   ",
                expected.len(),
                val.len(),
                hexdump::HexDumper::new(expected),
                hexdump::HexDumper::new(val),
            );
        }
    }

    fn check_tree(t: FsObject<ConstByteStream>, expected: &[u8]) {
        let mut out = Vec::new();
        t.serialise_toplevel(&mut out).unwrap();
        assert_bytes_eq(expected, &out);
    }

    pub fn basic_tree() -> FsObject<ConstByteStream> {
        FsObject::Directory(Directory(BTreeMap::from([
            dir_entry(b"dire", FsObject::Directory(Directory(BTreeMap::default()))),
            dir_entry(
                b"f",
                FsObject::File(
                    Executable::NotExecutable,
                    ConstByteStream(b"aaa\n".to_vec()),
                ),
            ),
            dir_entry(b"f2", FsObject::Symlink(FileName::singleton(b"f".to_vec()))),
        ])))
    }

    #[test]
    fn basic() {
        check_tree(basic_tree(), include_bytes!("testdata/test1.nar"));
    }

    #[test]
    fn unordered() {
        check_tree(
            FsObject::Directory(Directory(BTreeMap::from([
                dir_entry(
                    b"f",
                    FsObject::File(
                        Executable::NotExecutable,
                        ConstByteStream(b"aaa\n".to_vec()),
                    ),
                ),
                dir_entry(b"dire", FsObject::Directory(Directory(BTreeMap::default()))),
                dir_entry(b"f2", FsObject::Symlink(FileName::singleton(b"f".to_vec()))),
            ]))),
            include_bytes!("testdata/test1.nar"),
        );
    }
}
