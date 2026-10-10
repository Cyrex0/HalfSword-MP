// Included after native_gameplay_pawn_impl.h inside the provider namespace.
// Runtime schemas retain actual FProperty pointers. HsmpProp is not substituted
// for ArrayDim, declared classes, property flags or native alignment.
#pragma once
#include "ue4ss_pins.h"
#include "native_gameplay_weapons.h"
struct WeaponName8 {uint32_t index{},number{};};
struct WeaponVariant16 {void* pointer{};bool object{};uint8_t pad[7]{};};
static_assert(sizeof(WeaponName8)==8&&sizeof(WeaponVariant16)==16);
static_assert(sizeof(std::wstring)==32&&sizeof(wchar_t)==2,"pinned release MSVC string ABI");
uint64_t weapon_name_bits(WeaponName8 n){return uint64_t(n.index)|(uint64_t(n.number)<<32);}
struct WeaponApi {
    HMODULE module{};
    void*(*property)(void*,const wchar_t*){};
    void**(*children)(void*){};void*(*next)(void*){};
    int32_t*(*size)(void*){};int32_t*(*offset)(void*){};int32_t*(*dimension)(void*){};
    int32_t(*alignment)(const void*){};const uint64_t*(*flags)(const void*){};const uint32_t*(*field_flags)(const void*){};
    WeaponName8*(*field_name)(const void*,WeaponName8*){};
    WeaponVariant16*(*field_class)(void*,WeaponVariant16*){};
    WeaponName8*(*variant_name)(const WeaponVariant16*,WeaponName8*){};
    WeaponVariant16*(*owner)(void*,WeaponVariant16*){};
    uint64_t(*owner_hash)(WeaponVariant16*){};bool(*owner_object)(const WeaponVariant16*){};
    void**(*structure)(void*){};void**(*meta_class)(void*){};void**(*byte_enum)(void*){};void**(*object_class)(void*){};
    int16_t*(*struct_alignment)(void*){};int32_t*(*struct_size)(void*){};
    std::wstring*(*name_text)(const WeaponName8*,std::wstring*){};
    std::wstring*(*full_text)(const void*,std::wstring*,void*){};
};
WeaponApi weapon_api{};
bool weapon_module_pinned(HMODULE module){
    if(!module||!gameplay_quat_readable(module,sizeof(IMAGE_DOS_HEADER)))return false;
    const auto* image=reinterpret_cast<const uint8_t*>(module);const auto* dos=reinterpret_cast<const IMAGE_DOS_HEADER*>(image);
    if(dos->e_magic!=IMAGE_DOS_SIGNATURE||dos->e_lfanew<=0||dos->e_lfanew>=65536||!gameplay_quat_readable(image+dos->e_lfanew,sizeof(IMAGE_NT_HEADERS64)))return false;
    const auto* pe=reinterpret_cast<const IMAGE_NT_HEADERS64*>(image+dos->e_lfanew);
    return pe->Signature==IMAGE_NT_SIGNATURE&&pe->FileHeader.Machine==IMAGE_FILE_MACHINE_AMD64&&
        pe->OptionalHeader.Magic==IMAGE_NT_OPTIONAL_HDR64_MAGIC&&pe->FileHeader.TimeDateStamp==HSMP_UE4SS_TIMESTAMP&&pe->OptionalHeader.SizeOfImage==HSMP_UE4SS_SIZE_OF_IMAGE;
}
template<class T>T weapon_export(HMODULE module,const char* label,uint32_t rva){
    auto p=GetProcAddress(module,label);require(p&&reinterpret_cast<uintptr_t>(p)==reinterpret_cast<uintptr_t>(module)+rva,"native weapon metadata export unavailable");return reinterpret_cast<T>(p);
}
WeaponApi weapon_load_api(){
    WeaponApi a;a.module=GetModuleHandleW(L"UE4SS.dll");require(weapon_module_pinned(a.module),"native weapon pinned UE4SS unavailable");
#define WP(member,label,rva) a.member=weapon_export<decltype(a.member)>(a.module,label,rva)
    WP(property,"?GetPropertyByNameInChain@UObject@Unreal@RC@@QEAAPEAVFProperty@23@PEB_W@Z",0x3e3e50);
    WP(children,"?GetChildProperties@UStruct@Unreal@RC@@QEAAAEAPEAVFField@23@XZ",0x4009f0);
    WP(next,"?GetNextFieldAsProperty@FField@Unreal@RC@@QEAAPEAVFProperty@23@XZ",0x407250);
    WP(size,"?GetElementSize@FProperty@Unreal@RC@@QEAAAEAHXZ",0x3f16a0);
    WP(offset,"?GetOffset_Internal@FProperty@Unreal@RC@@QEAAAEAHXZ",0x3f1a00);
    WP(dimension,"?GetArrayDim@FProperty@Unreal@RC@@QEAAAEAHXZ",0x3f1250);
    WP(alignment,"?GetMinAlignment@FProperty@Unreal@RC@@QEBAHXZ",0x3f1860);
    WP(flags,"?GetPropertyFlags@FProperty@Unreal@RC@@QEBAAEBW4EPropertyFlags@23@XZ",0x3f1d20);
    WP(field_flags,"?GetFlagsPrivate@FField@Unreal@RC@@QEBAAEBW4EObjectFlags@23@XZ",0x4069d0);
    WP(field_name,"?GetFName@FField@Unreal@RC@@QEBA?AVFName@23@XZ",0x406720);
    WP(field_class,"?GetClass@FField@Unreal@RC@@QEAA?AVFFieldClassVariant@23@XZ",0x405f90);
    WP(variant_name,"?GetFName@FFieldClassVariant@Unreal@RC@@QEBA?AVFName@23@XZ",0x4068d0);
    WP(owner,"?GetOwnerVariant@FField@Unreal@RC@@QEAA?AVFFieldVariant@23@XZ",0x4074f0);
    WP(owner_hash,"?HashObject@FFieldVariant@Unreal@RC@@QEAA_KXZ",0x4081a0);
    WP(owner_object,"?IsUObject@FFieldVariant@Unreal@RC@@QEBA_NXZ",0x2ae290);
    WP(structure,"?GetStruct@FStructProperty@Unreal@RC@@QEAAAEAV?$TObjectPtr@VUScriptStruct@Unreal@RC@@@23@XZ",0x429180);
    WP(meta_class,"?GetMetaClass@FClassProperty@Unreal@RC@@QEAAAEAV?$TObjectPtr@VUClass@Unreal@RC@@@23@XZ",0x464950);
    WP(byte_enum,"?GetEnum@FByteProperty@Unreal@RC@@QEAAAEAV?$TObjectPtr@VUEnum@Unreal@RC@@@23@XZ",0x433160);
    WP(object_class,"?GetPropertyClass@FObjectPropertyBase@Unreal@RC@@QEAAAEAV?$TObjectPtr@VUClass@Unreal@RC@@@23@XZ",0x418c40);
    WP(struct_alignment,"?GetMinAlignment@UStruct@Unreal@RC@@QEAAAEAFXZ",0x400ef0);
    WP(struct_size,"?GetPropertiesSize@UStruct@Unreal@RC@@QEAAAEAHXZ",0x4011b0);
    WP(name_text,"?ToString@FName@Unreal@RC@@QEBA?BV?$basic_string@_WU?$char_traits@_W@std@@V?$allocator@_W@2@@std@@XZ",0x3f52f0);
    WP(full_text,"?GetFullName@UObject@Unreal@RC@@QEBA?AV?$basic_string@_WU?$char_traits@_W@std@@V?$allocator@_W@2@@std@@PEAV123@@Z",0x3e0d80);
#undef WP
    return a;
}
void weapon_api_ok(){
    require(weapon_api.module&&GetModuleHandleW(L"UE4SS.dll")==weapon_api.module&&weapon_module_pinned(weapon_api.module),"native weapon original metadata module changed");
    require(weapon_api.property&&weapon_api.children&&weapon_api.next&&weapon_api.size&&weapon_api.offset&&weapon_api.dimension&&weapon_api.alignment&&weapon_api.flags&&weapon_api.field_flags&&
        weapon_api.field_name&&weapon_api.field_class&&weapon_api.variant_name&&weapon_api.owner&&weapon_api.owner_hash&&weapon_api.owner_object&&
        weapon_api.structure&&weapon_api.meta_class&&weapon_api.byte_enum&&weapon_api.object_class&&weapon_api.struct_alignment&&weapon_api.struct_size&&weapon_api.name_text&&weapon_api.full_text,
        "native weapon complete metadata unavailable");
}
enum class WeaponKind {Class,Object,Int,Name,Vector,Double,Float,Byte,Color};
std::array<uint64_t,9> weapon_type_names{};
struct WeaponFieldSpec {const wchar_t* label;WeaponKind kind;int32_t offset,size,alignment;const wchar_t* declared;};
constexpr WeaponFieldSpec weapon_fields[]{
    {L"WeaponClass_54_B478ECF7499977809745A3973AD678EC",WeaponKind::Class,0,8,8,L"/Game/Assets/Weapons/Blueprints/ModularWeaponBP.ModularWeaponBP_C"},
    {L"ID_70_C02CF656483647A1933EEA96314B78A6",WeaponKind::Int,8,4,4,nullptr},
    {L"Name_57_3729B51148E846FE8DD336B9419BCEE1",WeaponKind::Name,0xc,8,4,nullptr},
    {L"HeadSubModule1_7_ABBFD017411F42A4950B1C9F2360A30D",WeaponKind::Class,0x18,8,8,L"/Game/Assets/Weapons/Blueprints/Modular_Weapon_Module.Modular_Weapon_Module_C"},
    {L"HeadSubModule2_9_90AAA8304C7794E1BF814C9354A1A7E9",WeaponKind::Class,0x20,8,8,L"/Game/Assets/Weapons/Blueprints/Modular_Weapon_Module.Modular_Weapon_Module_C"},
    {L"HeadModule_11_62DF53134688807E1DA7F4A20E9F7139",WeaponKind::Class,0x28,8,8,L"/Game/Assets/Weapons/Blueprints/Modular_Weapon_Module.Modular_Weapon_Module_C"},
    {L"GuardModule_13_6DD2B06245505E53B529D090333012F0",WeaponKind::Class,0x30,8,8,L"/Game/Assets/Weapons/Blueprints/Modular_Weapon_Module.Modular_Weapon_Module_C"},
    {L"PommelModule_15_561B01324BFCD4360DAE9A95299BB9D6",WeaponKind::Class,0x38,8,8,L"/Game/Assets/Weapons/Blueprints/Modular_Weapon_Module.Modular_Weapon_Module_C"},
    {L"GripModule_18_F4DF51EB4E742195B8C6BAB17E4C5DB4",WeaponKind::Class,0x40,8,8,L"/Game/Assets/Weapons/Blueprints/Modular_Weapon_Grip.Modular_Weapon_Grip_C"},
    {L"HeadSize_21_2D425E61473B8F64FBAB51B223459D57",WeaponKind::Vector,0x48,24,8,L"/Script/CoreUObject.Vector"},
    {L"GuardSize_23_5A1AA0E04708E86FEFF61E974DDA8704",WeaponKind::Vector,0x60,24,8,L"/Script/CoreUObject.Vector"},
    {L"GripSize_25_AC1660814C4C25C521AAA8830FE8ECCF",WeaponKind::Vector,0x78,24,8,L"/Script/CoreUObject.Vector"},
    {L"PommelSize_27_660CC00C49C26D503E16B2BC58CE115E",WeaponKind::Vector,0x90,24,8,L"/Script/CoreUObject.Vector"},
    {L"CustomMassScaleHead_30_B95872A242AD944E2CE4D493F718F9D7",WeaponKind::Double,0xa8,8,8,nullptr},
    {L"CustomMassScaleGuard_51_3A9024E74306B7BB5D186087011D1927",WeaponKind::Double,0xb0,8,8,nullptr},
    {L"CustomMassScaleGrip_32_0EAADEE0419C05C6DB38F0AE134A9B10",WeaponKind::Double,0xb8,8,8,nullptr},
    {L"CustomMassScalePommel_34_0AB28D814BDEF17D408D0DAA3A453173",WeaponKind::Double,0xc0,8,8,nullptr},
    {L"MaterialMetalSteel_37_AB7A28C94B176CF81A6C8BA34AC57C36",WeaponKind::Byte,0xc8,1,1,L"/Game/Blueprints/Enumerator/Enum_MaterialLayer.Enum_MaterialLayer"},
    {L"MaterialMetalColored_39_DC2EAC244758A8D82855CC940784A1D2",WeaponKind::Byte,0xc9,1,1,L"/Game/Blueprints/Enumerator/Enum_MaterialLayer.Enum_MaterialLayer"},
    {L"MaterialWeood_41_E0B3C8DB48943B878AEFA3AB01E7B99A",WeaponKind::Byte,0xca,1,1,L"/Game/Blueprints/Enumerator/Enum_MaterialLayer.Enum_MaterialLayer"},
    {L"MaterialLeather_43_41D1114148FDB4FE4DACC8A2F4CA9FEB",WeaponKind::Byte,0xcb,1,1,L"/Game/Blueprints/Enumerator/Enum_MaterialLayer.Enum_MaterialLayer"},
    {L"ColorWood_46_F3AE05AD4495EBCD1D354C8025D7C743",WeaponKind::Color,0xcc,16,4,L"/Script/CoreUObject.LinearColor"},
    {L"ColorLeather_48_DC45F07E4C0C3280278212A7158EE638",WeaponKind::Color,0xdc,16,4,L"/Script/CoreUObject.LinearColor"},
    {L"Price_60_83FE5A624EA188485BBE4E9C8606AEE5",WeaponKind::Double,0xf0,8,8,nullptr},
    {L"Tier_67_05026E6F43B7300AA8BACC9D9F9AB461",WeaponKind::Byte,0xf8,1,1,L"/Game/Blueprints/Enumerator/Enum_Ranks.Enum_Ranks"}
};
static_assert(std::size(weapon_fields)==25);
const wchar_t* weapon_kind_name(WeaponKind kind){switch(kind){case WeaponKind::Class:return L"ClassProperty";case WeaponKind::Object:return L"ObjectProperty";case WeaponKind::Int:return L"IntProperty";case WeaponKind::Name:return L"NameProperty";case WeaponKind::Vector:case WeaponKind::Color:return L"StructProperty";case WeaponKind::Double:return L"DoubleProperty";case WeaponKind::Float:return L"FloatProperty";case WeaponKind::Byte:return L"ByteProperty";}throw Error("native weapon unsupported field kind");}
void weapon_prewarm(){
    for(size_t i=0;i<weapon_type_names.size();++i)weapon_type_names[i]=name(weapon_kind_name(static_cast<WeaponKind>(i)));
    for(const auto& spec:weapon_fields)name(spec.label);
    for(const auto* label:{L"Weapon Passport",L"Owner",L"OwningWorld",L"Weapon R",L"Weapon L",L"Weapon Slot R 1",L"Weapon Slot R 2",L"Weapon Slot Back",L"Weapon Slot L 1",L"Weapon Slot L 2",L"X",L"Y",L"Z",L"R",L"G",L"B",L"A",L"ReturnValue"})name(label);
}
struct WeaponField {
    void* pointer{};uint64_t table{},name{},kind{},flags{},owner{};uint32_t field_flags{};bool owner_object{};
    WeaponVariant16 type{};WeaponKind kind_enum{};int32_t offset{},size{},dimension{},alignment{},alignment_slot{};
    uint64_t alignment_target{};std::array<uint8_t,32> alignment_code{};Obj declared{};LookupEntry declared_path;
};
struct WeaponSchema {Obj object{};LookupEntry path;const char* label{};int32_t size{},padded_size{};int16_t alignment{};std::vector<void*> chain;std::vector<WeaponField> fields;};
WeaponField weapon_field_read(void* pointer,WeaponKind kind,bool cold){
    (void)cold;
    require(pointer&&gameplay_quat_readable(pointer,8),"native weapon property unavailable");WeaponField f;f.pointer=pointer;std::memcpy(&f.table,pointer,8);
    const auto* rf=weapon_api.field_flags(pointer);require(rf&&(*rf&0x40000000u)==0,"native weapon property garbage");f.field_flags=*rf;
    f.kind_enum=kind;WeaponName8 n{},c{};weapon_api.field_name(pointer,&n);weapon_api.field_class(pointer,&f.type);weapon_api.variant_name(&f.type,&c);
    f.name=weapon_name_bits(n);f.kind=weapon_name_bits(c);const auto expected_kind=weapon_type_names[static_cast<size_t>(kind)];require(expected_kind&&f.kind==expected_kind,"native weapon field kind changed");const auto* flags=weapon_api.flags(pointer);auto* offset=weapon_api.offset(pointer);auto* size=weapon_api.size(pointer);auto* dimension=weapon_api.dimension(pointer);
    require(flags&&offset&&size&&dimension,"native weapon complete property scalars unavailable");f.flags=*flags;f.offset=*offset;f.size=*size;f.dimension=*dimension;
    WeaponVariant16 owner{};weapon_api.owner(pointer,&owner);f.owner=weapon_api.owner_hash(&owner);f.owner_object=weapon_api.owner_object(&owner);
    void** declared{};if(kind==WeaponKind::Class)declared=weapon_api.meta_class(pointer);else if(kind==WeaponKind::Object)declared=weapon_api.object_class(pointer);else if(kind==WeaponKind::Vector||kind==WeaponKind::Color)declared=weapon_api.structure(pointer);else if(kind==WeaponKind::Byte)declared=weapon_api.byte_enum(pointer);
    if(declared){require(*declared!=nullptr,"native weapon declared type missing");f.declared.address=reinterpret_cast<uint64_t>(*declared);}
    return f;
}
template<class Pure>void weapon_alignment_bind(WeaponField& f,Pure pure){
        check_guard();pure();f.alignment=weapon_api.alignment(f.pointer);check_guard();pure();
        // Pinned GetMinAlignment3F1860 reads this initialized native virtual-slot
        // offset at3F188C and dispatches table[slot]. Never invoke it in the tail.
        const auto* slot=reinterpret_cast<const int32_t*>(reinterpret_cast<const uint8_t*>(weapon_api.module)+0x1344370);f.alignment_slot=*slot;
        require(f.alignment_slot>=0&&f.alignment_slot<=4096&&f.alignment_slot%8==0&&gameplay_quat_readable(reinterpret_cast<const void*>(f.table),static_cast<size_t>(f.alignment_slot)+8),"native weapon alignment target unavailable");
        std::memcpy(&f.alignment_target,reinterpret_cast<const uint8_t*>(f.table)+f.alignment_slot,8);require(f.alignment_target&&gameplay_quat_readable(reinterpret_cast<const void*>(f.alignment_target),f.alignment_code.size()),"native weapon alignment code unavailable");
        std::memcpy(f.alignment_code.data(),reinterpret_cast<const void*>(f.alignment_target),f.alignment_code.size());
}
bool weapon_field_same(const WeaponField& a,const WeaponField& b){return a.pointer==b.pointer&&a.table==b.table&&a.name==b.name&&a.kind==b.kind&&a.flags==b.flags&&a.field_flags==b.field_flags&&a.owner==b.owner&&a.owner_object==b.owner_object&&
    a.type.pointer==b.type.pointer&&a.type.object==b.type.object&&a.offset==b.offset&&a.size==b.size&&a.dimension==b.dimension&&a.declared.address==b.declared.address;}
