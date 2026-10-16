//! Little-endian byte writing and bounds-checked reading, shared by the file
//! formats. Every read fails with [`Error::Format`] instead of panicking, so
//! a truncated or corrupt file errors cleanly.

use std::io::Read;

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;

use crate::error::Error;

/// The largest output a zlib stream can produce per input byte (deflate's
/// best case is about 1032:1). A declared size past this is corrupt, which
/// bounds what a hostile file can make a reader allocate.
const MAX_ZLIB_RATIO: usize = 1100;

pub(crate) fn corrupt(what: impl Into<String>) -> Error {
    Error::Format(what.into())
}

/// Appends little-endian values to a byte buffer.
#[derive(Default)]
pub(crate) struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn bytes(&mut self, v: &[u8]) {
        self.buf.extend_from_slice(v);
    }

    /// Overwrites a u32 written earlier at `at`.
    pub fn patch_u32(&mut self, at: usize, v: u32) {
        self.buf[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
}

/// Reads little-endian values from a byte slice.
#[derive(Clone)]
pub(crate) struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if n > self.remaining() {
            return Err(corrupt("unexpected end of data"));
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let mut out = [0; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    pub fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    pub fn i32(&mut self) -> Result<i32, Error> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    /// A count of items each at least `item_bytes` long, rejected when the
    /// rest of the data cannot hold that many. Callers allocate by it.
    pub fn count(&mut self, item_bytes: usize) -> Result<usize, Error> {
        let n = self.u32()? as usize;
        if n.saturating_mul(item_bytes.max(1)) > self.remaining() {
            return Err(corrupt("count larger than the data holding it"));
        }
        Ok(n)
    }

    pub fn utf8(&mut self, len: usize) -> Result<String, Error> {
        String::from_utf8(self.take(len)?.to_vec()).map_err(|_| corrupt("invalid UTF-8 text"))
    }
}

/// zlib-compresses `data` at `level` (0 stores, 9 is smallest).
pub(crate) fn zlib_compress(data: &[u8], level: u32) -> Vec<u8> {
    use std::io::Write;
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::new(level.min(9)));
    enc.write_all(data).expect("writing to a Vec cannot fail");
    enc.finish().expect("writing to a Vec cannot fail")
}

/// Inflates a zlib stream that must produce exactly `size` bytes.
pub(crate) fn zlib_decompress(data: &[u8], size: usize) -> Result<Vec<u8>, Error> {
    if size > data.len().saturating_mul(MAX_ZLIB_RATIO).saturating_add(64) {
        return Err(corrupt("compressed data too small for its declared size"));
    }
    let mut out = Vec::with_capacity(size);
    ZlibDecoder::new(data)
        .take(size as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| corrupt(format!("bad compressed data: {e}")))?;
    if out.len() != size {
        return Err(corrupt("compressed data has the wrong size"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_it_writes() {
        let mut w = Writer::new();
        w.u8(1);
        w.u16(0x0302);
        w.i32(-5);
        w.u32(7);
        let mut r = Reader::new(&w.buf);
        assert_eq!(r.u8().unwrap(), 1);
        assert_eq!(r.u16().unwrap(), 0x0302);
        assert_eq!(r.i32().unwrap(), -5);
        assert_eq!(r.u32().unwrap(), 7);
        assert!(r.is_empty());
        assert!(r.u8().is_err(), "reading past the end errors");
    }

    #[test]
    fn counts_are_bounded_by_the_data() {
        let mut w = Writer::new();
        w.u32(1_000_000);
        assert!(Reader::new(&w.buf).count(1).is_err());
    }

    #[test]
    fn zlib_round_trip_checks_size() {
        let data = vec![7u8; 10_000];
        let z = zlib_compress(&data, 6);
        assert_eq!(zlib_decompress(&z, data.len()).unwrap(), data);
        assert!(zlib_decompress(&z, data.len() - 1).is_err());
        assert!(zlib_decompress(&z, data.len() + 1).is_err());
        assert!(zlib_decompress(&z[..z.len() / 2], data.len()).is_err());
        assert!(zlib_decompress(&z, usize::MAX / 2).is_err());
    }
}
