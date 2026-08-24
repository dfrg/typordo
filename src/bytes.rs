use crate::error::{Error, Result};
use crate::layout;

/// A bounds-checked view over the whole cache file.
///
/// Every read in the crate goes through here. Reads are done byte-wise from
/// subslices, so the buffer needs no particular alignment and no `unsafe` is
/// involved anywhere in the crate.
///
/// # Byte order
///
/// Numbers are read in the machine's own order, because a cache is only ever
/// written in it. Fontconfig puts the endianness in the file name -- a
/// big-endian machine writes `be64` and asks for `be64` -- so a cache of the
/// wrong order is not one this crate rejects, it is one this crate never
/// looks for. See [`ARCHITECTURE`](crate::ARCHITECTURE).
#[derive(Clone, Copy)]
pub(crate) struct Bytes<'a>(&'a [u8]);

impl<'a> Bytes<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self(data)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    fn chunk<const N: usize>(&self, at: usize) -> Result<[u8; N]> {
        self.0
            .get(at..at.checked_add(N).ok_or(Error::Truncated { at, len: N })?)
            .and_then(|s| s.try_into().ok())
            .ok_or(Error::Truncated { at, len: N })
    }

    pub fn u16(&self, at: usize) -> Result<u16> {
        self.chunk::<2>(at).map(u16::from_ne_bytes)
    }

    pub fn u32(&self, at: usize) -> Result<u32> {
        self.chunk::<4>(at).map(u32::from_ne_bytes)
    }

    pub fn i32(&self, at: usize) -> Result<i32> {
        self.chunk::<4>(at).map(i32::from_ne_bytes)
    }

    pub fn i64(&self, at: usize) -> Result<i64> {
        self.chunk::<8>(at).map(i64::from_ne_bytes)
    }

    pub fn f64(&self, at: usize) -> Result<f64> {
        self.chunk::<8>(at).map(f64::from_ne_bytes)
    }

    /// A serialized pointer: four bytes or eight, depending on the target.
    ///
    /// Everything that follows a link in a cache goes through here rather
    /// than reading a fixed width, because an `intptr_t` is the one field
    /// whose size changes between the formats fontconfig writes.
    pub fn offset(&self, at: usize) -> Result<i64> {
        match layout::PTR {
            4 => self.i32(at).map(i64::from),
            _ => self.i64(at),
        }
    }

    /// A count that the format requires to be non-negative.
    pub fn count(&self, at: usize) -> Result<usize> {
        match self.i32(at)? {
            n if n < 0 => Err(Error::BadCount(n)),
            n => Ok(n as usize),
        }
    }

    /// Check that an array of `count` items of `stride` bytes starting at
    /// `base` fits inside the file, and return the count.
    ///
    /// Counts are read straight from the file, so a corrupt one can claim two
    /// billion entries. Without this the iterators would spin for hours over
    /// entries that cannot exist; a count is only trustworthy once the bytes
    /// behind it are known to be there.
    pub fn array(&self, base: usize, count: usize, stride: usize) -> Result<usize> {
        let end = count
            .checked_mul(stride)
            .and_then(|bytes| base.checked_add(bytes))
            .ok_or(Error::BadCount(i32::MAX))?;
        if end > self.0.len() {
            return Err(Error::Truncated { at: base, len: end - base });
        }
        Ok(count)
    }

    /// Resolve an offset taken relative to `base`, checking it lands inside
    /// the file.
    ///
    /// Offsets in the format are signed and routinely point backwards, so
    /// this cannot be a plain unsigned add.
    pub fn resolve(&self, base: usize, delta: i64) -> Result<usize> {
        let bad = || Error::BadOffset { base, delta };
        let at = i64::try_from(base).ok().and_then(|b| b.checked_add(delta)).ok_or_else(bad)?;
        let at = usize::try_from(at).map_err(|_| bad())?;
        // One past the end is a legal address, not a legal read: a zero-length
        // array serialized at the end of the file resolves to exactly `len`.
        // Every read through this type is bounds-checked separately, so
        // admitting the address here cannot admit an out-of-bounds read.
        if at > self.0.len() {
            return Err(bad());
        }
        Ok(at)
    }

    /// Read a serialized pointer field and resolve it relative to `base`.
    ///
    /// Serialized structures cannot store real pointers, so they tag offsets
    /// by setting the low bit — see `FcOffsetEncode` in `fcint.h`. A field
    /// with the bit clear was never relocated and must not be followed.
    ///
    /// Returns `None` for a null field, which is how chains terminate.
    pub fn follow(&self, base: usize, at: usize) -> Result<Option<usize>> {
        match self.offset(at)? {
            0 => Ok(None),
            raw if raw & 1 == 0 => Err(Error::NotAnOffset(raw)),
            raw => self.resolve(base, raw & !1).map(Some),
        }
    }

    /// A NUL-terminated UTF-8 string starting at `at`.
    pub fn str(&self, at: usize) -> Result<&'a str> {
        let rest = self.0.get(at..).ok_or(Error::BadString(at))?;
        let end = rest.iter().position(|&b| b == 0).ok_or(Error::BadString(at))?;
        std::str::from_utf8(&rest[..end]).map_err(|_| Error::BadString(at))
    }
}

