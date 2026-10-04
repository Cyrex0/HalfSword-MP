//! Records: the one binary representation of every piece of replicated data (HSMP-SHM ABI 2,
//! protocol v6). A record is a `#[repr(C)]` plain-old-data struct declared once with
//! [`ipc_pod!`](crate::ipc_pod) in `schema/<domain>.rs`. The same bytes are
//!
//! - written by the game (through HSMPNative) into a shared-memory slot, blob or ring record,
//! - copied by the sidecar into a network message behind an 8-byte [`crate::wire::WireHdr`]
//!   (kind, aux, peer) — no decode, no re-encode,
//! - validated and read in place by the server (and the receiving sidecar, and the game).
//!
//! A record is either *fixed* (one struct) or *variable*: a fixed head struct followed by
//! `count` rows of one row struct. In shared memory a variable record lives in a
//! [`VarBuf`] (head + the full row capacity); on the wire and in ring records only the head
//! and the `count` used rows travel. The prefix of a `VarBuf` IS the wire payload.
//!
//! Untrusted bytes (network, a scribbled segment) are accepted only through [`view`] /
//! [`VarBuf::load`], which check the size exactly, the row count against the capacity, every
//! float for finiteness, every [`Str`](crate::layout::Str) for canonical UTF-8, every
//! [`Bool`](crate::layout::Bool) for 0/1, and then the record's own semantic check
//! ([`Record::check`]). No `unsafe` here: bytemuck does the casts.

use std::borrow::Cow;

use crate::layout::{IpcType, Pod, TypeDesc};

/// Why bytes were refused as a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invalid {
    /// Fewer bytes than the head needs.
    Short { need: usize, got: usize },
    /// The size does not match head + count rows exactly.
    Size { want: usize, got: usize },
    /// The row count is over the record's capacity.
    Rows { n: usize, max: usize },
    /// A float field is NaN or infinite.
    Float(&'static str),
    /// A string field is not canonical (bad UTF-8, or bytes after the NUL padding).
    Str(&'static str),
    /// A boolean field is neither 0 nor 1.
    Bool(&'static str),
    /// A field is outside its allowed range (enum code, bound, world limit, ...).
    Range(&'static str),
    /// The message kind is not the expected one / unknown.
    Kind(u16),
    /// The 8-byte wire header is missing.
    Header,
}

impl Invalid {
    pub fn as_str(&self) -> &'static str {
        match self {
            Invalid::Short { .. } => "short",
            Invalid::Size { .. } => "size",
            Invalid::Rows { .. } => "rows",
            Invalid::Float(_) => "float",
            Invalid::Str(_) => "str",
            Invalid::Bool(_) => "bool",
            Invalid::Range(_) => "range",
            Invalid::Kind(_) => "kind",
            Invalid::Header => "header",
        }
    }
    /// The offending field, if any.
    pub fn field(&self) -> Option<&'static str> {
        match self {
            Invalid::Float(f) | Invalid::Str(f) | Invalid::Bool(f) | Invalid::Range(f) => Some(f),
            _ => None,
        }
    }
}

impl core::fmt::Display for Invalid {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Invalid::Short { need, got } => write!(f, "short record ({got} of {need} bytes)"),
            Invalid::Size { want, got } => write!(f, "record size {got}, want {want}"),
            Invalid::Rows { n, max } => write!(f, "{n} rows, capacity {max}"),
            Invalid::Float(x) => write!(f, "non-finite float in `{x}`"),
            Invalid::Str(x) => write!(f, "malformed string in `{x}`"),
            Invalid::Bool(x) => write!(f, "malformed bool in `{x}`"),
            Invalid::Range(x) => write!(f, "`{x}` out of range"),
            Invalid::Kind(k) => write!(f, "unexpected kind {k:#06x}"),
            Invalid::Header => f.write_str("missing wire header"),
        }
    }
}

impl std::error::Error for Invalid {}

/// The row type of a fixed record.
pub type NoRow = [u64; 0];

