//! Layout descriptors: every shared struct describes itself (name, fields, offsets, sizes),
//! so the layout hash, the C header and the Lua schema are all derived from one source.
//!
//! `ipc_pod!` declares a plain-old-data payload struct (no implicit padding, `Copy`, `Pod`).
//! `ipc_layout!` declares a non-POD shared struct (it may hold atomics, `UnsafeCell`s and
//! trailing alignment padding), for the header and the segment regions.

use core::sync::atomic::{AtomicU32, AtomicU64};

/// The shape of one type in the shared segment.
#[derive(Debug)]
pub enum TypeDesc {
    /// A scalar. `c` is the C spelling.
    Prim { name: &'static str, c: &'static str, size: usize },
    /// `[elem; len]`.
    Array { elem: &'static TypeDesc, len: usize },
    /// A `#[repr(C)]` struct.
    Struct { name: &'static str, size: usize, align: usize, pod: bool, fields: &'static [FieldDesc] },
    /// [`Str<N>`]: UTF-8, NUL-padded to `cap` bytes (C: `char[cap]`; Lua: a string).
    Str { cap: usize },
    /// [`Bool`]: one byte, 0 or 1 (Lua: a boolean).
    Bool,
}

/// One field of a struct.
#[derive(Debug)]
pub struct FieldDesc {
    pub name: &'static str,
    pub offset: usize,
    pub ty: &'static TypeDesc,
}

impl TypeDesc {
    pub const fn size(&self) -> usize {
        match self {
            TypeDesc::Prim { size, .. } => *size,
            TypeDesc::Array { elem, len } => elem.size() * *len,
            TypeDesc::Struct { size, .. } => *size,
            TypeDesc::Str { cap } => *cap,
            TypeDesc::Bool => 1,
        }
    }
    pub const fn name(&self) -> &'static str {
        match self {
            TypeDesc::Prim { name, .. } => name,
            TypeDesc::Array { .. } => "[]",
            TypeDesc::Struct { name, .. } => name,
            TypeDesc::Str { .. } => "str",
            TypeDesc::Bool => "bool",
        }
    }
    /// Struct fields (empty for scalars and arrays).
    pub const fn fields(&self) -> &'static [FieldDesc] {
        match self {
            TypeDesc::Struct { fields, .. } => fields,
            _ => &[],
        }
    }
    /// Look a field up by name.
    pub fn field(&self, name: &str) -> Option<&'static FieldDesc> {
        self.fields().iter().find(|f| f.name == name)
    }
}

/// A type that can live in the shared segment and describes its own layout.
///
/// # Safety
/// `DESC` must describe the type's real `#[repr(C)]` layout exactly.
pub unsafe trait IpcType: Sized {
    const DESC: TypeDesc;
}

/// Plain old data: every bit pattern is a valid value, no padding bytes, no pointers,
/// no interior mutability. Payloads copied in and out of seqlock slots, triple buffers
/// and ring records are `Pod`.
///
/// # Safety
/// The type must be `#[repr(C)]`, contain only `Pod` fields and have no padding.
pub unsafe trait Pod: Copy + 'static + IpcType {
    /// The all-zero value.
    fn zeroed() -> Self {
        // SAFETY: Pod types accept every bit pattern, including all zeros.
        unsafe { core::mem::zeroed() }
    }
}

macro_rules! prim {
    ($($t:ty => $c:expr),* $(,)?) => {$(
        unsafe impl IpcType for $t {
            const DESC: TypeDesc = TypeDesc::Prim { name: stringify!($t), c: $c, size: core::mem::size_of::<$t>() };
        }
    )*};
}
prim!(u8 => "uint8_t", u16 => "uint16_t", u32 => "uint32_t", u64 => "uint64_t",
      i8 => "int8_t", i16 => "int16_t", i32 => "int32_t", i64 => "int64_t",
      f32 => "float", f64 => "double");
unsafe impl IpcType for AtomicU32 {
    const DESC: TypeDesc = TypeDesc::Prim { name: "u32", c: "uint32_t", size: 4 };
}
unsafe impl IpcType for AtomicU64 {
    const DESC: TypeDesc = TypeDesc::Prim { name: "u64", c: "uint64_t", size: 8 };
}
macro_rules! pod_prim { ($($t:ty),*) => {$( unsafe impl Pod for $t {} )*}; }
pod_prim!(u8, u16, u32, u64, i8, i16, i32, i64, f32, f64);

