//! Bounds-checked little-endian byte reader and writer helpers.
//! Every decoder facing the network goes through `Reader`, which never panics.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Truncated;

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }
    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }
    pub fn pos(&self) -> usize {
        self.pos
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], Truncated> {
        if self.remaining() < n {
            return Err(Truncated);
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], Truncated> {
        let mut a = [0u8; N];
        a.copy_from_slice(self.bytes(N)?);
        Ok(a)
    }
    pub fn u8(&mut self) -> Result<u8, Truncated> {
        Ok(self.bytes(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16, Truncated> {
        Ok(u16::from_le_bytes(self.array()?))
    }
    pub fn u32(&mut self) -> Result<u32, Truncated> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    pub fn u64(&mut self) -> Result<u64, Truncated> {
        Ok(u64::from_le_bytes(self.array()?))
    }
    pub fn rest(&mut self) -> &'a [u8] {
        let s = &self.buf[self.pos..];
        self.pos = self.buf.len();
        s
    }
}

pub trait Put {
    fn put_u8(&mut self, v: u8);
    fn put_u16(&mut self, v: u16);
    fn put_u32(&mut self, v: u32);
    fn put_u64(&mut self, v: u64);
    fn put(&mut self, b: &[u8]);
}

impl Put for Vec<u8> {
    fn put_u8(&mut self, v: u8) {
        self.push(v)
    }
    fn put_u16(&mut self, v: u16) {
        self.extend_from_slice(&v.to_le_bytes())
    }
    fn put_u32(&mut self, v: u32) {
        self.extend_from_slice(&v.to_le_bytes())
    }
    fn put_u64(&mut self, v: u64) {
        self.extend_from_slice(&v.to_le_bytes())
    }
    fn put(&mut self, b: &[u8]) {
        self.extend_from_slice(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_is_bounds_checked() {
        let mut r = Reader::new(&[1, 2, 3]);
        assert_eq!(r.u16(), Ok(0x0201));
        assert_eq!(r.u16(), Err(Truncated));
        assert_eq!(r.u8(), Ok(3));
        assert!(r.is_empty());
        assert_eq!(r.bytes(1), Err(Truncated));
        assert_eq!(r.rest(), &[] as &[u8]);
    }
}