/// A record type: its kind, name, row layout and semantic check. Implement with [`record!`].
pub trait Record: Pod + bytemuck::Pod + IpcType {
    /// The kind id (high byte = domain). The same id on the wire and in ring records.
    const KIND: u16;
    /// Lua-facing name (`IPC.send("damage", t)`), also the tap / dump name.
    const NAME: &'static str;
    /// Row type (`NoRow` for a fixed record).
    type Row: Pod + bytemuck::Pod + IpcType;
    /// Row capacity (0 for a fixed record).
    const MAX_ROWS: usize;
    /// The head's row-count field (`None` for a fixed record).
    const COUNT_FIELD: Option<&'static str>;
    /// Rows the head claims (unclamped; [`view`] refuses more than `MAX_ROWS`).
    fn rows(&self) -> usize;
    fn set_rows(&mut self, n: usize);
    /// Semantic check of the head (ranges, enum codes); after the generic checks.
    fn check(&self) -> Result<(), Invalid>;
    /// Semantic check of one row.
    fn check_row(&self, row: &Self::Row) -> Result<(), Invalid>;
}

/// Bytes of a payload for `head` with `n` rows.
#[inline]
pub const fn payload_len<R: Record>(n: usize) -> usize {
    core::mem::size_of::<R>() + n * core::mem::size_of::<R::Row>()
}

/// Declare a [`Record`] impl.
///
/// ```ignore
/// record!(RootState, kind = K_ROOT, name = "root");
/// record!(RootState, kind = K_ROOT, name = "root", check = check_root);
/// record!(WorldStateHead, kind = K_WORLD_STATE, name = "world_state",
///         rows = WorldObj, count = n, max = 64, check = check_ws, check_row = check_obj);
/// ```
#[macro_export]
macro_rules! record {
    ($t:ty, kind = $kind:expr, name = $name:expr $(, check = $check:path)? $(,)?) => {
        impl $crate::record::Record for $t {
            const KIND: u16 = $kind;
            const NAME: &'static str = $name;
            type Row = $crate::record::NoRow;
            const MAX_ROWS: usize = 0;
            const COUNT_FIELD: Option<&'static str> = None;
            #[inline] fn rows(&self) -> usize { 0 }
            #[inline] fn set_rows(&mut self, _n: usize) {}
            #[inline] fn check(&self) -> Result<(), $crate::record::Invalid> {
                $( return $check(self); )?
                #[allow(unreachable_code)] Ok(())
            }
            #[inline] fn check_row(&self, _r: &Self::Row) -> Result<(), $crate::record::Invalid> { Ok(()) }
        }
    };
    ($t:ty, kind = $kind:expr, name = $name:expr, rows = $row:ty, count = $count:ident, max = $max:expr
     $(, check = $check:path)? $(, check_row = $check_row:path)? $(,)?) => {
        impl $crate::record::Record for $t {
            const KIND: u16 = $kind;
            const NAME: &'static str = $name;
            type Row = $row;
            const MAX_ROWS: usize = $max;
            const COUNT_FIELD: Option<&'static str> = Some(stringify!($count));
            #[inline] fn rows(&self) -> usize { self.$count as usize }
            #[inline] fn set_rows(&mut self, n: usize) { self.$count = n as _; }
            #[inline] fn check(&self) -> Result<(), $crate::record::Invalid> {
                $( return $check(self); )?
                #[allow(unreachable_code)] Ok(())
            }
            #[inline] fn check_row(&self, _r: &Self::Row) -> Result<(), $crate::record::Invalid> {
                $( return $check_row(self, _r); )?
                #[allow(unreachable_code)] Ok(())
            }
        }
        const _: () = {
            // Rows follow the head directly: the head size must keep them aligned.
            assert!(core::mem::size_of::<$t>() % core::mem::align_of::<$row>() == 0);
            assert!(core::mem::size_of::<$row>() > 0);
        };
    };
}

// ---- generic (descriptor-driven) checks --------------------------------------------------

/// Check every float (finite), string (canonical) and bool (0/1) in `bytes` laid out as `d`.
/// `bytes` must be at least `d.size()` long (callers check sizes first).
pub fn check_desc(bytes: &[u8], d: &TypeDesc) -> Result<(), Invalid> {
    walk(bytes, d, "")
}