unsafe impl<T: IpcType, const N: usize> IpcType for [T; N] {
    const DESC: TypeDesc = TypeDesc::Array { elem: &T::DESC, len: N };
}
unsafe impl<T: Pod, const N: usize> Pod for [T; N] {}

// ---- fixed strings and booleans ----------------------------------------------------------

/// A fixed-capacity UTF-8 string: the bytes, then NUL padding up to `N`. A string of exactly
/// `N` bytes has no terminator. Canonical form (checked by [`crate::record`] validation):
/// valid UTF-8 up to the first NUL, and only NULs after it.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Str<const N: usize>(pub [u8; N]);

unsafe impl<const N: usize> IpcType for Str<N> {
    const DESC: TypeDesc = TypeDesc::Str { cap: N };
}
// SAFETY: a transparent byte array.
unsafe impl<const N: usize> Pod for Str<N> {}
unsafe impl<const N: usize> bytemuck::Zeroable for Str<N> {}
unsafe impl<const N: usize> bytemuck::Pod for Str<N> {}

impl<const N: usize> Default for Str<N> {
    fn default() -> Self {
        Str([0; N])
    }
}

impl<const N: usize> Str<N> {
    pub const CAP: usize = N;

    /// `s` truncated on a char boundary to `N` bytes.
    pub fn new(s: &str) -> Self {
        let mut v = Str([0; N]);
        v.set(s);
        v
    }

    /// Replace the content with `s`, truncated on a char boundary to `N` bytes.
    /// Returns false if it had to truncate.
    pub fn set(&mut self, s: &str) -> bool {
        let mut n = s.len().min(N);
        while n > 0 && !s.is_char_boundary(n) {
            n -= 1;
        }
        self.0 = [0; N];
        self.0[..n].copy_from_slice(&s.as_bytes()[..n]);
        n == s.len()
    }

    /// Byte length (up to the first NUL).
    pub fn len(&self) -> usize {
        self.0.iter().position(|&b| b == 0).unwrap_or(N)
    }

    pub fn is_empty(&self) -> bool {
        N == 0 || self.0[0] == 0
    }

    /// The raw content bytes (up to the first NUL).
    pub fn bytes(&self) -> &[u8] {
        &self.0[..self.len()]
    }

    /// The content if it is valid UTF-8 (always, for a validated record).
    pub fn as_str(&self) -> Option<&str> {
        core::str::from_utf8(self.bytes()).ok()
    }

    /// The content, invalid UTF-8 replaced by U+FFFD.
    pub fn lossy(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(self.bytes())
    }

    /// Canonical: UTF-8 content, NULs only after it.
    pub fn is_canonical(&self) -> bool {
        let n = self.len();
        core::str::from_utf8(&self.0[..n]).is_ok() && self.0[n..].iter().all(|&b| b == 0)
    }
}

impl<const N: usize> core::fmt::Debug for Str<N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&*self.lossy(), f)
    }
}

impl<const N: usize> PartialEq<str> for Str<N> {
    fn eq(&self, o: &str) -> bool {
        self.bytes() == o.as_bytes()
    }
}
impl<const N: usize> PartialEq<&str> for Str<N> {
    fn eq(&self, o: &&str) -> bool {
        self.bytes() == o.as_bytes()
    }
}

/// A one-byte boolean: 0 = false, 1 = true; any other value fails validation.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Bool(pub u8);

unsafe impl IpcType for Bool {
    const DESC: TypeDesc = TypeDesc::Bool;
}
// SAFETY: a transparent byte.
unsafe impl Pod for Bool {}
unsafe impl bytemuck::Zeroable for Bool {}
unsafe impl bytemuck::Pod for Bool {}

impl Bool {
    pub const TRUE: Bool = Bool(1);
    pub const FALSE: Bool = Bool(0);
    #[inline]
    pub fn get(self) -> bool {
        self.0 != 0
    }
}

impl From<bool> for Bool {
    fn from(b: bool) -> Self {
        Bool(b as u8)
    }
}

impl core::fmt::Debug for Bool {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            0 => f.write_str("false"),
            1 => f.write_str("true"),
            n => write!(f, "Bool({n})"),
        }
    }
}