/// The arithmetic every reader depends on, against values a corrupt file can
/// hold.
///
/// These matter most for a target this crate cannot run on. A count near
/// `i32::MAX` scaled by a pointer width overflows a 32-bit `usize` long
/// before any bounds check sees the address, and on x86_64 it simply does
/// not: the same corrupt cache is harmless here and a panic there. So the
/// bound is asserted directly rather than inferred from a walk that happens
/// not to crash.
#[cfg(test)]
mod tests {
    use super::Bytes;

    fn bytes(len: usize) -> Vec<u8> {
        vec![0u8; len]
    }

    #[test]
    fn an_array_that_would_overflow_is_rejected() {
        let buf = bytes(64);
        let data = Bytes::new(&buf);
        // count * stride overflows usize outright.
        assert!(data.array(0, usize::MAX, 2).is_err());
        assert!(data.array(0, usize::MAX / 2 + 1, 4).is_err());
        // The product fits but base + it does not.
        assert!(data.array(usize::MAX - 8, 4, 8).is_err());
        // Large but not overflowing, and still past the end.
        assert!(data.array(0, i32::MAX as usize, 8).is_err());
    }

    #[test]
    fn an_array_that_fits_is_accepted_to_the_last_byte() {
        let buf = bytes(64);
        let data = Bytes::new(&buf);
        assert_eq!(data.array(0, 8, 8).unwrap(), 8, "exactly the whole buffer");
        assert_eq!(data.array(32, 4, 8).unwrap(), 4, "exactly to the end");
        assert_eq!(data.array(64, 0, 8).unwrap(), 0, "empty at the very end");
        assert!(data.array(33, 4, 8).is_err(), "one byte past");
    }

    /// `resolve` takes a signed offset, so it has to survive both directions.
    #[test]
    fn resolving_a_hostile_offset_is_rejected() {
        let buf = bytes(64);
        let data = Bytes::new(&buf);
        assert!(data.resolve(0, i64::MIN).is_err(), "far negative");
        assert!(data.resolve(0, i64::MAX).is_err(), "far positive");
        assert!(data.resolve(0, -1).is_err(), "before the start");
        assert!(data.resolve(32, -33).is_err(), "past the start from inside");
        assert_eq!(data.resolve(32, -32).unwrap(), 0, "back to the start");
        assert_eq!(data.resolve(0, 64).unwrap(), 64, "one past the end is an address");
        assert!(data.resolve(0, 65).is_err(), "two past is not");
    }

    /// A count field is signed in the format, and a negative one is not a
    /// small number: it is a very large `usize` once cast.
    #[test]
    fn a_negative_count_is_rejected_rather_than_cast() {
        let mut buf = bytes(16);
        buf[0..4].copy_from_slice(&(-1i32).to_ne_bytes());
        let data = Bytes::new(&buf);
        assert!(data.count(0).is_err());
    }

    #[test]
    fn a_read_that_would_wrap_is_truncated_not_wrapped() {
        let buf = bytes(16);
        let data = Bytes::new(&buf);
        assert!(data.u32(usize::MAX).is_err());
        assert!(data.u32(usize::MAX - 2).is_err());
        assert!(data.i64(13).is_err(), "starts inside, ends outside");
        assert!(data.u32(12).is_ok(), "exactly the last four bytes");
    }
}