fn walk(b: &[u8], d: &TypeDesc, field: &'static str) -> Result<(), Invalid> {
    match d {
        TypeDesc::Prim { name, size, .. } => {
            if *name == "f32" {
                let v = f32::from_le_bytes(b[..4].try_into().map_err(|_| Invalid::Float(field))?);
                if !v.is_finite() {
                    return Err(Invalid::Float(field));
                }
            } else if *name == "f64" {
                let v = f64::from_le_bytes(b[..8].try_into().map_err(|_| Invalid::Float(field))?);
                if !v.is_finite() {
                    return Err(Invalid::Float(field));
                }
            }
            let _ = size;
            Ok(())
        }
        TypeDesc::Array { elem, len } => {
            if !needs_check(elem) {
                return Ok(());
            }
            let s = elem.size();
            for i in 0..*len {
                walk(&b[i * s..(i + 1) * s], elem, field)?;
            }
            Ok(())
        }
        TypeDesc::Struct { fields, .. } => {
            for f in *fields {
                let sz = f.ty.size();
                walk(&b[f.offset..f.offset + sz], f.ty, f.name)?;
            }
            Ok(())
        }
        TypeDesc::Str { cap } => {
            let s = &b[..*cap];
            let n = s.iter().position(|&x| x == 0).unwrap_or(*cap);
            if core::str::from_utf8(&s[..n]).is_err() || s[n..].iter().any(|&x| x != 0) {
                return Err(Invalid::Str(field));
            }
            Ok(())
        }
        TypeDesc::Bool => {
            if b[0] > 1 {
                return Err(Invalid::Bool(field));
            }
            Ok(())
        }
    }
}

/// Does a value of this shape contain anything [`check_desc`] looks at?
pub const fn needs_check(d: &TypeDesc) -> bool {
    match d {
        TypeDesc::Prim { name, .. } => {
            let n = name.as_bytes();
            n.len() == 3 && n[0] == b'f'
        }
        TypeDesc::Array { elem, .. } => needs_check(elem),
        TypeDesc::Struct { fields, .. } => {
            let mut i = 0;
            while i < fields.len() {
                if needs_check(fields[i].ty) {
                    return true;
                }
                i += 1;
            }
            false
        }
        TypeDesc::Str { .. } | TypeDesc::Bool => true,
    }
}

/// Clamp `x` into a finite value (non-finite -> `or`). For writers.
#[inline]
pub fn finite_or(x: f32, or: f32) -> f32 {
    if x.is_finite() {
        x
    } else {
        or
    }
}

// ---- views ---------------------------------------------------------------------------------

/// A head borrowed from the input (aligned input) or copied (unaligned input).
pub enum Ref<'a, T> {
    Borrowed(&'a T),
    Owned(T),
}

impl<T> core::ops::Deref for Ref<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        match self {
            Ref::Borrowed(t) => t,
            Ref::Owned(t) => t,
        }
    }
}

impl<T: core::fmt::Debug> core::fmt::Debug for Ref<'_, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        (**self).fmt(f)
    }
}

/// A validated record: the head and its rows, zero-copy when the input is aligned.
#[derive(Debug)]
pub struct View<'a, R: Record> {
    pub head: Ref<'a, R>,
    pub rows: Cow<'a, [R::Row]>,
}

impl<R: Record> View<'_, R> {
    /// An owned copy of the head.
    pub fn head(&self) -> R {
        *self.head
    }
}