void weapon_alignment_final(const WeaponField& f){
    require(*reinterpret_cast<const int32_t*>(reinterpret_cast<const uint8_t*>(weapon_api.module)+0x1344370)==f.alignment_slot&&
        gameplay_quat_readable(reinterpret_cast<const void*>(f.table),static_cast<size_t>(f.alignment_slot)+8),"native weapon original alignment dispatch changed");
    uint64_t target{};std::memcpy(&target,reinterpret_cast<const uint8_t*>(f.table)+f.alignment_slot,8);
    require(target==f.alignment_target&&gameplay_quat_readable(reinterpret_cast<const void*>(target),f.alignment_code.size())&&
        std::memcmp(reinterpret_cast<const void*>(target),f.alignment_code.data(),f.alignment_code.size())==0,"native weapon original alignment target changed");
}
void weapon_chain_final(const LookupEntry& path,const std::vector<void*>& chain,bool complete){
    lookup_entry_final(path);auto* object=lookup_node_get(path.pinned.front(),path.zero_item);auto** head=weapon_api.children(object);require(head,"native weapon original field chain unavailable");void* field=*head;
    for(size_t i=0;i<chain.size();++i){require(field==chain[i],"native weapon original field storage changed");if(complete||i+1<chain.size())field=weapon_api.next(field);}if(complete)require(field==nullptr,"native weapon original field cardinality changed");lookup_entry_final(path);
}
int32_t weapon_schema_layout(const char* label,int32_t raw,int16_t alignment,int32_t expected_raw,int32_t expected_padded,int16_t expected_alignment){
    // Pinned UStruct.GetStructureSize2345D0 aligns GetPropertiesSize to
    // GetMinAlignment. The SDK records both values; raw is not padded extent.
    int64_t padded=-1;
    if(raw>0&&alignment>0&&(alignment&(alignment-1))==0)
        padded=(int64_t(raw)+alignment-1)&-int64_t(alignment);
    if(raw!=expected_raw||alignment!=expected_alignment||padded!=expected_padded||padded>INT32_MAX){
        char reason[192]{};std::snprintf(reason,sizeof(reason),"native weapon schema unsupported; schema=%s raw=%d/%d padded=%lld/%d align=%d/%d",
            label,raw,expected_raw,static_cast<long long>(padded),expected_padded,static_cast<int>(alignment),static_cast<int>(expected_alignment));throw Error(reason);
    }
    return static_cast<int32_t>(padded);
}
void weapon_schema_final(const WeaponSchema& s){
    lookup_entry_final(s.path);auto* object=lookup_node_get(s.path.pinned.front(),s.path.zero_item);
    const auto* raw=weapon_api.struct_size(object);const auto* alignment=weapon_api.struct_alignment(object);require(raw&&alignment,"native weapon original struct metadata unavailable");
    weapon_schema_layout(s.label,*raw,*alignment,s.size,s.padded_size,s.alignment);
    weapon_chain_final(s.path,s.chain,true);
    for(size_t i=0;i<s.fields.size();++i){const auto& f=s.fields[i];if(f.declared.weak)lookup_entry_final(f.declared_path);
        auto fresh=weapon_field_read(f.pointer,f.kind_enum,false);require(weapon_field_same(f,fresh),"native weapon original field metadata changed");weapon_alignment_final(f);}
    lookup_entry_final(s.path);
}
WeaponSchema weapon_schema_bind(Obj object,const WeaponFieldSpec* specs,size_t count,const char* label,int32_t raw_bytes,int32_t bytes,int16_t minimum){
    WeaponSchema s;s.object=object;s.path=gameplay_path(object);auto* p=get(object);auto* size=weapon_api.struct_size(p);auto* alignment=weapon_api.struct_alignment(p);
    require(size&&alignment,"native weapon struct metadata unavailable");s.label=label;s.size=*size;s.alignment=*alignment;s.padded_size=weapon_schema_layout(label,s.size,s.alignment,raw_bytes,bytes,minimum);
    auto** head=weapon_api.children(get(object));require(head,"native weapon passport field chain unavailable");
    for(void* field=*head;field;field=weapon_api.next(field)){require(s.chain.size()<count&&std::find(s.chain.begin(),s.chain.end(),field)==s.chain.end(),"native weapon passport field cardinality unsupported");s.chain.push_back(field);}
    require(s.chain.size()==count,"native weapon complete fields unavailable");
    for(size_t index=0;index<count;++index){weapon_chain_final(s.path,s.chain,true);const auto& spec=specs[index];void* selected{};for(auto* field:s.chain){WeaponName8 n{};weapon_api.field_name(field,&n);if(weapon_name_bits(n)==name(spec.label)){require(!selected,"native weapon duplicate field");selected=field;}}
        require(selected,"native weapon passport field missing");auto f=weapon_field_read(selected,spec.kind,false);weapon_alignment_bind(f,[&](){weapon_chain_final(s.path,s.chain,true);auto current=weapon_field_read(selected,spec.kind,false);require(weapon_field_same(f,current),"native weapon field changed during alignment callback");});
        require(f.kind==name(weapon_kind_name(spec.kind))&&f.offset==spec.offset&&f.size==spec.size&&f.dimension==1&&f.alignment==spec.alignment&&f.owner_object&&f.owner==object.address,"native weapon passport field ABI unsupported");
        if(spec.declared){auto expected=find(spec.declared);require(f.declared.address==expected.address,"native weapon declared field type unsupported");f.declared=expected;f.declared_path=gameplay_path(expected);}s.fields.push_back(std::move(f));}
    check_guard();weapon_schema_final(s);return s;
}
struct WeaponValues {std::array<uint64_t,7> classes{};WeaponName8 name{};int32_t id{};std::array<double,12> sizes{};std::array<double,4> mass{};std::array<uint8_t,4> materials{};std::array<float,8> colors{};double price{};uint8_t tier{};};
WeaponValues weapon_values(const uint8_t* p){
    WeaponValues v;require(gameplay_quat_readable(p,0x100)&&reinterpret_cast<uintptr_t>(p)%8==0,"native weapon passport storage unavailable");
    for(size_t i=0;i<7;++i)std::memcpy(&v.classes[i],p+(i?0x18+(i-1)*8:0),8);std::memcpy(&v.id,p+8,4);std::memcpy(&v.name,p+0xc,8);
    std::memcpy(v.sizes.data(),p+0x48,96);std::memcpy(v.mass.data(),p+0xa8,32);std::memcpy(v.materials.data(),p+0xc8,4);std::memcpy(v.colors.data(),p+0xcc,32);std::memcpy(&v.price,p+0xf0,8);std::memcpy(&v.tier,p+0xf8,1);
    for(auto d:v.sizes)require(std::isfinite(d),"native weapon size nonfinite");for(auto d:v.mass)require(std::isfinite(d),"native weapon mass nonfinite");for(auto c:v.colors)require(std::isfinite(c),"native weapon color nonfinite");require(std::isfinite(v.price),"native weapon price nonfinite");return v;
}
bool weapon_values_same(const WeaponValues& a,const WeaponValues& b){return a.classes==b.classes&&weapon_name_bits(a.name)==weapon_name_bits(b.name)&&a.id==b.id&&a.materials==b.materials&&a.tier==b.tier&&
    std::memcmp(a.sizes.data(),b.sizes.data(),96)==0&&std::memcmp(a.mass.data(),b.mass.data(),32)==0&&std::memcmp(a.colors.data(),b.colors.data(),32)==0&&std::memcmp(&a.price,&b.price,8)==0;}
