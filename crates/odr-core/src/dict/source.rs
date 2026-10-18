//! Block access for the dictionary resolver.
//!
//! [`BlockSource`] is the only way the resolver touches bytes: by
//! `(absolute file number, block number)`. Two implementations ship:
//! [`FileSet`] over read-only [`Evidence`] files (production) and
//! [`ImageSet`] over in-memory images (tests, fuzzing). Neither can write.
//!
//! The block size and byte order are properties of the **set** (one
//! database), not of a file: Oracle requires every datafile of a tablespace
//! to share the tablespace block size, and the whole database shares one
//! endianness. Mixed block sizes across tablespaces are supported by
//! registering each file with its own block size.

use crate::block::{Rdba, RdbaScheme};
use crate::bytes::ByteOrder;
use crate::source::Evidence;
use std::cell::RefCell;
use std::collections::BTreeMap;

/// Where a block comes from, for provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockAddr {
    /// Absolute file number (`V$DATAFILE.FILE#`).
    pub file: u32,
    /// Block number within the file (block 1 = `kcvfh`).
    pub block: u32,
}

/// Read-only access to the blocks of one database.
pub trait BlockSource {
    /// Byte order of the database.
    fn order(&self) -> ByteOrder;
    /// Block size of `file`, if the file is registered.
    fn block_size(&self, file: u32) -> Option<usize>;
    /// Rdba scheme of the tablespace `file` belongs to.
    fn scheme(&self, file: u32) -> Option<RdbaScheme>;
    /// Map a relative file number to an absolute one. For a single-file
    /// (bigfile) tablespace, `rel` is ignored.
    fn absolute_file(&self, rel: Option<u32>, hint: u32) -> Option<u32>;
    /// The bytes of one block, or `None` if the file is unknown, the block
    /// is beyond EOF, or it cannot be read. Never panics.
    fn block(&self, addr: BlockAddr) -> Option<Vec<u8>>;

    /// Resolve an rdba found inside `from_file` into an absolute address.
    /// Under smallfile, the relative file number is looked up; under
    /// bigfile, the block belongs to `from_file` itself.
    fn resolve(&self, rdba: Rdba, from_file: u32) -> Option<BlockAddr> {
        let scheme = self.scheme(from_file)?;
        let (rel, block) = rdba.split(scheme);
        let file = self.absolute_file(rel, from_file)?;
        Some(BlockAddr { file, block })
    }
}

/// Per-file registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileInfo {
    /// Absolute file number.
    pub file: u32,
    /// Relative file number within its tablespace (`None` for bigfile).
    pub relative: Option<u32>,
    /// Block size in bytes.
    pub block_size: usize,
    /// Rdba scheme of the tablespace.
    pub scheme: RdbaScheme,
}

/// In-memory images, keyed by absolute file number. For tests and fuzzing.
#[derive(Debug)]
pub struct ImageSet {
    order: ByteOrder,
    files: BTreeMap<u32, (FileInfo, Vec<u8>)>,
}

impl ImageSet {
    /// Empty set with the given byte order.
    pub fn new(order: ByteOrder) -> Self {
        Self {
            order,
            files: BTreeMap::new(),
        }
    }

    /// Register an image. Replaces any previous image for the same file number.
    pub fn add(&mut self, info: FileInfo, image: Vec<u8>) {
        self.files.insert(info.file, (info, image));
    }
}

impl BlockSource for ImageSet {
    fn order(&self) -> ByteOrder {
        self.order
    }
    fn block_size(&self, file: u32) -> Option<usize> {
        self.files.get(&file).map(|(i, _)| i.block_size)
    }
    fn scheme(&self, file: u32) -> Option<RdbaScheme> {
        self.files.get(&file).map(|(i, _)| i.scheme)
    }
    fn absolute_file(&self, rel: Option<u32>, hint: u32) -> Option<u32> {
        match rel {
            None => Some(hint),
            Some(r) => self
                .files
                .values()
                .find(|(i, _)| i.relative == Some(r))
                .map(|(i, _)| i.file),
        }
    }
    fn block(&self, addr: BlockAddr) -> Option<Vec<u8>> {
        let (info, img) = self.files.get(&addr.file)?;
        let bs = info.block_size;
        let start = usize::try_from(addr.block).ok()?.checked_mul(bs)?;
        let end = start.checked_add(bs)?;
        img.get(start..end).map(<[u8]>::to_vec)
    }
}