/// Validate `payload` as an `R` and view it in place. This is the only way untrusted bytes
/// become a record: exact size, row count within capacity, generic checks, then `R::check`.
pub fn view<R: Record>(payload: &[u8]) -> Result<View<'_, R>, Invalid> {
    let hs = core::mem::size_of::<R>();
    if payload.len() < hs {
        return Err(Invalid::Short { need: hs, got: payload.len() });
    }
    let (hb, rb) = payload.split_at(hs);
    let head: Ref<'_, R> = match bytemuck::try_from_bytes::<R>(hb) {
        Ok(r) => Ref::Borrowed(r),
        Err(_) => Ref::Owned(bytemuck::pod_read_unaligned::<R>(hb)),
    };
    let n = head.rows();
    if n > R::MAX_ROWS {
        return Err(Invalid::Rows { n, max: R::MAX_ROWS });
    }
    let want = payload_len::<R>(n);
    if payload.len() != want {
        return Err(Invalid::Size { want, got: payload.len() });
    }
    check_desc(hb, &R::DESC)?;
    head.check()?;
    let rows: Cow<'_, [R::Row]> = if n == 0 {
        Cow::Borrowed(&[])
    } else {
        match bytemuck::try_cast_slice::<u8, R::Row>(rb) {
            Ok(s) => Cow::Borrowed(s),
            Err(_) => {
                let rs = core::mem::size_of::<R::Row>();
                Cow::Owned((0..n).map(|i| bytemuck::pod_read_unaligned::<R::Row>(&rb[i * rs..(i + 1) * rs])).collect())
            }
        }
    };
    if needs_check(&<R::Row as IpcType>::DESC) || core::mem::size_of::<R::Row>() > 0 {
        let rs = core::mem::size_of::<R::Row>();
        for (i, r) in rows.iter().enumerate() {
            if needs_check(&<R::Row as IpcType>::DESC) {
                check_desc(&rb[i * rs..(i + 1) * rs], &<R::Row as IpcType>::DESC)?;
            }
            head.check_row(r)?;
        }
    }
    Ok(View { head, rows })
}

/// Validate a record value built in process (writers' self-check, tests).
pub fn check<R: Record>(head: &R, rows: &[R::Row]) -> Result<(), Invalid> {
    if rows.len() != head.rows() {
        return Err(Invalid::Size { want: payload_len::<R>(head.rows()), got: payload_len::<R>(rows.len()) });
    }
    if rows.len() > R::MAX_ROWS {
        return Err(Invalid::Rows { n: rows.len(), max: R::MAX_ROWS });
    }
    check_desc(bytemuck::bytes_of(head), &R::DESC)?;
    head.check()?;
    for r in rows {
        check_desc(bytemuck::bytes_of(r), &<R::Row as IpcType>::DESC)?;
        head.check_row(r)?;
    }
    Ok(())
}

/// Append the payload of `head` + `rows` to `out` (the head's count is set to `rows.len()`,
/// clamped to the capacity).
pub fn write_payload<R: Record>(out: &mut Vec<u8>, head: &R, rows: &[R::Row]) {
    let n = rows.len().min(R::MAX_ROWS);
    let mut h = *head;
    h.set_rows(n);
    out.extend_from_slice(bytemuck::bytes_of(&h));
    out.extend_from_slice(bytemuck::cast_slice(&rows[..n]));
}

/// A payload as an owned `Vec`.
pub fn to_payload<R: Record>(head: &R, rows: &[R::Row]) -> Vec<u8> {
    let mut v = Vec::with_capacity(payload_len::<R>(rows.len()));
    write_payload(&mut v, head, rows);
    v
}

// ---- VarBuf: a variable record with its full row capacity (shared memory) ------------------

/// Head + the full row capacity, for slots and blobs. Its first [`VarBuf::payload_len`] bytes
/// are exactly the wire / ring payload, so writers and readers copy them as they are.
#[repr(C)]
pub struct VarBuf<R: Record, const CAP: usize> {
    pub head: R,
    pub rows: [R::Row; CAP],
}

impl<R: Record, const CAP: usize> Clone for VarBuf<R, CAP> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<R: Record, const CAP: usize> Copy for VarBuf<R, CAP> {}

// SAFETY: repr(C) of Pod fields; the head size is a multiple of the row alignment (`record!`
// asserts it) and both sizes are multiples of 8 (`ipc_pod!`), so there is no padding.
unsafe impl<R: Record, const CAP: usize> Pod for VarBuf<R, CAP> {}
unsafe impl<R: Record, const CAP: usize> bytemuck::Zeroable for VarBuf<R, CAP> {}
unsafe impl<R: Record, const CAP: usize> bytemuck::Pod for VarBuf<R, CAP> {}