template<class Text>void weapon_text(Text& out,const std::wstring& value,size_t utf8_limit){
    // Validate exactly the same UTF-16→UTF-8 domain as the existing Lua converter.
    size_t bytes{};for(size_t i=0;i<value.size();++i){const uint32_t c=value[i];require(c!=0,"native weapon text NUL");if(c>=0xd800&&c<=0xdbff){require(i+1<value.size()&&value[i+1]>=0xdc00&&value[i+1]<=0xdfff,"native weapon text invalid UTF16");++i;bytes+=4;}else{require(c<0xdc00||c>0xdfff,"native weapon text invalid UTF16");bytes+=c<0x80?1:c<0x800?2:3;}}
    require(bytes<=utf8_limit&&value.size()<std::size(out.data),"native weapon text exceeds source bound");out={};out.length=static_cast<uint32_t>(value.size());for(size_t i=0;i<value.size();++i)out.data[i]=static_cast<uint16_t>(value[i]);
}
struct WeaponProperty {WeaponField field;Obj owner{};LookupEntry owner_path;std::vector<void*> prefix;};
LookupEntry weapon_path_pure(void* object);
void weapon_admit_identity(const LookupEntry& path);
std::array<LookupEntry,7> weapon_classes_pure(const WeaponValues& values,const LookupEntry& class_path);
template<class Find,class Qualify>void* weapon_bind_pointer(Find find,Qualify qualify){
    void* original=find();require(original,"native weapon containing property missing");qualify();
    require(find()==original,"native weapon containing property changed before first read");return original;
}
void weapon_property_final(const WeaponProperty& p){
    lookup_entry_final(p.owner_path);if(p.field.declared.weak)lookup_entry_final(p.field.declared_path);
    auto* owner=lookup_node_get(p.owner_path.pinned.front(),p.owner_path.zero_item);auto** head=weapon_api.children(owner);require(head,"native weapon original property chain unavailable");void* next=*head;
    for(auto* field:p.prefix){require(next==field,"native weapon original property replaced");if(field!=p.field.pointer)next=weapon_api.next(field);}
    require(!p.prefix.empty()&&p.prefix.back()==p.field.pointer,"native weapon original property binding missing");
    auto actual=weapon_field_read(p.field.pointer,p.field.kind_enum,false);require(weapon_field_same(p.field,actual),"native weapon original containing property changed");weapon_alignment_final(p.field);lookup_entry_final(p.owner_path);
}
WeaponProperty weapon_property_bind(Obj receiver,const wchar_t* label,WeaponKind kind,int32_t offset,int32_t size,const wchar_t* declared){
    check_guard();void* pointer=weapon_bind_pointer([&](){return weapon_api.property(get(receiver),label);},[&](){get(receiver);});
    WeaponProperty p;p.field=weapon_field_read(pointer,kind,true);
    require(p.field.name==name(label)&&p.field.offset==offset&&p.field.size==size&&p.field.dimension==1&&p.field.owner_object&&p.field.owner,"native weapon containing property ABI unsupported");
    p.owner_path=weapon_path_pure(reinterpret_cast<void*>(p.field.owner));weapon_admit_identity(p.owner_path);p.owner={p.owner_path.pinned.front().weak,p.field.owner};
    auto* original_owner=lookup_node_get(p.owner_path.pinned.front(),p.owner_path.zero_item);auto** head=weapon_api.children(original_owner);require(head,"native weapon containing class properties missing");
    for(auto* field=*head;field;field=weapon_api.next(field)){require(p.prefix.size()<4096&&std::find(p.prefix.begin(),p.prefix.end(),field)==p.prefix.end(),"native weapon containing property chain bound");p.prefix.push_back(field);if(field==pointer)break;}
    require(!p.prefix.empty()&&p.prefix.back()==pointer,"native weapon containing property chain missing");
    require(is(p.owner,L"/Script/CoreUObject.Class")&&vt->is_a(get(receiver),get(p.owner))!=0,"native weapon original property owner unsupported");
    if(declared){auto expected=find(declared);require(p.field.declared.address==expected.address,"native weapon containing declared class/struct unsupported");p.field.declared=expected;p.field.declared_path=gameplay_path(expected);}
    weapon_alignment_bind(p.field,[&](){weapon_chain_final(p.owner_path,p.prefix,false);auto current=weapon_field_read(p.field.pointer,kind,false);require(weapon_field_same(p.field,current),"native weapon property changed during alignment callback");});
    require(p.field.alignment==8,"native weapon containing property alignment unsupported");check_guard();weapon_property_final(p);return p;
}
constexpr const wchar_t* weapon_actor_path=L"/Game/Assets/Weapons/Blueprints/ModularWeaponBP.ModularWeaponBP_C";
constexpr const wchar_t* weapon_struct_path=L"/Game/Blueprints/Structure/Passports/Str_Passport_Weapon1.Str_Passport_Weapon1";
constexpr const wchar_t* weapon_alias_names[]{L"Weapon R",L"Weapon L",L"Weapon Slot R 1",L"Weapon Slot R 2",L"Weapon Slot Back",L"Weapon Slot L 1",L"Weapon Slot L 2"};
constexpr int32_t weapon_alias_offsets[]{0x17b8,0x1848,0x2200,0x2208,0x2218,0x2280,0x2288};
struct WeaponActor {
    Obj actor{},level{},owner{},owner_level{};LookupEntry path,level_path,owner_path,owner_level_path;WeaponProperty passport,owner_field,world_field,owner_world_field;
    uint64_t owner_address{};WeaponValues values;std::array<LookupEntry,7> classes;LookupEntry actor_class_path;
    Obj get_owner{};LookupEntry get_owner_path;
    bool bound{};
};
void weapon_owner_code(){
    const auto* image=reinterpret_cast<const uint8_t*>(GetModuleHandleW(nullptr));require(image&&gameplay_quat_readable(image,0x1000),"native weapon original engine unavailable");
    const auto* dos=reinterpret_cast<const IMAGE_DOS_HEADER*>(image);require(dos->e_magic==IMAGE_DOS_SIGNATURE&&dos->e_lfanew>0&&dos->e_lfanew<65536&&gameplay_quat_readable(image+dos->e_lfanew,sizeof(IMAGE_NT_HEADERS64)),"native weapon original engine header invalid");
    const auto* pe=reinterpret_cast<const IMAGE_NT_HEADERS64*>(image+dos->e_lfanew);
    constexpr uint8_t code[]{0x48,0x8b,0x42,0x20,0x45,0x33,0xc9,0x48,0x85,0xc0,0x41,0x0f,0x95,0xc1,0x4c,0x03,0xc8,0x4c,0x89,0x4a,0x20,0x48,0x8b,0x81,0x40,0x01,0,0,0x49,0x89,0,0xc3};
    require(pe->Signature==IMAGE_NT_SIGNATURE&&pe->FileHeader.Machine==IMAGE_FILE_MACHINE_AMD64&&pe->OptionalHeader.Magic==IMAGE_NT_OPTIONAL_HDR64_MAGIC&&
        pe->OptionalHeader.SizeOfImage>0x34a5530+sizeof(code)&&gameplay_quat_readable(image+0x34a5530,sizeof(code))&&std::memcmp(image+0x34a5530,code,sizeof(code))==0,"native weapon original actor owner code changed");
}
void weapon_actor_final(const WeaponActor& a,const WeaponSchema& schema,Obj world){
    lookup_entry_final(a.path);lookup_entry_final(a.level_path);lookup_entry_final(a.actor_class_path);lookup_entry_final(a.get_owner_path);if(a.owner.weak){lookup_entry_final(a.owner_path);lookup_entry_final(a.owner_level_path);weapon_property_final(a.owner_world_field);}
    weapon_property_final(a.passport);weapon_property_final(a.owner_field);weapon_property_final(a.world_field);weapon_schema_final(schema);weapon_owner_code();
    const auto* actor=lookup_node_get(a.path.pinned.front(),a.path.zero_item);const auto* level=lookup_node_get(a.level_path.pinned.front(),a.level_path.zero_item);
    const auto* outer=source_outer(actor);require(outer&&reinterpret_cast<uint64_t>(*outer)==a.level.address,"native weapon original actor level changed");
    uint64_t owner{},owning{};std::memcpy(&owner,static_cast<const uint8_t*>(actor)+a.owner_field.field.offset,8);std::memcpy(&owning,static_cast<const uint8_t*>(level)+a.world_field.field.offset,8);
    require(owner==a.owner_address&&owning==world.address,"native weapon original owner/world changed");
    if(a.owner.weak){const auto* original_owner=lookup_node_get(a.owner_path.pinned.front(),a.owner_path.zero_item);const auto* original_level=lookup_node_get(a.owner_level_path.pinned.front(),a.owner_level_path.zero_item);
        const auto* owner_outer=source_outer(original_owner);uint64_t owner_world{};std::memcpy(&owner_world,static_cast<const uint8_t*>(original_level)+a.owner_world_field.field.offset,8);
        require(owner_outer&&reinterpret_cast<uint64_t>(*owner_outer)==a.owner_level.address&&owner_world==world.address,"native weapon original owner membership changed");}
    auto current=weapon_values(static_cast<const uint8_t*>(actor)+a.passport.field.offset);require(weapon_values_same(a.values,current),"native weapon passport changed after native callback");
    for(size_t i=0;i<a.classes.size();++i)if(a.values.classes[i]){require(!a.classes[i].pinned.empty()&&a.classes[i].pinned.front().address==a.values.classes[i],"native weapon original class binding unavailable");lookup_entry_final(a.classes[i]);}
    lookup_entry_final(a.path);lookup_entry_final(a.level_path);
}
void weapon_owner_link_final(const WeaponActor& a){
    lookup_entry_final(a.path);lookup_entry_final(a.owner_path);weapon_property_final(a.owner_field);
    const auto* actor=lookup_node_get(a.path.pinned.front(),a.path.zero_item);uint64_t owner{};
    std::memcpy(&owner,static_cast<const uint8_t*>(actor)+a.owner_field.field.offset,8);
    require(owner==a.owner_address,"native weapon original owner changed during level callback");lookup_entry_final(a.owner_path);lookup_entry_final(a.path);
}
Obj weapon_level_return(const LookupEntry& owner_path,const void* returned_level){
    // Qualify the original complete Owner/Outer path before any metadata read
    // of the copied getter result. The result is only a scalar comparison.
    lookup_entry_final(owner_path);require(owner_path.pinned.size()>1,"native weapon original owner level path unavailable");
    const auto& level=owner_path.pinned[1];require(level.weak&&level.address==reinterpret_cast<uint64_t>(returned_level),"native weapon owner level getter changed");
    lookup_node_get(level,owner_path.zero_item);return {level.weak,level.address};
}
WeaponActor weapon_actor_bind(Obj actor,Obj world,const WeaponSchema& schema,HsmpViewResult* result){
    WeaponActor a;a.actor=actor;a.path=gameplay_path(actor);require(is(actor,weapon_actor_path),"native weapon actor class unsupported");
    auto cls=keep(vt->class_of(get(actor)));require(is(cls,L"/Script/CoreUObject.Class"),"native weapon actor class unavailable");a.actor_class_path=gameplay_path(cls);
    a.passport=weapon_property_bind(actor,L"Weapon Passport",WeaponKind::Vector,0xa58,0x100,weapon_struct_path);require(a.passport.field.declared.address==schema.object.address,"native weapon original passport struct changed");
    a.owner_field=weapon_property_bind(actor,L"Owner",WeaponKind::Object,0x140,8,L"/Script/Engine.Actor");
    Function owner(L"/Script/Engine.Actor:GetOwner");owner.field(L"ReturnValue",L"ObjectProperty",8);a.get_owner=owner.function;a.get_owner_path=gameplay_path(owner.function);weapon_owner_code();
    // The pinned getter above is exactly Actor[Owner.offset]. Capture the raw
    // link and its original identity together, before any owner callback.
    auto* original_actor=get(actor);weapon_property_final(a.owner_field);std::memcpy(&a.owner_address,static_cast<const uint8_t*>(original_actor)+a.owner_field.field.offset,8);
    if(a.owner_address){a.owner_path=weapon_path_pure(reinterpret_cast<void*>(a.owner_address));weapon_admit_identity(a.owner_path);a.owner={a.owner_path.pinned.front().weak,a.owner_address};}
    if(a.owner.weak){require(is(a.owner,L"/Script/Engine.Actor"),"native weapon owner class unsupported");
        Function level(L"/Script/Engine.Actor:GetLevel");level.field(L"ReturnValue",L"ObjectProperty",8);weapon_owner_link_final(a);level.call(a.owner,result);
        const auto* returned_level=level.value<void*>(L"ReturnValue",L"ObjectProperty");weapon_owner_link_final(a);a.owner_level=weapon_level_return(a.owner_path,returned_level);
        require(a.owner_level.weak&&is(a.owner_level,L"/Script/Engine.Level"),"native weapon original owner level unavailable");a.owner_level_path=gameplay_path(a.owner_level);
        a.owner_world_field=weapon_property_bind(a.owner_level,L"OwningWorld",WeaponKind::Object,0xc0,8,L"/Script/Engine.World");}
    a.level=returned(actor,L"/Script/Engine.Actor:GetLevel",result);require(a.level.weak&&is(a.level,L"/Script/Engine.Level"),"native weapon original level unavailable");a.level_path=gameplay_path(a.level);
    a.world_field=weapon_property_bind(a.level,L"OwningWorld",WeaponKind::Object,0xc0,8,L"/Script/Engine.World");
    auto class_type=find(L"/Script/CoreUObject.Class");const auto class_path=gameplay_path(class_type);
    auto* native=get(actor);weapon_property_final(a.passport);uint64_t own{};std::memcpy(&own,static_cast<const uint8_t*>(native)+0x140,8);require(own==a.owner_address,"native weapon owner getter binding changed");
    a.values=weapon_values(static_cast<const uint8_t*>(native)+a.passport.field.offset);
    a.classes=weapon_classes_pure(a.values,class_path);
    a.bound=true;check_guard();weapon_actor_final(a,schema,world);return a;
}
struct WeaponPawnSnapshot {
    uint64_t handle{};Obj world{},pawn{},controller{};LookupEntry world_path,pawn_path,controller_path;
    std::array<WeaponProperty,7> properties;std::array<uint64_t,7> addresses{};std::array<uint32_t,7> aliases{};
};
struct WeaponRosterSnapshot {WeaponSchema schema,vector,color;std::vector<WeaponPawnSnapshot> pawns;std::vector<WeaponActor> actors;std::vector<HsmpGameplayWeaponPassport> output;};
thread_local const WeaponRosterSnapshot* weapon_active_roster{};
struct WeaponWatch {const WeaponRosterSnapshot* previous{weapon_active_roster};explicit WeaponWatch(const WeaponRosterSnapshot& roster){weapon_active_roster=&roster;}~WeaponWatch(){weapon_active_roster=previous;}};
LookupEntry weapon_path_pure(void* object){
    LookupEntry path;require(source_package_name,"native weapon original package unavailable");path.package=*source_package_name;auto node=lookup_node(object,path,false,nullptr);
    for(;;){require(path.original.size()<64,"native weapon original path bound");for(const auto& old:path.original)require(old.address!=node.address,"native weapon original path cycle");
        path.original.push_back(node);const auto* p=lookup_node_get(node,path.zero_item);auto* flags=retirement_flags(p);auto* class_flags=retirement_flags(vt->resolve(node.class_weak));require(flags&&class_flags,"native weapon original RF unavailable");path.flags.push_back(*flags);path.class_flags.push_back(*class_flags);
        const auto* outer=source_outer(p);require(outer,"native weapon original Outer unavailable");if(!*outer)break;node=lookup_node(const_cast<void*>(*outer),path,true,nullptr);}
    path.pinned=path.original;lookup_pin(path);lookup_entry_final(path);return path;
}
void weapon_admit_identity(const LookupEntry& path){
    const auto& n=path.pinned.front();const Identity id{n.address,n.name,n.class_weak,n.class_address};auto found=identities.find(n.weak);
    if(found!=identities.end())require(found->second.address==id.address&&found->second.name==id.name&&found->second.class_weak==id.class_weak&&found->second.class_address==id.class_address,"native weapon original identity conflict");
    else{require(identities.size()<65536,"native weapon original identity bound");identities.emplace(n.weak,id);}
}
std::array<LookupEntry,7> weapon_classes_pure(const WeaponValues& values,const LookupEntry& class_path){
    // No guard/find/keep/Function/name callback may split this raw-value block.
    std::array<LookupEntry,7> paths;lookup_entry_final(class_path);
    for(size_t i=0;i<paths.size();++i)if(values.classes[i]){paths[i]=weapon_path_pure(reinterpret_cast<void*>(values.classes[i]));weapon_admit_identity(paths[i]);
        auto* original=lookup_node_get(paths[i].pinned.front(),paths[i].zero_item);auto* cls=lookup_node_get(class_path.pinned.front(),class_path.zero_item);
        require(vt->is_a(original,cls)!=0,"native weapon live passport class invalid");}
    for(const auto& path:paths)if(!path.pinned.empty())lookup_entry_final(path);lookup_entry_final(class_path);return paths;
}
void weapon_output_capacity(uint32_t count,size_t actual,uint32_t capacity,const HsmpGameplayWeaponPassport* output){
    require(count>0&&count<=32&&capacity<=count*7&&actual<=count*7&&actual<=capacity&&(!capacity||output),"native weapon actual output exceeds source weapon count");
}
size_t weapon_alias_index(const WeaponRosterSnapshot& snapshot,uint32_t pawn,uint64_t address){
    if(!address)return snapshot.actors.size();for(size_t n=0;n<snapshot.actors.size();++n)if(snapshot.actors[n].actor.address==address){
        require(snapshot.output[n].pawn_index==pawn,"native weapon shared across different pawns");return n;}return snapshot.actors.size();
}
std::shared_ptr<const WeaponRosterSnapshot> gameplay_weapon_snapshot;
void weapon_roster_final(const WeaponRosterSnapshot& snapshot){
    weapon_api_ok();weapon_schema_final(snapshot.schema);weapon_schema_final(snapshot.vector);weapon_schema_final(snapshot.color);
    for(const auto& row:snapshot.pawns){const auto& entry=gameplay_entry(row.handle);require(entry.stage==3&&entry.applied&&same(entry.world,row.world)&&same(entry.pawn,row.pawn)&&same(entry.controller,row.controller),"native weapon original gameplay binding changed");
        gameplay_pure(entry);lookup_entry_final(row.world_path);lookup_entry_final(row.pawn_path);lookup_entry_final(row.controller_path);auto* pawn=lookup_node_get(row.pawn_path.pinned.front(),row.pawn_path.zero_item);
        for(size_t i=0;i<7;++i){weapon_property_final(row.properties[i]);uint64_t address{};std::memcpy(&address,static_cast<const uint8_t*>(pawn)+row.properties[i].field.offset,8);require(address==row.addresses[i],"native weapon original seven-alias binding changed");}}
    for(const auto& actor:snapshot.actors)weapon_actor_final(actor,snapshot.schema,snapshot.pawns.front().world);
    for(const auto& row:snapshot.pawns){const auto& entry=gameplay_entry(row.handle);gameplay_pure(entry);lookup_entry_final(row.pawn_path);}
}
void gameplay_weapons_guard(){
    if(!weapon_active_roster)return;const auto& snapshot=*weapon_active_roster;
    for(const auto& row:snapshot.pawns){lookup_entry_final(row.world_path);lookup_entry_final(row.pawn_path);lookup_entry_final(row.controller_path);auto* pawn=lookup_node_get(row.pawn_path.pinned.front(),row.pawn_path.zero_item);
        for(size_t i=0;i<7;++i){weapon_property_final(row.properties[i]);uint64_t address{};std::memcpy(&address,static_cast<const uint8_t*>(pawn)+row.properties[i].field.offset,8);require(address==row.addresses[i],"native weapon original hard alias changed during callback");}}
    for(const auto& actor:snapshot.actors){lookup_entry_final(actor.path);if(actor.bound)weapon_actor_final(actor,snapshot.schema,snapshot.pawns.front().world);}
}
void gameplay_weapons_final(const uint64_t* handles,uint32_t count){
    require(gameplay_weapon_snapshot&&handles&&gameplay_weapon_snapshot->pawns.size()==count,"native weapon complete snapshot missing");
    for(size_t i=0;i<count;++i)require(gameplay_weapon_snapshot->pawns[i].handle==handles[i],"native weapon complete original roster changed");weapon_roster_final(*gameplay_weapon_snapshot);
}
void gameplay_weapons_discard(uint64_t handle){if(gameplay_weapon_snapshot)for(const auto& p:gameplay_weapon_snapshot->pawns)if(p.handle==handle){gameplay_weapon_snapshot.reset();break;}}
void gameplay_weapons_reset(){gameplay_weapon_snapshot.reset();weapon_api={};weapon_type_names={};}
void weapon_convert(const WeaponRosterSnapshot& roster,const WeaponActor& a,HsmpGameplayWeaponPassport& out){
    const auto original=[&](){check_guard();weapon_roster_final(roster);};
    const auto path=[&](const LookupEntry& identity,HsmpGameplayWeaponPath& target){if(identity.pinned.empty()){target={};return;}
        original();auto* p=lookup_node_get(identity.pinned.front(),identity.zero_item);std::wstring converted;weapon_api.full_text(p,&converted,nullptr);original();
        const auto split=converted.find(L' ');require(split!=std::wstring::npos&&split>0&&split+1<converted.size(),"native weapon class full name unsupported");weapon_text(target,converted.substr(split+1),512);};
    path(a.actor_class_path,out.actor_class);for(size_t i=0;i<7;++i)path(a.classes[i],out.classes[i]);
    original();const WeaponName8 copied=a.values.name;std::wstring converted;weapon_api.name_text(&copied,&converted);original();weapon_text(out.name,converted,128);
    out.id=a.values.id;std::memcpy(out.materials,a.values.materials.data(),4);out.tier=a.values.tier;std::memcpy(out.sizes,a.values.sizes.data(),96);std::memcpy(out.mass,a.values.mass.data(),32);out.price=a.values.price;std::memcpy(out.colors,a.values.colors.data(),32);
}
thread_local uint32_t weapon_batch_attempts{};
thread_local bool weapon_batch_timing_active{};
struct WeaponBatchTrace {
    bool enabled{},complete{};uint32_t attempt{},phase{};std::array<uint64_t,5> nanoseconds{};
    std::chrono::steady_clock::time_point started{},last{};
    WeaponBatchTrace(){
        if(weapon_batch_timing_active||active_lookup||active_guard||weapon_active_roster||weapon_batch_attempts>=8||!create_logger.load())return;
        enabled=true;weapon_batch_timing_active=true;attempt=++weapon_batch_attempts;started=last=std::chrono::steady_clock::now();
    }
    void advance(uint32_t next){if(!enabled)return;const auto now=std::chrono::steady_clock::now();
        nanoseconds[phase]+=static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::nanoseconds>(now-last).count());phase=next;last=now;}
    ~WeaponBatchTrace(){
        if(!enabled)return;const auto now=std::chrono::steady_clock::now();
        nanoseconds[phase]+=static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::nanoseconds>(now-last).count());weapon_batch_timing_active=false;
        // Declared before the lock: no live operation/TLS/mutex reaches logger.
        if(const auto logger=create_logger.load()){
            constexpr const char* labels[]{"schema_admission_inclusive","aliases","actors","conversion","final"};
            for(uint32_t i=0;i<5;++i)logger("gameplay_weapons_us",complete?1u:2u,nanoseconds[i]/1000,attempt,i,0,0,labels[i]);
            const auto total=static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::microseconds>(now-started).count());
            logger("gameplay_weapons_us",complete?1u:2u,total,attempt,5,0,0,"instrumented_total_inclusive");
        }
    }
};
int32_t gameplay_weapons(const uint64_t* handles,uint32_t count,const HsmpViewGuard* guard,uint32_t* aliases,HsmpGameplayWeaponPassport* output,uint32_t capacity,uint32_t* written,HsmpViewResult* result){
    WeaponBatchTrace timing;
    const std::lock_guard lock(gameplay_mutex);gameplay_weapon_snapshot.reset();if(written)*written=0;
    try{initialize_result(result);thread();require(handles&&aliases&&written&&count>0&&count<=32&&count==gameplay_pawns.size()&&capacity<=count*7&&(!capacity||output),"native weapon caller storage/roster bound");
        auto& first=gameplay_entry(handles[0]);OperationScope operation(guard,first.world);GameplayBoundaryScope boundary(first);GameplayWatch watch(first);if(!weapon_api.module)weapon_api=weapon_load_api();weapon_api_ok();weapon_prewarm();
        auto snapshot=std::make_shared<WeaponRosterSnapshot>();
        constexpr WeaponFieldSpec vector_specs[]{{L"X",WeaponKind::Double,0,8,8,nullptr},{L"Y",WeaponKind::Double,8,8,8,nullptr},{L"Z",WeaponKind::Double,16,8,8,nullptr}};
        constexpr WeaponFieldSpec color_specs[]{{L"R",WeaponKind::Float,0,4,4,nullptr},{L"G",WeaponKind::Float,4,4,4,nullptr},{L"B",WeaponKind::Float,8,4,4,nullptr},{L"A",WeaponKind::Float,12,4,4,nullptr}};
        snapshot->schema=weapon_schema_bind(find(weapon_struct_path),weapon_fields,std::size(weapon_fields),"WeaponPassport",0xf9,0x100,8);
        snapshot->vector=weapon_schema_bind(find(L"/Script/CoreUObject.Vector"),vector_specs,std::size(vector_specs),"Vector",24,24,8);
        snapshot->color=weapon_schema_bind(find(L"/Script/CoreUObject.LinearColor"),color_specs,std::size(color_specs),"LinearColor",16,16,4);
        timing.advance(1);
        snapshot->pawns.reserve(count);uint32_t own{};
        for(uint32_t index=0;index<count;++index){require(std::find(handles,handles+index,handles[index])==handles+index,"native weapon duplicate gameplay handle");const auto& entry=gameplay_entry(handles[index]);
            require(entry.stage==3&&entry.applied&&same(entry.world,first.world)&&same(entry.controller,first.controller),"native weapon original applied roster changed");own+=entry.own;
            WeaponPawnSnapshot row;row.handle=handles[index];row.world=entry.world;row.pawn=entry.pawn;row.controller=entry.controller;row.world_path=entry.world_path;row.pawn_path=entry.pawn_path;row.controller_path=entry.controller_path;
            for(size_t i=0;i<7;++i)row.properties[i]=weapon_property_bind(row.pawn,weapon_alias_names[i],WeaponKind::Object,weapon_alias_offsets[i],8,weapon_actor_path);
            auto* native=get(row.pawn);for(size_t i=0;i<7;++i)std::memcpy(&row.addresses[i],static_cast<const uint8_t*>(native)+row.properties[i].field.offset,8);snapshot->pawns.push_back(std::move(row));}
        require(own==1,"native weapon owned roster ambiguous");snapshot->actors.reserve(count*7);snapshot->output.reserve(count*7);
        timing.advance(2);
        // Capture every original actor weak/path/RF before any weapon callback.
        // The same pure block qualifies the original complete alias census first.
        check_guard();{WeaponWatch originals(*snapshot);gameplay_weapons_guard();
        for(uint32_t pawn=0;pawn<count;++pawn){auto& row=snapshot->pawns[pawn];for(size_t i=0;i<7;++i){if(!row.addresses[i])continue;size_t found=snapshot->actors.size();
                found=weapon_alias_index(*snapshot,pawn,row.addresses[i]);
                if(found==snapshot->actors.size()){WeaponActor actor;actor.path=weapon_path_pure(reinterpret_cast<void*>(row.addresses[i]));weapon_admit_identity(actor.path);actor.actor={actor.path.pinned.front().weak,row.addresses[i]};snapshot->actors.push_back(std::move(actor));HsmpGameplayWeaponPassport copied{};copied.pawn_index=pawn;snapshot->output.push_back(copied);}
                row.aliases[i]=static_cast<uint32_t>(found+1);}}
        gameplay_weapons_guard();}
        WeaponWatch originals(*snapshot);
        for(size_t i=0;i<snapshot->actors.size();++i){const auto original=snapshot->actors[i].path;const auto actor=snapshot->actors[i].actor;auto fresh=weapon_actor_bind(actor,snapshot->pawns[snapshot->output[i].pawn_index].world,snapshot->schema,result);
            require(gameplay_path_equal(original,fresh.path),"native weapon original cold actor witness changed");snapshot->actors[i]=std::move(fresh);}
        weapon_output_capacity(count,snapshot->output.size(),capacity,output);check_guard();weapon_roster_final(*snapshot);
        timing.advance(3);
        for(size_t i=0;i<snapshot->actors.size();++i)weapon_convert(*snapshot,snapshot->actors[i],snapshot->output[i]);
        timing.advance(4);
        check_guard();lookup_finish();weapon_roster_final(*snapshot);
        // No callbacks or native queries after final full-set closure.
        for(size_t i=0;i<snapshot->pawns.size();++i)std::copy(snapshot->pawns[i].aliases.begin(),snapshot->pawns[i].aliases.end(),aliases+i*7);
        if(!snapshot->output.empty())std::copy(snapshot->output.begin(),snapshot->output.end(),output);*written=static_cast<uint32_t>(snapshot->output.size());gameplay_weapon_snapshot=std::move(snapshot);result->complete=1;timing.complete=true;return 1;
    }catch(const std::exception& error){failure(result,error.what());return -1;}
}
