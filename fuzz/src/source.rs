//! Sources that watch the parser read.

use std::io::{self, Read, Seek, SeekFrom};

/// Counts what the parser takes from a source, the way the parser's own
/// budget counts it (non-empty reads and every seek are one operation each),
/// so a read that bypassed the budget would show up here.
pub struct Counting<S> {
    inner: S,
    pub bytes: u64,
    pub operations: u64,
}

impl<S> Counting<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            bytes: 0,
            operations: 0,
        }
    }
}

impl<S: Read> Read for Counting<S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if !buf.is_empty() {
            self.operations += 1;
        }
        let n = self.inner.read(buf)?;
        self.bytes += n as u64;
        Ok(n)
    }
}

impl<S: Seek> Seek for Counting<S> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.operations += 1;
        self.inner.seek(to)
    }
}

/// A huge virtual file: `head` at the start, `tail` at the end, zeros in
/// between. A parser that scans instead of seeking, or trusts a length
/// field to size an allocation, reads or allocates far past its budget here.
pub struct Sparse {
    head: Vec<u8>,
    tail: Vec<u8>,
    len: u64,
    at: u64,
}

impl Sparse {
    pub fn new(head: Vec<u8>, tail: Vec<u8>, len: u64) -> Self {
        let len = len.max((head.len() + tail.len()) as u64);
        Self {
            head,
            tail,
            len,
            at: 0,
        }
    }
}

impl Read for Sparse {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut n = 0;
        let tail_start = self.len - self.tail.len() as u64;
        while n < buf.len() && self.at < self.len {
            let at = self.at;
            let byte = if at < self.head.len() as u64 {
                self.head[at as usize]
            } else if at >= tail_start {
                self.tail[(at - tail_start) as usize]
            } else {
                // Fill a run of zeros up to the tail in one step.
                let run = ((tail_start - at) as usize).min(buf.len() - n);
                buf[n..n + run].fill(0);
                n += run;
                self.at += run as u64;
                continue;
            };
            buf[n] = byte;
            n += 1;
            self.at += 1;
        }
        Ok(n)
    }
}

impl Seek for Sparse {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let target = match to {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.at.checked_add_signed(d),
        };
        let target = target
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before start"))?;
        self.at = target;
        Ok(target)
    }
}