unsafe impl<R: Record, const CAP: usize> IpcType for VarBuf<R, CAP> {
    const DESC: TypeDesc = TypeDesc::Struct {
        name: "VarBuf",
        size: core::mem::size_of::<Self>(),
        align: core::mem::align_of::<Self>(),
        pod: true,
        fields: &[
            crate::layout::FieldDesc { name: "head", offset: 0, ty: &R::DESC },
            crate::layout::FieldDesc { name: "rows", offset: core::mem::size_of::<R>(), ty: &<[R::Row; CAP] as IpcType>::DESC },
        ],
    };
}

impl<R: Record, const CAP: usize> VarBuf<R, CAP> {
    const LAYOUT_OK: () = {
        assert!(CAP <= R::MAX_ROWS || R::MAX_ROWS == 0, "VarBuf capacity over the record's MAX_ROWS");
        assert!(core::mem::size_of::<R>().is_multiple_of(8));
        assert!(core::mem::size_of::<Self>() == core::mem::size_of::<R>() + CAP * core::mem::size_of::<R::Row>());
    };

    /// A zeroed buffer on the heap (blobs can be large).
    pub fn new_boxed() -> Box<Self> {
        let () = Self::LAYOUT_OK;
        crate::layout::boxed_zeroed::<Self>()
    }

    /// Rows in use (the head's count, clamped to the capacity).
    #[inline]
    pub fn n(&self) -> usize {
        self.head.rows().min(CAP)
    }

    /// Bytes of the payload (head + used rows).
    #[inline]
    pub fn payload_len(&self) -> usize {
        payload_len::<R>(self.n())
    }

    /// The payload: the buffer's prefix, as it goes on the wire.
    #[inline]
    pub fn payload(&self) -> &[u8] {
        let () = Self::LAYOUT_OK;
        &bytemuck::bytes_of(self)[..self.payload_len()]
    }

    /// The used rows.
    #[inline]
    pub fn used(&self) -> &[R::Row] {
        &self.rows[..self.n()]
    }

    /// Set head and rows (rows beyond the capacity are dropped; the count follows).
    pub fn set(&mut self, head: &R, rows: &[R::Row]) {
        let n = rows.len().min(CAP);
        self.head = *head;
        self.head.set_rows(n);
        self.rows[..n].copy_from_slice(&rows[..n]);
    }

    /// Validate an untrusted payload and copy it in. On error `self` is unchanged.
    pub fn load(&mut self, payload: &[u8]) -> Result<(), Invalid> {
        let v = view::<R>(payload)?;
        if v.rows.len() > CAP {
            return Err(Invalid::Rows { n: v.rows.len(), max: CAP });
        }
        let head = v.head();
        let n = v.rows.len();
        self.rows[..n].copy_from_slice(&v.rows);
        self.head = head;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Bool, Str};

    crate::ipc_pod! {
        pub struct TRow {
            pub id: u32,
            pub v: f32,
        }
        pub struct THead {
            pub a: u32,
            pub n: u16,
            pub ok: Bool,
            pub _r: u8,
            pub name: Str<16>,
            pub x: f64,
        }
        pub struct TFixed {
            pub p: [f32; 3],
            pub mode: u8,
            pub _r: [u8; 3],
        }
    }

    fn check_fixed(t: &TFixed) -> Result<(), Invalid> {
        if t.mode > 3 {
            return Err(Invalid::Range("mode"));
        }
        Ok(())
    }
    fn check_row(_h: &THead, r: &TRow) -> Result<(), Invalid> {
        if r.id == 0 {
            return Err(Invalid::Range("id"));
        }
        Ok(())
    }

    crate::record!(TFixed, kind = 0x7f01, name = "t_fixed", check = check_fixed);
    crate::record!(THead, kind = 0x7f02, name = "t_var", rows = TRow, count = n, max = 8, check_row = check_row);

