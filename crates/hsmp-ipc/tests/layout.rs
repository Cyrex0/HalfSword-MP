//! Layout hash and descriptors.

use hsmp_ipc::layout::{layout_hash, FieldDesc, IpcType, TypeDesc};
use hsmp_ipc::schema::pose::{PeerPlay, PoseBuf};
use hsmp_ipc::schema::Stamped;
use hsmp_ipc::{Segment, LAYOUT_HASH};

const U32: TypeDesc = TypeDesc::Prim { name: "u32", c: "uint32_t", size: 4 };
const F32: TypeDesc = TypeDesc::Prim { name: "f32", c: "float", size: 4 };

const fn s(fields: &'static [FieldDesc], name: &'static str) -> TypeDesc {
    TypeDesc::Struct { name, size: 8, align: 4, pod: true, fields }
}

#[test]
fn hash_changes_on_any_edit() {
    static BASE: [FieldDesc; 2] = [FieldDesc { name: "a", offset: 0, ty: &U32 }, FieldDesc { name: "b", offset: 4, ty: &U32 }];
    static RENAMED: [FieldDesc; 2] = [FieldDesc { name: "a", offset: 0, ty: &U32 }, FieldDesc { name: "c", offset: 4, ty: &U32 }];
    static REORDERED: [FieldDesc; 2] = [FieldDesc { name: "b", offset: 0, ty: &U32 }, FieldDesc { name: "a", offset: 4, ty: &U32 }];
    static RETYPED: [FieldDesc; 2] = [FieldDesc { name: "a", offset: 0, ty: &U32 }, FieldDesc { name: "b", offset: 4, ty: &F32 }];
    let base = layout_hash(&s(&BASE, "X"), 1);
    assert_eq!(base, layout_hash(&s(&BASE, "X"), 1));
    for other in [
        layout_hash(&s(&RENAMED, "X"), 1),
        layout_hash(&s(&REORDERED, "X"), 1),
        layout_hash(&s(&RETYPED, "X"), 1),
        layout_hash(&s(&BASE, "Y"), 1),
        layout_hash(&s(&BASE, "X"), 2),
    ] {
        assert_ne!(base, other);
    }
}

#[test]
fn segment_hash_is_the_published_constant() {
    assert_eq!(layout_hash(&<Segment as IpcType>::DESC, hsmp_ipc::ABI_MAJOR), LAYOUT_HASH);
}

#[test]
fn descriptors_match_the_types() {
    assert_eq!(<Stamped<PoseBuf> as IpcType>::DESC.size(), core::mem::size_of::<Stamped<PoseBuf>>());
    assert_eq!(<PeerPlay as IpcType>::DESC.size(), core::mem::size_of::<PeerPlay>());
    let f = <PeerPlay as IpcType>::DESC.field("b").unwrap();
    assert_eq!(f.offset, core::mem::offset_of!(PeerPlay, b));
    assert_eq!(f.ty.size(), 25 * 13 * 4);
    for d in hsmp_ipc::schema::PODS {
        let sum: usize = d.fields().iter().map(|f| f.ty.size()).sum();
        assert_eq!(sum, d.size(), "{} has padding", d.name());
    }
}
