use std::fmt;

/// Something in a cache file did not hold up to inspection.
///
/// Every variant means the same class of thing: the bytes on disk did not
/// describe the structure they claimed to. None of them are recoverable —
/// the cache is either readable or it is not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The file is too short to contain the structure it points at.
    Truncated {
        /// Offset the read started at.
        at: usize,
        /// Bytes the read needed.
        len: usize,
    },
    /// The leading magic was not [`MAGIC_MMAP`](crate::cache::MAGIC_MMAP).
    ///
    /// A cache built in memory rather than mapped from disk carries a
    /// different magic and never appears in a file.
    BadMagic(u32),
    /// The cache is a format version this crate does not read.
    UnsupportedVersion(i32),
    /// The header's self-reported size disagreed with the file length.
    ///
    /// This is also what rejects a cache written by a build with a different
    /// word size, since the header is read at the wrong stride.
    SizeMismatch {
        /// Length the header claimed.
        declared: u64,
        /// Length the file actually is.
        actual: usize,
    },
    /// An offset pointed outside the file, or wrapped when resolved.
    BadOffset {
        /// Address the offset was taken relative to.
        base: usize,
        /// The offset itself.
        delta: i64,
    },
    /// A pointer field held a real pointer where an encoded offset was required.
    ///
    /// Serialized structures tag their offsets by setting the low bit; a
    /// clear low bit means the field was never relocated for the file.
    NotAnOffset(i64),
    /// A string was not valid UTF-8, or ran off the end without a terminator.
    BadString(usize),
    /// A count in the file was negative.
    BadCount(i32),
    /// A linked list did not terminate within the bounds the file allows.
    ///
    /// A corrupt `next` chain can point backwards and cycle forever; this is
    /// the budget that stops it.
    ChainTooLong,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { at, len } => {
                write!(f, "read of {len} bytes at {at} runs past end of cache")
            }
            Self::BadMagic(got) => write!(f, "bad cache magic {got:#010x}"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported cache version {v}"),
            Self::SizeMismatch { declared, actual } => {
                write!(f, "cache declares {declared} bytes but file is {actual}")
            }
            Self::BadOffset { base, delta } => {
                write!(f, "offset {delta} from {base} lands outside the cache")
            }
            Self::NotAnOffset(v) => write!(f, "pointer field {v:#x} is not an encoded offset"),
            Self::BadString(at) => write!(f, "malformed string at {at}"),
            Self::BadCount(n) => write!(f, "negative count {n}"),
            Self::ChainTooLong => write!(f, "value chain exceeds the bounds of the cache"),
        }
    }
}

impl std::error::Error for Error {}

/// A result whose error is always an [`Error`].
pub type Result<T> = std::result::Result<T, Error>;