    #[test]
    fn fixed_round_trip_and_checks() {
        let t = TFixed { p: [1.0, 2.0, 3.0], mode: 2, _r: [0; 3] };
        let p = to_payload(&t, &[]);
        assert_eq!(p.len(), 16);
        let v = view::<TFixed>(&p).unwrap();
        assert_eq!(v.head(), t);
        assert!(view::<TFixed>(&p[..15]).is_err());
        let mut long = p.clone();
        long.push(0);
        assert!(matches!(view::<TFixed>(&long), Err(Invalid::Size { .. })));
        let mut bad = t;
        bad.p[1] = f32::NAN;
        assert_eq!(view::<TFixed>(&to_payload(&bad, &[])).unwrap_err(), Invalid::Float("p"));
        bad = t;
        bad.mode = 9;
        assert_eq!(view::<TFixed>(&to_payload(&bad, &[])).unwrap_err(), Invalid::Range("mode"));
    }

    #[test]
    fn var_round_trip_unaligned_and_hostile_counts() {
        let h = THead { a: 7, n: 0, ok: Bool::TRUE, _r: 0, name: Str::new("Willie"), x: 1.5 };
        let rows = [TRow { id: 1, v: 0.5 }, TRow { id: 2, v: -1.0 }];
        let p = to_payload(&h, &rows);
        assert_eq!(p.len(), 32 + 16);
        let v = view::<THead>(&p).unwrap();
        assert_eq!(v.head.n, 2);
        assert_eq!(&*v.rows, &rows);
        assert_eq!(v.head.name, "Willie");
        // Unaligned input: copied, same result.
        let mut un = vec![0u8];
        un.extend_from_slice(&p);
        let v2 = view::<THead>(&un[1..]).unwrap();
        assert_eq!(&*v2.rows, &rows);
        // Count over capacity / count not matching the length.
        let mut big = h;
        big.n = 9;
        let mut bp = bytemuck::bytes_of(&big).to_vec();
        bp.extend(std::iter::repeat_n(1u8, 9 * 8));
        assert!(matches!(view::<THead>(&bp), Err(Invalid::Rows { n: 9, max: 8 })));
        let mut short = p.clone();
        short.truncate(40);
        assert!(matches!(view::<THead>(&short), Err(Invalid::Size { .. })));
        // Bad string / bool / row check.
        let mut s = p.clone();
        s[16 + 6] = b'x'; // a byte after the NUL padding
        assert_eq!(view::<THead>(&s).unwrap_err(), Invalid::Str("name"));
        let mut b = p.clone();
        b[6] = 2;
        assert_eq!(view::<THead>(&b).unwrap_err(), Invalid::Bool("ok"));
        let zero_row = to_payload(&h, &[TRow { id: 0, v: 0.0 }]);
        assert_eq!(view::<THead>(&zero_row).unwrap_err(), Invalid::Range("id"));
    }

    #[test]
    fn varbuf_prefix_is_the_payload() {
        let mut b = VarBuf::<THead, 8>::new_boxed();
        let h = THead { a: 1, n: 0, ok: Bool::FALSE, _r: 0, name: Str::new("x"), x: 0.0 };
        b.set(&h, &[TRow { id: 5, v: 1.0 }; 3]);
        assert_eq!(b.n(), 3);
        assert_eq!(b.payload(), &to_payload(&h, &[TRow { id: 5, v: 1.0 }; 3])[..]);
        let mut c = VarBuf::<THead, 8>::new_boxed();
        c.load(b.payload()).unwrap();
        assert_eq!(c.payload(), b.payload());
        assert!(c.load(&[1, 2, 3]).is_err());
        assert_eq!(c.payload(), b.payload(), "a refused load leaves the buffer unchanged");
    }

    #[test]
    fn str_truncates_on_char_boundaries() {
        let s = Str::<4>::new("aé€");
        assert_eq!(s.as_str(), Some("aé"));
        assert!(s.is_canonical());
        assert_eq!(Str::<4>::new("abcd").len(), 4);
        assert!(Str::<4>([0xff, 0, 0, 0]).as_str().is_none());
    }
}