/// Read-only datafiles on disk (ADR-0002). Blocks are read on demand; a
/// small LRU-less cache of the last blocks avoids re-reading hot dictionary
/// blocks.
pub struct FileSet {
    order: ByteOrder,
    files: BTreeMap<u32, (FileInfo, RefCell<Evidence>)>,
    cache: RefCell<BTreeMap<BlockAddr, Vec<u8>>>,
    cache_cap: usize,
}

impl std::fmt::Debug for FileSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileSet")
            .field("order", &self.order)
            .field("files", &self.files.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl FileSet {
    /// Empty set. `cache_blocks` bounds the in-memory block cache.
    pub fn new(order: ByteOrder, cache_blocks: usize) -> Self {
        Self {
            order,
            files: BTreeMap::new(),
            cache: RefCell::new(BTreeMap::new()),
            cache_cap: cache_blocks.max(1),
        }
    }

    /// Register an already-opened evidence file.
    pub fn add(&mut self, info: FileInfo, evidence: Evidence) {
        self.files.insert(info.file, (info, RefCell::new(evidence)));
    }

    /// Registered files.
    pub fn files(&self) -> impl Iterator<Item = &FileInfo> + '_ {
        self.files.values().map(|(i, _)| i)
    }
}

impl BlockSource for FileSet {
    fn order(&self) -> ByteOrder {
        self.order
    }
    fn block_size(&self, file: u32) -> Option<usize> {
        self.files.get(&file).map(|(i, _)| i.block_size)
    }
    fn scheme(&self, file: u32) -> Option<RdbaScheme> {
        self.files.get(&file).map(|(i, _)| i.scheme)
    }
    fn absolute_file(&self, rel: Option<u32>, hint: u32) -> Option<u32> {
        match rel {
            None => Some(hint),
            Some(r) => self
                .files
                .values()
                .find(|(i, _)| i.relative == Some(r))
                .map(|(i, _)| i.file),
        }
    }
    fn block(&self, addr: BlockAddr) -> Option<Vec<u8>> {
        if let Some(b) = self.cache.borrow().get(&addr) {
            return Some(b.clone());
        }
        let (info, ev) = self.files.get(&addr.file)?;
        let bs = info.block_size;
        let off = u64::from(addr.block).checked_mul(u64::try_from(bs).ok()?)?;
        let mut buf = vec![0u8; bs];
        let n = ev.try_borrow_mut().ok()?.read_at(off, &mut buf).ok()?;
        if n != bs {
            return None;
        }
        let mut c = self.cache.borrow_mut();
        if c.len() >= self.cache_cap {
            c.clear();
        }
        c.insert(addr, buf.clone());
        Some(buf)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn image_set_resolves_smallfile_and_bigfile() {
        let mut s = ImageSet::new(ByteOrder::Little);
        s.add(
            FileInfo {
                file: 7,
                relative: Some(1),
                block_size: 16,
                scheme: RdbaScheme::Smallfile,
            },
            (0..64u8).collect(),
        );
        s.add(
            FileInfo {
                file: 9,
                relative: None,
                block_size: 16,
                scheme: RdbaScheme::Bigfile,
            },
            (100..164u8).collect(),
        );
        // smallfile rdba 1/2 found inside file 7 -> absolute file 7 block 2
        let a = s.resolve(Rdba::smallfile(1, 2).unwrap(), 7).unwrap();
        assert_eq!(a, BlockAddr { file: 7, block: 2 });
        assert_eq!(s.block(a).unwrap(), (32..48u8).collect::<Vec<_>>());
        // bigfile: all 32 bits are the block, file is the file we came from
        let b = s.resolve(Rdba(3), 9).unwrap();
        assert_eq!(b, BlockAddr { file: 9, block: 3 });
        assert_eq!(s.block(b).unwrap()[0], 148);
        // unknown relative file / beyond EOF / unknown file
        assert_eq!(s.resolve(Rdba::smallfile(5, 0).unwrap(), 7), None);
        assert_eq!(s.block(BlockAddr { file: 7, block: 4 }), None);
        assert_eq!(s.block(BlockAddr { file: 1, block: 0 }), None);
        assert_eq!(
            s.block(BlockAddr {
                file: 7,
                block: u32::MAX
            }),
            None
        );
    }
}