/// Declare a `#[repr(C)]` POD payload struct with a layout descriptor.
/// Fails to compile if the struct has implicit padding (add explicit `_pad` fields).
#[macro_export]
macro_rules! ipc_pod {
    ($(
        $(#[$m:meta])*
        pub struct $name:ident { $( $(#[$fm:meta])* pub $f:ident : $t:ty ),* $(,)? }
    )*) => {$(
        $(#[$m])*
        #[repr(C)]
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct $name { $( $(#[$fm])* pub $f: $t ),* }

        unsafe impl $crate::layout::IpcType for $name {
            const DESC: $crate::layout::TypeDesc = $crate::layout::TypeDesc::Struct {
                name: stringify!($name),
                size: core::mem::size_of::<$name>(),
                align: core::mem::align_of::<$name>(),
                pod: true,
                fields: &[ $( $crate::layout::FieldDesc {
                    name: stringify!($f),
                    offset: core::mem::offset_of!($name, $f),
                    ty: &<$t as $crate::layout::IpcType>::DESC,
                } ),* ],
            };
        }
        // SAFETY: repr(C), every field Pod, no padding (asserted below).
        unsafe impl $crate::layout::Pod for $name {}
        unsafe impl $crate::bytemuck::Zeroable for $name {}
        unsafe impl $crate::bytemuck::Pod for $name {}
        impl Default for $name { fn default() -> Self { <$name as $crate::layout::Pod>::zeroed() } }
        const _: () = {
            // No implicit padding: the field sizes add up to the struct size.
            assert!(0 $( + core::mem::size_of::<$t>() )* == core::mem::size_of::<$name>(),
                concat!("ipc_pod ", stringify!($name), " has implicit padding"));
            // Words are copied as u64 atomics: size and alignment must allow that.
            assert!(core::mem::size_of::<$name>() % 8 == 0,
                concat!("ipc_pod ", stringify!($name), " size must be a multiple of 8"));
        };
    )*};
}

/// Declare a `#[repr(C)]` shared struct that is not POD (atomics, cells, region alignment).
#[macro_export]
macro_rules! ipc_layout {
    ($(
        $(#[$m:meta])*
        pub struct $name:ident { $( $(#[$fm:meta])* $vis:vis $f:ident : $t:ty ),* $(,)? }
    )*) => {$(
        $(#[$m])*
        #[repr(C)]
        pub struct $name { $( $(#[$fm])* $vis $f: $t ),* }

        unsafe impl $crate::layout::IpcType for $name {
            const DESC: $crate::layout::TypeDesc = $crate::layout::TypeDesc::Struct {
                name: stringify!($name),
                size: core::mem::size_of::<$name>(),
                align: core::mem::align_of::<$name>(),
                pod: false,
                fields: &[ $( $crate::layout::FieldDesc {
                    name: stringify!($f),
                    offset: core::mem::offset_of!($name, $f),
                    ty: &<$t as $crate::layout::IpcType>::DESC,
                } ),* ],
            };
        }
    )*};
}

// ---- layout hash (FNV-1a 64, const) -------------------------------------------------------

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

const fn fnv_bytes(mut h: u64, b: &[u8]) -> u64 {
    let mut i = 0;
    while i < b.len() {
        h ^= b[i] as u64;
        h = h.wrapping_mul(FNV_PRIME);
        i += 1;
    }
    h
}
const fn fnv_u64(h: u64, v: u64) -> u64 {
    fnv_bytes(h, &v.to_le_bytes())
}

const fn hash_desc(mut h: u64, d: &TypeDesc) -> u64 {
    match d {
        TypeDesc::Prim { name, size, .. } => {
            h = fnv_bytes(h, b"P");
            h = fnv_bytes(h, name.as_bytes());
            fnv_u64(h, *size as u64)
        }
        TypeDesc::Array { elem, len } => {
            h = fnv_bytes(h, b"A");
            h = fnv_u64(h, *len as u64);
            hash_desc(h, elem)
        }
        TypeDesc::Struct { name, size, align, fields, .. } => {
            h = fnv_bytes(h, b"S");
            h = fnv_bytes(h, name.as_bytes());
            h = fnv_u64(h, *size as u64);
            h = fnv_u64(h, *align as u64);
            let mut i = 0;
            while i < fields.len() {
                let f = &fields[i];
                h = fnv_bytes(h, b"F");
                h = fnv_bytes(h, f.name.as_bytes());
                h = fnv_u64(h, f.offset as u64);
                h = hash_desc(h, f.ty);
                i += 1;
            }
            fnv_bytes(h, b"E")
        }
        TypeDesc::Str { cap } => {
            h = fnv_bytes(h, b"T");
            fnv_u64(h, *cap as u64)
        }
        TypeDesc::Bool => fnv_bytes(h, b"B"),
    }
}

/// FNV-1a 64 over the canonical description of `root` plus the ABI major. Any rename,
/// reorder, resize or retype anywhere below `root` changes it.
pub const fn layout_hash(root: &TypeDesc, abi_major: u16) -> u64 {
    let h = fnv_u64(FNV_OFFSET, abi_major as u64);
    hash_desc(h, root)
}

/// Copy `size_of::<T>()` bytes word by word with relaxed atomic loads. Never a data race
/// in the Rust memory model, even while another process writes the source.
///
/// # Safety
/// `src` must be valid for reads of `size_of::<T>()` bytes and 8-aligned; `T: Pod` with a
/// size that is a multiple of 8.
#[inline]
pub(crate) unsafe fn atomic_load_words<T: Pod>(src: *const T, dst: &mut T) {
    let n = core::mem::size_of::<T>() / 8;
    let s = src as *const AtomicU64;
    let d = dst as *mut T as *mut u64;
    for i in 0..n {
        // SAFETY: caller guarantees bounds and alignment; AtomicU64 has u64's layout.
        unsafe { *d.add(i) = (*s.add(i)).load(core::sync::atomic::Ordering::Relaxed) };
    }
}

/// The store counterpart of [`atomic_load_words`].
///
/// # Safety
/// `dst` must be valid for writes of `size_of::<T>()` bytes and 8-aligned.
#[inline]
pub(crate) unsafe fn atomic_store_words<T: Pod>(src: &T, dst: *mut T) {
    let n = core::mem::size_of::<T>() / 8;
    let s = src as *const T as *const u64;
    let d = dst as *const AtomicU64;
    for i in 0..n {
        // SAFETY: as above; `src` is a valid Pod value with no padding (ipc_pod! asserts).
        unsafe { (*d.add(i)).store(*s.add(i), core::sync::atomic::Ordering::Relaxed) };
    }
}

/// A zeroed `T` on the heap without building it on the stack first (blobs are up to 128 KiB).
pub fn boxed_zeroed<T: Pod>() -> Box<T> {
    let layout = std::alloc::Layout::new::<T>();
    if layout.size() == 0 {
        return Box::new(T::zeroed());
    }
    // SAFETY: non-zero size; all-zero is a valid T (Pod); the allocation has T's layout.
    unsafe {
        let p = std::alloc::alloc_zeroed(layout) as *mut T;
        if p.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        Box::from_raw(p)
    }
}

/// View a Pod value as its u64 words (Pod sizes are multiples of 8 and have no padding).
pub fn as_words<T: Pod>(v: &T) -> &[u64] {
    assert!(core::mem::size_of::<T>().is_multiple_of(8) && core::mem::align_of::<T>() <= 8 || core::mem::size_of::<T>().is_multiple_of(8));
    let n = core::mem::size_of::<T>() / 8;
    if !(v as *const T as usize).is_multiple_of(core::mem::align_of::<u64>()) {
        panic!("as_words: value not 8-aligned");
    }
    // SAFETY: Pod, size multiple of 8 (ipc_pod! asserts), alignment checked above, no padding.
    unsafe { core::slice::from_raw_parts(v as *const T as *const u64, n) }
}

/// Mutable word view of a Pod value (every bit pattern is a valid Pod).
pub fn as_words_mut<T: Pod>(v: &mut T) -> &mut [u64] {
    let n = core::mem::size_of::<T>() / 8;
    if !(v as *mut T as usize).is_multiple_of(core::mem::align_of::<u64>()) {
        panic!("as_words_mut: value not 8-aligned");
    }
    // SAFETY: as in `as_words`; any u64 pattern written leaves a valid Pod.
    unsafe { core::slice::from_raw_parts_mut(v as *mut T as *mut u64, n) }
}

/// A zero-initialised shared primitive on the heap (all-zero is the valid initial state of
/// every segment type: atomics, cells over Pod, Pod arrays).
///
/// # Safety
/// `T` must be valid when all-zero (every segment struct is).
pub unsafe fn boxed_zeroed_raw<T>() -> Box<T> {
    let layout = std::alloc::Layout::new::<T>();
    assert!(layout.size() > 0);
    // SAFETY: non-zero size; the caller guarantees all-zero validity.
    unsafe {
        let p = std::alloc::alloc_zeroed(layout) as *mut T;
        if p.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        Box::from_raw(p)
    }
}
