// Exercises the production typed-copy/metadata/text helpers; synthetic fixture
// storage establishes rejection behavior, not native game parity.
#include "../src/native_presentation.cpp"
#include "../src/native_gameplay_weapons_impl.h"
#include <iostream>
#include <limits>
extern "C" void hsmp_native_set_presentation(const HsmpPresentation*){}
extern "C" void hsmp_native_set_gameplay(const HsmpGameplay*){}
extern "C" void hsmp_native_set_puppet_publish(HsmpPuppetPublish){}
extern "C" void hsmp_native_profile_checkpoint(const char*,uint32_t){}
extern "C" void hsmp_native_profile_tick(uint32_t){}
extern "C" int32_t hsmp_native_capture_profile_active(){return 0;}
extern "C" void hsmp_native_capture_profile_row(const HsmpNativeCaptureRow*){}
extern "C" void hsmp_native_set_source_path_reader(HsmpNativePathReader){}
namespace {
int checks{};
void check(bool value,const char* why){++checks;if(!value)throw std::runtime_error(why);}
template<class F>void rejects(F f,const char* why){bool rejected{};try{f();}catch(const Error&){rejected=true;}check(rejected,why);}
struct MockProperty {
    uint64_t table{};WeaponName8 name{50,7},kind{60,0};WeaponVariant16 type{reinterpret_cast<void*>(100),false,{}};
    uint64_t flags{0x12345678},owner{101};uint32_t field_flags{};int32_t offset{0},size{8},dimension{1};void* declared{reinterpret_cast<void*>(102)};
    void* next{};
};
int declared_reads{},field_name_reads{};
WeaponName8* mock_field_name(const void* p,WeaponName8* n){++field_name_reads;*n=static_cast<const MockProperty*>(p)->name;return n;}
WeaponVariant16* mock_field_class(void* p,WeaponVariant16* v){*v=static_cast<MockProperty*>(p)->type;v->pointer=&static_cast<MockProperty*>(p)->kind;return v;}
WeaponName8* mock_variant_name(const WeaponVariant16* v,WeaponName8* n){*n=*static_cast<const WeaponName8*>(v->pointer);return n;}
const uint64_t* mock_flags(const void* p){return &static_cast<const MockProperty*>(p)->flags;}
const uint32_t* mock_field_flags(const void* p){return &static_cast<const MockProperty*>(p)->field_flags;}
int32_t* mock_offset(void* p){return &static_cast<MockProperty*>(p)->offset;}
int32_t* mock_size(void* p){return &static_cast<MockProperty*>(p)->size;}
int32_t* mock_dimension(void* p){return &static_cast<MockProperty*>(p)->dimension;}
WeaponVariant16* mock_owner(void* p,WeaponVariant16* v){v->pointer=reinterpret_cast<void*>(static_cast<MockProperty*>(p)->owner);v->object=true;return v;}
uint64_t mock_owner_hash(WeaponVariant16* v){return reinterpret_cast<uint64_t>(v->pointer);}
bool mock_owner_object(const WeaponVariant16* v){return v->object;}
void** mock_declared(void* p){++declared_reads;return &static_cast<MockProperty*>(p)->declared;}
void metadata_cases(){
    names[L"ClassProperty"]=60;weapon_type_names[static_cast<size_t>(WeaponKind::Class)]=60;weapon_api.field_name=mock_field_name;weapon_api.field_class=mock_field_class;weapon_api.variant_name=mock_variant_name;
    weapon_api.flags=mock_flags;weapon_api.field_flags=mock_field_flags;weapon_api.offset=mock_offset;weapon_api.size=mock_size;weapon_api.dimension=mock_dimension;weapon_api.owner=mock_owner;
    weapon_api.owner_hash=mock_owner_hash;weapon_api.owner_object=mock_owner_object;weapon_api.meta_class=mock_declared;
    MockProperty p;auto original=weapon_field_read(&p,WeaponKind::Class,false);
    check(original.pointer==&p&&original.dimension==1&&original.declared.address==102&&original.flags==p.flags,"actual raw FProperty API capture");
    check(weapon_field_same(original,weapon_field_read(&p,WeaponKind::Class,false)),"same complete metadata");
    auto mutation=[&](auto edit,const char* why){MockProperty next=p;edit(next);auto fresh=weapon_field_read(&next,WeaponKind::Class,false);fresh.pointer=original.pointer;fresh.type.pointer=original.type.pointer;check(!weapon_field_same(original,fresh),why);};
    mutation([](auto& f){f.dimension=2;},"ArrayDim mutation");mutation([](auto& f){f.size=4;},"size mutation");mutation([](auto& f){f.offset=8;},"offset mutation");
    mutation([](auto& f){f.flags^=1;},"full property flags mutation");mutation([](auto& f){f.owner=103;},"original declaring owner mutation");
    mutation([](auto& f){f.field_flags=1;},"full native RF mutation");
    mutation([](auto& f){f.name.number^=1;},"full Name Number mutation");mutation([](auto& f){f.declared=reinterpret_cast<void*>(104);},"declared native class mutation");
    mutation([](auto& f){f.table=105;},"native property vtable mutation");
    p.kind.index=99;declared_reads=0;rejects([&](){weapon_field_read(&p,WeaponKind::Class,false);},"changed kind refuses before subclass accessor");check(declared_reads==0,"no invalid typed metadata query");
    p.kind.index=60;p.declared=nullptr;rejects([&](){weapon_field_read(&p,WeaponKind::Class,false);},"missing declared class refuses");
    p.field_flags=0x40000000u;field_name_reads=0;rejects([&](){weapon_field_read(&p,WeaponKind::Class,false);},"garbage field refuses");check(field_name_reads==0,"RF refuses before FName metadata");
    int finds{},old_reads{};int original_storage{},replacement{};void* selected=&original_storage;
    rejects([&](){auto* field=weapon_bind_pointer([&](){++finds;return selected;},[&](){selected=&replacement;});if(field==&original_storage)++old_reads;},"callback replacement before first metadata refuses");
    check(finds==2&&old_reads==0,"replaced borrowed field never dereferenced");
    selected=&original_storage;check(weapon_bind_pointer([&](){return selected;},[](){})==&original_storage,"stable first metadata pointer is admitted");
}
void value_cases(){
    alignas(8) std::array<uint8_t,0x100> raw{};const uint64_t classes[]{10,11,0,13,14,15,16};
    for(size_t i=0;i<7;++i)std::memcpy(raw.data()+(i?0x18+(i-1)*8:0),classes+i,8);
    const int32_t id=-12345;const WeaponName8 native_name{0xabcdef01,0x76543210};std::memcpy(raw.data()+8,&id,4);std::memcpy(raw.data()+0xc,&native_name,8);
    double sizes[12],mass[4];float colors[8];for(size_t i=0;i<12;++i)sizes[i]=double(i)+0.12345678901234567;sizes[3]=-0.0;
    for(size_t i=0;i<4;++i)mass[i]=double(i)+0.9876543210987654;mass[2]=-0.0;for(size_t i=0;i<8;++i)colors[i]=float(i)+0.12345f;colors[5]=-0.0f;
    const uint8_t materials[]{1,2,255,0};double price=-0.0;std::memcpy(raw.data()+0x48,sizes,96);std::memcpy(raw.data()+0xa8,mass,32);std::memcpy(raw.data()+0xc8,materials,4);
    std::memcpy(raw.data()+0xcc,colors,32);std::memcpy(raw.data()+0xf0,&price,8);raw[0xf8]=255;
    const auto original=weapon_values(raw.data());check(original.id==id&&weapon_name_bits(original.name)==weapon_name_bits(native_name),"integer/full FName exact");
    check(std::memcmp(original.classes.data(),classes,56)==0,"all7 native nullable class leaves");check(std::memcmp(original.sizes.data(),sizes,96)==0,"all4 exact native vectors");
    check(std::memcmp(original.mass.data(),mass,32)==0,"all4 exact native masses");check(std::memcmp(original.colors.data(),colors,32)==0,"both exact float32 colors");
    check(std::memcmp(original.materials.data(),materials,4)==0&&original.tier==255,"all4 material bytes+tier");check(std::memcmp(&original.price,&price,8)==0,"price signed zero exact");
    raw[0x14]=255;raw[0xec]=255;raw[0xff]=255;check(weapon_values_same(original,weapon_values(raw.data())),"padding is not semantic data");
    raw[0xb8]^=1;check(!weapon_values_same(original,weapon_values(raw.data())),"later callback changing earlier native field refuses");raw[0xb8]^=1;
    const double positive_zero=0;std::memcpy(raw.data()+0xf0,&positive_zero,8);check(!weapon_values_same(original,weapon_values(raw.data())),"signed-zero mutation is exact disagreement");
    const double nan=std::numeric_limits<double>::quiet_NaN();std::memcpy(raw.data()+0xa8,&nan,8);rejects([&](){weapon_values(raw.data());},"nonfinite native mass refuses");
    rejects([&](){weapon_values(raw.data()+1);},"unaligned passport storage refuses");
}
void text_cases(){
    HsmpGameplayWeaponPath path{};weapon_text(path,L"/Game/Exact.Exact_C",512);check(path.length==19&&path.data[path.length]==0,"caller-owned class text with terminator");
    HsmpGameplayWeaponName label{};weapon_text(label,L"None_42",128);check(label.length==7&&label.data[0]=='N',"full native name string retained");
    weapon_text(label,L"",128);check(label.length==0,"observed empty name supported");weapon_text(path,L"",512);check(path.length==0,"observed null class output empty");
    std::wstring unicode{wchar_t(0xd83d),wchar_t(0xde00)};weapon_text(label,unicode,128);check(label.length==2&&label.data[0]==0xd83d&&label.data[1]==0xde00,"lossless surrogate pair UTF16");
    rejects([&](){weapon_text(label,std::wstring(129,L'x'),128);},"name byte bound");rejects([&](){weapon_text(path,std::wstring(513,L'x'),512);},"class byte bound");
    rejects([&](){weapon_text(label,std::wstring(65,wchar_t(0x100)),128);},"UTF8 bytes rather than UTF16 count bound");
    rejects([&](){weapon_text(label,std::wstring(1,wchar_t(0xd800)),128);},"lone high surrogate");rejects([&](){weapon_text(label,std::wstring(1,wchar_t(0xdc00)),128);},"lone low surrogate");
    rejects([&](){weapon_text(label,std::wstring{L'a',0,L'b'},128);},"NUL string refuses");
}
void alignment_cases(){
    std::vector<uint8_t> image(HSMP_UE4SS_SIZE_OF_IMAGE);weapon_api.module=reinterpret_cast<HMODULE>(image.data());int32_t slot=0;std::memcpy(image.data()+0x1344370,&slot,4);
    std::array<uint8_t,32> code{};uint64_t table=reinterpret_cast<uint64_t>(code.data());WeaponField f;f.table=reinterpret_cast<uint64_t>(&table);f.alignment_slot=0;f.alignment_target=table;f.alignment_code=code;
    weapon_alignment_final(f);check(true,"same retained native virtual target");table+=1;rejects([&](){weapon_alignment_final(f);},"alignment virtual target mutation");table-=1;
    code[3]^=1;rejects([&](){weapon_alignment_final(f);},"alignment target code mutation");code[3]^=1;slot=8;std::memcpy(image.data()+0x1344370,&slot,4);
    rejects([&](){weapon_alignment_final(f);},"initialized native slot mutation");weapon_api={};
}
void alias_capacity_cases(){
    WeaponRosterSnapshot snapshot;WeaponActor original;original.actor={1,100};snapshot.actors.push_back(original);HsmpGameplayWeaponPassport output{};output.pawn_index=0;snapshot.output.push_back(output);
    check(weapon_alias_index(snapshot,0,100)==0,"same original actor alias reuses copied row");check(weapon_alias_index(snapshot,0,200)==1,"distinct actor remains distinct");check(weapon_alias_index(snapshot,0,0)==1,"null alias does not bind an actor");
    rejects([&](){weapon_alias_index(snapshot,1,100);},"actor shared across pawns refuses");
    weapon_output_capacity(2,2,2,&output);check(true,"actual2 source weapons need capacity2 not14");weapon_output_capacity(2,0,0,nullptr);check(true,"explicit no-weapon roster uses zero capacity");
    rejects([&](){weapon_output_capacity(2,3,2,&output);},"extra native weapon exceeds source capacity");rejects([&](){weapon_output_capacity(2,0,1,nullptr);},"nonnull output required for nonzero capacity");
    rejects([&](){weapon_output_capacity(2,1,0,nullptr);},"native weapon cannot fit zero source capacity");rejects([&](){weapon_output_capacity(2,2,15,&output);},"source capacity cannot exceed hardfield census bound");
}
void schema_layout_cases(){
    check(weapon_schema_layout("WeaponPassport",249,8,249,256,8)==256,"native raw249/alignment8 preserves padded256 extent");
    check(weapon_schema_layout("Vector",24,8,24,24,8)==24,"Vector exact raw and padded24/alignment8");
    check(weapon_schema_layout("LinearColor",16,4,16,16,4)==16,"LinearColor exact raw and padded16/alignment4");
    rejects([&](){weapon_schema_layout("WeaponPassport",256,8,249,256,8);},"padded256 cannot replace original raw249");
    rejects([&](){weapon_schema_layout("WeaponPassport",250,8,249,256,8);},"same padded extent does not admit changed semantic size");
    rejects([&](){weapon_schema_layout("WeaponPassport",249,16,249,256,8);},"same padded extent does not admit changed native alignment");
    rejects([&](){weapon_schema_layout("WeaponPassport",249,0,249,256,8);},"missing native alignment refuses");
    rejects([&](){weapon_schema_layout("WeaponPassport",INT32_MAX,8,249,256,8);},"rounded extent cannot overflow int32");
    try{weapon_schema_layout("WeaponPassport",256,8,249,256,8);throw Error("fixture expected schema refusal");}
    catch(const Error& e){check(std::string(e.what())=="native weapon schema unsupported; schema=WeaponPassport raw=256/249 padded=256/256 align=8/8","schema refusal copies exact actual/expected scalars and fixed name");}
}
// Pure native metadata stand-ins for the production original-path helpers.
// Retired addresses are trapped if any metadata reader touches them.
struct ColdObject {uint64_t name{};ColdObject* cls{};const void* outer{};uint32_t flags{};void* head{};ColdObject* gi{};int32_t raw_size{249};int16_t alignment{8};uint16_t payload_padding{};std::array<uint8_t,256> payload{};};
static_assert(offsetof(ColdObject,payload)%8==0,"synthetic passport payload preserves native alignment");
std::vector<ColdObject*> cold_objects;ColdObject* cold_retired{};void* cold_poisoned_field{};int cold_stale_reads{},cold_field_reads{};
ColdObject* cold_rf_missing{};ColdObject* cold_metadata_trap{};int cold_invalid_metadata_reads{},cold_target_rf_reads{};
uint64_t cold_weak(void* p){for(size_t i=0;i<cold_objects.size();++i)if(cold_objects[i]==p)return p==cold_retired?0:(uint64_t(1)<<32)|(i+1);return 0;}
void* cold_resolve(uint64_t w){const auto index=static_cast<uint32_t>(w);if((w>>32)!=1||index==0||index>cold_objects.size())return nullptr;auto* p=cold_objects[index-1];return p==cold_retired?nullptr:p;}
void cold_live(const void* p){if(p==cold_retired){++cold_stale_reads;throw Error("fixture retired metadata touched");}}
void cold_metadata(const void* p){cold_live(p);if(p==cold_metadata_trap){++cold_invalid_metadata_reads;throw Error("fixture RF-failed metadata touched");}}
void* cold_class(void* p){cold_metadata(p);return static_cast<ColdObject*>(p)->cls;}
const uint64_t* cold_name(const void* p){cold_metadata(p);return &static_cast<const ColdObject*>(p)->name;}
const uint32_t* cold_rf(const void* p){cold_live(p);if(p==cold_metadata_trap)++cold_target_rf_reads;return p==cold_rf_missing?nullptr:&static_cast<const ColdObject*>(p)->flags;}
const void* const* cold_outer(const void* p){cold_metadata(p);return &static_cast<const ColdObject*>(p)->outer;}
int32_t cold_is_a(void* p,void* cls){cold_live(p);cold_live(cls);return static_cast<ColdObject*>(p)->cls==cls?1:0;}
void** cold_children(void* p){cold_live(p);return &static_cast<ColdObject*>(p)->head;}
int32_t* cold_size(void* p){cold_live(p);return &static_cast<ColdObject*>(p)->raw_size;}
int16_t* cold_alignment(void* p){cold_live(p);return &static_cast<ColdObject*>(p)->alignment;}
void* cold_next(void* p){if(p==cold_poisoned_field){++cold_field_reads;throw Error("fixture borrowed field touched");}return static_cast<MockProperty*>(p)->next;}
void cold_path_cases(){
    ColdObject type{(uint64_t(0xabcdef12)<<32)|1},world{2},gi{3},owner{(uint64_t(0x12345678)<<32)|4},class_a{5},class_b{6};type.cls=&type;
    for(auto* p:{&world,&gi,&owner,&class_a,&class_b})p->cls=&type;world.gi=&gi;
    cold_objects={&type,&world,&gi,&owner,&class_a,&class_b};cold_retired=nullptr;cold_stale_reads=0;cold_poisoned_field=nullptr;cold_field_reads=0;
    HsmpReflect reflect{};reflect.weak=cold_weak;reflect.resolve=cold_resolve;reflect.class_of=cold_class;reflect.is_a=cold_is_a;vt=&reflect;
    object_name=cold_name;retirement_flags=cold_rf;source_outer=cold_outer;const uint64_t package=99;source_package_name=&package;identities.clear();
    LookupState lookup;lookup.world={cold_weak(&world),reinterpret_cast<uint64_t>(&world)};lookup.gi={cold_weak(&gi),reinterpret_cast<uint64_t>(&gi)};
    lookup.world_node=source_path_node(&world);lookup.gi_node=source_path_node(&gi);lookup.gi_property.offset=static_cast<int32_t>(offsetof(ColdObject,gi));active_lookup=&lookup;
    const auto class_path=weapon_path_pure(&type);WeaponValues values;values.classes={reinterpret_cast<uint64_t>(&class_a),0,reinterpret_cast<uint64_t>(&class_b),0,reinterpret_cast<uint64_t>(&class_a),0,0};
    const auto paths=weapon_classes_pure(values,class_path);
    check(paths[0].pinned.front().address==values.classes[0]&&paths[2].pinned.front().address==values.classes[2],"all saved class pointers acquire original pure identities together");
    check(paths[1].pinned.empty()&&paths[3].pinned.empty()&&paths[5].pinned.empty()&&paths[6].pinned.empty(),"original null class leaves stay null");
    check(gameplay_path_equal(paths[0],paths[4]),"same-class aliases retain agreeing original witnesses");
    // A later callback destroys a saved later class. Qualification refuses via
    // its original weak before any old-address FName/class/RF/Outer read.
    cold_retired=&class_b;rejects([&](){lookup_entry_final(paths[2]);},"later callback retirement refuses original class witness");check(cold_stale_reads==0,"no stale saved class metadata read");cold_retired=nullptr;
    auto original_name=class_a.name;class_a.name^=1;rejects([&](){lookup_entry_final(paths[0]);},"later class rename refuses original witness");class_a.name=original_name;
    const auto owner_path=weapon_path_pure(&owner);MockProperty first,second,replacement;first.next=&second;owner.head=&first;weapon_api.children=cold_children;weapon_api.next=cold_next;
    const std::vector<void*> chain{&first,&second};weapon_chain_final(owner_path,chain,true);check(true,"original borrowed chain qualified before scan");
    // Models a preceding declared-class find callback replacing its owner's
    // allocation. The next spec's actual production entry check must reject
    // before following the stale field or querying its name.
    owner.head=&replacement;cold_poisoned_field=&first;cold_field_reads=0;
    rejects([&](){weapon_chain_final(owner_path,chain,true);cold_next(&first);},"declared-class callback chain replacement refuses before next spec");
    check(cold_field_reads==0,"poisoned old field never read after callback");
    // The native GetLevel result is copied as a scalar; no returned()->keep
    // touches it until the original Owner/Level path closes after the call.
    LookupState level_lookup;level_lookup.world=lookup.world;level_lookup.gi=lookup.gi;
    level_lookup.world_node=lookup.world_node;level_lookup.gi_node=lookup.gi_node;level_lookup.gi_property=lookup.gi_property;
    active_lookup=&level_lookup;owner.outer=&class_b;const auto owner_level_path=weapon_path_pure(&owner);
    auto admitted_level=weapon_level_return(owner_level_path,&class_b);check(admitted_level.address==reinterpret_cast<uint64_t>(&class_b)&&admitted_level.weak==cold_weak(&class_b),"stable native level return reuses exact original identity");
    owner.outer=&class_a;rejects([&](){weapon_level_return(owner_level_path,&class_b);},"GetLevel callback Owner Outer replacement refuses before returned metadata");owner.outer=&class_b;
    cold_retired=&class_b;cold_stale_reads=0;rejects([&](){weapon_level_return(owner_level_path,&class_b);},"GetLevel callback original Level retirement refuses");check(cold_stale_reads==0,"no retired GetLevel output metadata read");cold_retired=nullptr;
    rejects([&](){weapon_level_return(owner_level_path,&class_a);},"different raw Level return refuses without adoption");owner.outer=nullptr;
    owner.head=nullptr;weapon_api.struct_size=cold_size;weapon_api.struct_alignment=cold_alignment;
    WeaponSchema schema;schema.object={cold_weak(&owner),reinterpret_cast<uint64_t>(&owner)};schema.path=owner_path;schema.label="WeaponPassport";schema.size=249;schema.padded_size=256;schema.alignment=8;
    weapon_schema_final(schema);check(true,"final schema rereads exact original raw/min/padded extent");
    owner.raw_size=250;rejects([&](){weapon_schema_final(schema);},"later native raw size mutation refuses even when padded extent agrees");owner.raw_size=249;
    owner.alignment=16;rejects([&](){weapon_schema_final(schema);},"later native minimum alignment mutation refuses even when padded extent agrees");owner.alignment=8;
    // Build the actual copied plan from an original typed field and repeatedly
    // feed agreeing expectations. Native reads remain fresh and unique.
    std::vector<uint8_t> image(HSMP_UE4SS_SIZE_OF_IMAGE);weapon_api.module=reinterpret_cast<HMODULE>(image.data());
    std::array<uint8_t,32> code{};uint64_t table=reinterpret_cast<uint64_t>(code.data());MockProperty passport_field;passport_field.table=reinterpret_cast<uint64_t>(&table);
    passport_field.owner=reinterpret_cast<uint64_t>(&owner);passport_field.declared=&type;owner.head=&passport_field;
    weapon_api.field_name=mock_field_name;weapon_api.field_class=mock_field_class;weapon_api.variant_name=mock_variant_name;weapon_api.flags=mock_flags;weapon_api.field_flags=mock_field_flags;
    weapon_api.offset=mock_offset;weapon_api.size=mock_size;weapon_api.dimension=mock_dimension;weapon_api.owner=mock_owner;weapon_api.owner_hash=mock_owner_hash;weapon_api.owner_object=mock_owner_object;weapon_api.meta_class=mock_declared;
    auto bound=weapon_field_read(&passport_field,WeaponKind::Class,false);bound.alignment=8;bound.alignment_slot=0;bound.alignment_target=table;bound.alignment_code=code;bound.declared_path=class_path;
    schema.chain={&passport_field};schema.fields={bound};WeaponBoundaryPlan plan;plan.schema(schema);plan.schema(schema);
    check(plan.fields.size()==1&&plan.chains.size()==1&&plan.schemas.size()==1,"agreeing full metadata and chains are deduplicated without new observations");
    field_name_reads=0;plan.validate();plan.validate();check(field_name_reads==2,"two fresh passes read each unique typed field once per pass");
    const auto rf_case=[&](ColdObject& object,uint32_t flags,const char* kind,const char* source,const char* scalar,bool missing){
        const auto original=object.flags;object.flags=flags;cold_metadata_trap=&object;cold_rf_missing=missing?&object:nullptr;cold_invalid_metadata_reads=0;cold_target_rf_reads=0;
        bool refused{};try{plan.validate();}catch(const Error& error){const std::string reason=error.what();refused=true;
            check(reason.find(std::string("native weapon original ")+kind+" RF changed; src="+source+" row=-1 own=-1 depth=0 pawn=0")!=std::string::npos,"RF diagnostic preserves original kind/refusal and first copied path provenance");
            char copied_name[32]{},copied_class[32]{};std::snprintf(copied_name,sizeof(copied_name),"name=%016llx",static_cast<unsigned long long>(object.name));std::snprintf(copied_class,sizeof(copied_class),"class=%016llx",static_cast<unsigned long long>(type.name));
            check(reason.find(scalar)!=std::string::npos&&reason.find(copied_name)!=std::string::npos&&reason.find(copied_class)!=std::string::npos&&reason.find("class_avail=1")!=std::string::npos&&reason.size()<192,"RF diagnostic copies expected/current scalar and all64 name/class bits within result bound");}
        check(refused&&cold_invalid_metadata_reads==0&&cold_target_rf_reads==1,"RF failure performs one original getter and no postfailure name/class/Outer query");
        object.flags=original;cold_metadata_trap=nullptr;cold_rf_missing=nullptr;
    };
    rf_case(owner,1,"object","schema","rf=00000000/00000001 avail=1",false);
    rf_case(type,1,"class","schema","rf=00000000/00000001 avail=1",false);
    rf_case(owner,0x40000000u,"object","schema","rf=00000000/40000000 avail=1",false);
    rf_case(type,0x40000000u,"class","schema","rf=00000000/40000000 avail=1",false);
    rf_case(owner,0,"object","schema","rf=00000000/00000000 avail=0",true);
    plan.validate();check(true,"restored original metadata still passes unchanged RF predicate");
    // Exercise the production snapshot constructor's tags, including a
    // weapon Owner ancestor that is not one of the current owned pawns.
    LookupState provenance_lookup;provenance_lookup.world=level_lookup.world;provenance_lookup.gi=level_lookup.gi;
    provenance_lookup.world_node=level_lookup.world_node;provenance_lookup.gi_node=level_lookup.gi_node;provenance_lookup.gi_property=level_lookup.gi_property;
    const auto previous_lookup=active_lookup;active_lookup=&provenance_lookup;
    WeaponProperty provenance_property;provenance_property.owner_path=owner_path;provenance_property.prefix={&passport_field};provenance_property.field=bound;
    WeaponRosterSnapshot provenance;provenance.schema=schema;provenance.vector=schema;provenance.color=schema;provenance.class_path=class_path;
    WeaponPawnSnapshot provenance_row;provenance_row.own=1;provenance_row.world_path=weapon_path_pure(&world);provenance_row.controller_path=provenance_row.world_path;provenance_row.pawn_path=weapon_path_pure(&class_b);provenance_row.properties.fill(provenance_property);provenance.pawns.push_back(provenance_row);
    WeaponActor provenance_actor;provenance_actor.path=owner_path;provenance_actor.bound=true;provenance_actor.owner={cold_weak(&class_a),reinterpret_cast<uint64_t>(&class_a)};
    class_a.outer=&gi;provenance_actor.owner_path=weapon_path_pure(&class_a);provenance_actor.level_path=provenance_row.world_path;provenance_actor.owner_level_path=provenance_row.world_path;
    provenance_actor.actor_class_path=class_path;provenance_actor.get_owner_path=class_path;provenance_actor.passport=provenance_property;provenance_actor.owner_field=provenance_property;provenance_actor.world_field=provenance_property;provenance_actor.owner_world_field=provenance_property;provenance.actors.push_back(provenance_actor);
    HsmpGameplayWeaponPassport provenance_output{};provenance.output.push_back(provenance_output);WeaponBoundaryPlan provenance_plan(provenance);
    GameplayPawn current_original;current_original.pawn={cold_weak(&class_b),reinterpret_cast<uint64_t>(&class_b)};gameplay_pawns.emplace(19,current_original);
    const auto provenance_case=[&](ColdObject& object,const char* expected){
        object.flags=0x40000000u;cold_metadata_trap=&object;cold_target_rf_reads=0;cold_invalid_metadata_reads=0;bool refused{};
        try{provenance_plan.validate();}catch(const Error& error){refused=true;const std::string reason=error.what();check(reason.find(expected)!=std::string::npos&&reason.find("rf=00000000/40000000 avail=1")!=std::string::npos&&reason.find("class_avail=1")!=std::string::npos&&reason.size()<192,"failed original plan reports copied path row/role/depth and scalar current-pawn membership");}
        check(refused&&cold_target_rf_reads==1&&cold_invalid_metadata_reads==0,"provenance performs no getter after original RF refusal");object.flags=0;cold_metadata_trap=nullptr;
    };
    provenance_case(class_b,"src=pawn row=0 own=1 depth=0 pawn=1");
    provenance_case(class_a,"src=weapon_owner row=0 own=1 depth=0 pawn=0");
    provenance_case(gi,"src=weapon_owner row=0 own=1 depth=1 pawn=0");
    const auto first_origin=provenance_plan.origins.at(reinterpret_cast<uint64_t>(&class_a));provenance_plan.path(provenance_actor.owner_path,"later",31,0);
    check(std::string(first_origin.source)=="weapon_owner"&&std::string(provenance_plan.origins.at(reinterpret_cast<uint64_t>(&class_a)).source)=="weapon_owner","agreeing duplicate path keeps first deterministic provenance without changing strict witness merge");
    class_a.outer=nullptr;gameplay_pawns.erase(19);active_lookup=previous_lookup;
    rejects([&](){weapon_boundary_passes(plan,[&](){owner.name^=1;});},"actual second metadata pass catches original mutation during raw-link span");owner.name^=1;
    passport_field.flags^=1;rejects([&](){plan.validate();},"next native boundary refuses a field flags mutation");passport_field.flags^=1;
    passport_field.dimension=2;rejects([&](){plan.validate();},"next native boundary refuses original ArrayDim mutation");passport_field.dimension=1;
    code[2]^=1;rejects([&](){plan.validate();},"next native boundary refuses exact alignment target code mutation");code[2]^=1;
    auto conflict=bound;conflict.alignment_code[0]^=1;rejects([&](){plan.field(conflict);},"same field pointer with conflicting code expectation refuses");
    conflict=bound;conflict.alignment=16;rejects([&](){plan.field(conflict);},"same field pointer with conflicting alignment expectation refuses");
    auto wrong_schema=schema;wrong_schema.size=250;rejects([&](){plan.schema(wrong_schema);},"same schema with conflicting original raw extent refuses");
    rejects([&](){plan.chain(owner_path,{&passport_field,&replacement},true);},"complete chain cardinality conflict refuses");
    WeaponBoundaryPlan prefix;prefix.chain(owner_path,{&passport_field},false);prefix.chain(owner_path,{&passport_field,&replacement},false);
    check(prefix.chains.begin()->second.fields.size()==2&&!prefix.chains.begin()->second.complete,"agreeing prefixes retain all original field dependencies");
    auto changed_path=owner_path;changed_path.original.front().name^=uint64_t(1)<<32;rejects([&](){plan.path(changed_path);},"copied original FName Number conflict refuses");
    changed_path=owner_path;changed_path.class_flags[0]^=1;rejects([&](){plan.path(changed_path);},"copied original class RF conflict refuses");
    WeaponRosterSnapshot original_roster;original_roster.plan=std::make_shared<const WeaponBoundaryPlan>(plan);check(weapon_boundary_active(original_roster),"only original operation may use copied plan");
    {LookupState nested;WeaponLookupScope nested_scope(nested);check(!weapon_boundary_active(original_roster),"nested operation cannot inherit a validated original plan");}
    check(weapon_boundary_active(original_roster)&&active_lookup==&level_lookup,"nested scope restores original configuration without caching admission");
    owner.name^=1;rejects([&](){original_roster.plan->validate();},"original proof after nested return remains fresh");owner.name^=1;
    cold_retired=&owner;cold_stale_reads=0;rejects([&](){plan.validate();},"expired original metadata refuses before any field or raw link");check(cold_stale_reads==0,"expired original plan never reads stale object metadata");cold_retired=nullptr;
    WeaponRosterSnapshot links;WeaponPawnSnapshot row;row.handle=11;row.world=lookup.world;row.pawn={cold_weak(&class_a),reinterpret_cast<uint64_t>(&class_a)};row.controller={cold_weak(&class_b),reinterpret_cast<uint64_t>(&class_b)};
    for(size_t i=0;i<7;++i)row.properties[i].field.offset=static_cast<int32_t>(offsetof(ColdObject,payload)+8*i);links.pawns.push_back(row);
    GameplayPawn entry;entry.stage=3;entry.world=row.world;entry.pawn=row.pawn;entry.controller=row.controller;entry.applied=GameplayApplied{};gameplay_pawns.emplace(row.handle,std::move(entry));
    WeaponActor actor;actor.bound=true;actor.actor=schema.object;actor.level=row.controller;actor.owner={cold_weak(&gi),reinterpret_cast<uint64_t>(&gi)};actor.owner_address=actor.owner.address;actor.owner_level=actor.level;
    actor.owner_field.field.offset=static_cast<int32_t>(offsetof(ColdObject,gi));actor.world_field.field.offset=actor.owner_field.field.offset;actor.owner_world_field.field.offset=actor.owner_field.field.offset;actor.passport.field.offset=static_cast<int32_t>(offsetof(ColdObject,payload));
    owner.outer=&class_b;owner.gi=&gi;gi.outer=&class_b;class_b.gi=&world;links.actors.push_back(actor);weapon_roster_links(links);check(true,"dedup raw kernel preserves complete alias/owner/level/world/value closure");
    uint64_t changed_alias=99;std::memcpy(class_a.payload.data()+48,&changed_alias,8);rejects([&](){weapon_roster_links(links);},"seventh original alias mutation refuses");class_a.payload.fill(0);
    owner.gi=nullptr;rejects([&](){weapon_roster_links(links);},"original actor owner mutation refuses");owner.gi=&gi;
    class_b.gi=&gi;rejects([&](){weapon_roster_links(links);},"original owning world mutation refuses");class_b.gi=&world;
    owner.payload[0xf8]=1;rejects([&](){weapon_roster_links(links);},"last passport field Tier mutation refuses");owner.payload[0xf8]=0;
    // Configuration has no applied-result values. Each application acquires
    // its own fresh values/class leaves before any conversion callback.
    auto current_schema=schema;current_schema.path=weapon_path_pure(&owner);WeaponBoundaryPlan current_plan;current_plan.schema(current_schema);
    links.schema=current_schema;links.vector=current_schema;links.color=current_schema;links.class_path=class_path;links.pawns[0].own=1;gameplay_entry(row.handle).own=1;
    HsmpGameplayWeaponPassport config_output{};config_output.pawn_index=0;config_output.name.length=1;config_output.name.data[0]='x';links.output.push_back(config_output);
    links.actors[0].values.tier=93;links.actors[0].classes[0]=paths[0];const auto config=weapon_configuration_copy(links);
    const uint64_t config_handles[]{row.handle};auto warm=weapon_configuration_snapshot(config,config_handles,1);
    check(warm.require_applied&&warm.actors[0].bound&&!warm.actors[0].values_bound&&warm.actors[0].values.tier==0&&warm.actors[0].classes[0].pinned.empty(),"prepared metadata excludes old values and class-leaf witnesses");
    check(warm.output[0].name.length==0&&!warm.plan,"prepared metadata excludes converted strings and prior operation plan");
    owner.payload[0xf8]=37;const uint64_t live_class=reinterpret_cast<uint64_t>(&class_a);std::memcpy(owner.payload.data(),&live_class,8);
    field_name_reads=0;std::vector<int> capture_order;
    weapon_boundary_passes(current_plan,[&]{capture_order.push_back(field_name_reads);weapon_roster_links(warm);weapon_capture_value_rows(warm);});
    check(capture_order==std::vector<int>({1})&&field_name_reads==2,"shared capture boundary performs fresh metadata before and after census without a standalone alias pass");
    check(warm.actors[0].values_bound&&warm.actors[0].values.tier==37&&warm.actors[0].values.classes[0]==live_class&&warm.actors[0].classes[0].pinned.front().address==live_class,"warm row captures current all25 values and original live class rather than bootstrap values");
    auto rejected_capture=weapon_configuration_snapshot(config,config_handles,1);uint64_t changed_before_capture=99;std::memcpy(class_a.payload.data()+48,&changed_before_capture,8);
    rejects([&]{weapon_boundary_passes(current_plan,[&]{weapon_roster_links(rejected_capture);weapon_capture_value_rows(rejected_capture);});},"retained shared boundary rejects seventh alias mutation before passport capture");
    check(!rejected_capture.actors[0].values_bound,"failed original alias proof cannot produce a fresh value census");class_a.payload.fill(0);
    rejects([&]{weapon_boundary_passes(current_plan,[&]{weapon_roster_links(rejected_capture);weapon_capture_value_rows(rejected_capture);owner.name^=1;});},"actual shared second metadata pass rejects an original mutation after value capture");owner.name^=1;
    check(rejected_capture.actors[0].values_bound&&!gameplay_weapon_snapshot,"failed postcapture closure publishes no successful passport snapshot");
    owner.payload[0xf8]=38;rejects([&](){weapon_roster_links(warm);},"later callback mutation refuses the new per-result passport");
    auto next=weapon_configuration_snapshot(config,config_handles,1);weapon_roster_links(next);weapon_capture_value_rows(next);
    check(next.actors[0].values.tier==38&&next.actors[0].classes[0].pinned.front().address==live_class,"next result recaptures changed values without metadata rebinding or old-value acceptance");
    const auto old_world=gameplay_entry(row.handle).world;gameplay_entry(row.handle).world={old_world.weak,old_world.address+8};rejects([&](){weapon_roster_links(next);},"prepared binding rejects a different original world");gameplay_entry(row.handle).world=old_world;
    owner.name^=1;rejects([&](){current_plan.validate();},"retained metadata is freshly checked after configuration reuse");owner.name^=1;
    gameplay_entry(row.handle).applied.reset();rejects([&](){weapon_roster_links(next);},"prepared metadata cannot substitute an applied native result");
    links.require_applied=false;links.actors[0].values_bound=false;weapon_roster_links(links);check(true,"bootstrap-only no-applied mode qualifies original links without publishing a result");
    owner.payload.fill(0);
    gameplay_pawns.erase(row.handle);owner.outer=nullptr;owner.gi=nullptr;gi.outer=nullptr;class_b.gi=nullptr;
    cold_poisoned_field=nullptr;active_lookup=nullptr;vt=nullptr;object_name=nullptr;retirement_flags=nullptr;source_outer=nullptr;source_package_name=nullptr;weapon_api={};identities.clear();cold_objects.clear();
}
void configuration_lifecycle_cases(){
    const auto previous_api=weapon_api;weapon_api={};LookupState operation;active_lookup=&operation;
    GameplayPawn remote;remote.stage=3;remote.roster_count=2;remote.world={1,100};remote.controller={2,200};remote.pawn={3,300};gameplay_pawns.emplace(10,remote);
    gameplay_weapon_configuration.reset();gameplay_weapon_snapshot.reset();
    gameplay_weapons_prepare(10,nullptr);
    check(!gameplay_weapon_configuration&&!gameplay_weapon_snapshot&&!weapon_api.module,"remote-first finish defers before metadata discovery or publishing configuration/result proof");
    gameplay_entry(10).own=1;gameplay_weapons_prepare(10,nullptr);
    check(!gameplay_weapon_configuration&&!gameplay_weapon_snapshot&&!weapon_api.module,"owned-first short roster also defers all metadata discovery and publication");gameplay_entry(10).own=0;
    gameplay_entry(10).roster_count=0;rejects([&]{weapon_prepare_roster(gameplay_entry(10));},"missing original expected roster count refuses");
    gameplay_entry(10).roster_count=33;rejects([&]{weapon_prepare_roster(gameplay_entry(10));},"original expected roster count above32 refuses");gameplay_entry(10).roster_count=2;
    GameplayPawn owned=remote;owned.own=1;owned.stage=2;owned.pawn={4,400};gameplay_pawns.emplace(20,owned);
    check(!weapon_prepare_roster(gameplay_entry(10)),"unfinished owned row keeps preparation deferred");gameplay_entry(20).stage=3;
    check(weapon_prepare_roster(gameplay_entry(10)),"complete same-original roster permits explicit bootstrap preparation");
    gameplay_entry(10).stage=2;check(!weapon_prepare_roster(gameplay_entry(20)),"any unfinished original row defers complete-roster discovery");gameplay_entry(10).stage=3;
    gameplay_entry(20).roster_count=1;rejects([&]{weapon_prepare_roster(gameplay_entry(10));},"different stored original roster count cannot be adopted");gameplay_entry(20).roster_count=2;
    auto excess=remote;excess.pawn={5,500};gameplay_pawns.emplace(30,excess);rejects([&]{weapon_prepare_roster(gameplay_entry(10));},"actual roster exceeding original expected count refuses before discovery");gameplay_pawns.erase(30);
    gameplay_entry(20).own=0;rejects([&]{weapon_prepare_roster(gameplay_entry(10));},"complete original roster without an owned pawn refuses instead of waiting forever");gameplay_entry(20).own=1;
    gameplay_entry(10).roster_count=3;gameplay_entry(20).roster_count=3;gameplay_entry(10).own=1;
    rejects([&]{weapon_prepare_roster(gameplay_entry(10));},"incomplete roster with multiple owned pawns retains immediate original refusal");gameplay_entry(10).own=0;gameplay_entry(10).roster_count=2;gameplay_entry(20).roster_count=2;
    WeaponConfiguration config;WeaponPawnSnapshot a;a.handle=10;a.world=remote.world;a.controller=remote.controller;a.pawn=remote.pawn;
    auto b=a;b.handle=20;b.pawn=owned.pawn;b.own=1;config.pawns={a,b};const uint64_t handles[]{10,20},reversed[]{20,10};
    const auto copied=weapon_configuration_snapshot(config,handles,2);check(copied.pawns[0].handle==10&&copied.pawns[1].handle==20&&copied.require_applied,"prepared original ordered handles retained for each new operation");
    rejects([&](){weapon_configuration_snapshot(config,reversed,2);},"reordered prepared handles refuse rather than silently rebind");
    rejects([&](){weapon_configuration_snapshot(config,handles,1);},"incomplete prepared roster refuses");
    gameplay_entry(20).controller.address+=8;rejects([&](){weapon_prepare_roster(gameplay_entry(10));},"different controller cannot seed prepared metadata");gameplay_entry(20).controller=remote.controller;
    gameplay_entry(10).own=1;rejects([&](){weapon_prepare_roster(gameplay_entry(10));},"ambiguous owned bootstrap refuses before discovery");gameplay_entry(10).own=0;
    gameplay_weapon_configuration=std::make_shared<const WeaponConfiguration>(config);auto result=std::make_shared<WeaponRosterSnapshot>();result->pawns=config.pawns;gameplay_weapon_snapshot=result;
    const auto retained=gameplay_weapon_configuration;gameplay_weapons_discard(10);
    check(!gameplay_weapon_snapshot&&gameplay_weapon_configuration==retained,"per-apply result discard preserves original metadata only");
    gameplay_weapons_config_discard(99);check(gameplay_weapon_configuration==retained,"unrelated handle cannot replace original metadata profile");
    gameplay_weapons_config_discard(20);check(!gameplay_weapon_configuration&&retained->pawns.size()==2,"explicit lifecycle invalidation drops publication but keeps in-flight copied ownership alive");
    gameplay_weapon_configuration=retained;gameplay_weapons_reset();check(!gameplay_weapon_configuration&&!gameplay_weapon_snapshot&&!weapon_api.module,"begin/reset invalidates both profile and per-result proof");
    gameplay_pawns.clear();active_lookup=nullptr;weapon_api=previous_api;
}
struct TimingRow {uint32_t complete{},attempt{},phase{};uint64_t us{};std::string label;};
std::vector<TimingRow> weapon_timing_rows;bool weapon_timing_unsafe{};
void weapon_timing_log(const char* stage,uint32_t edge,uint64_t us,uint32_t attempt,uint32_t phase,uint32_t kind,uint32_t id,const char* label){
    if(std::strcmp(stage,"gameplay_weapons_us")!=0||kind||id||active_lookup||active_guard||weapon_active_roster||gameplay_boundary||weapon_batch_timing_active)weapon_timing_unsafe=true;
    if(gameplay_mutex.try_lock())gameplay_mutex.unlock();else weapon_timing_unsafe=true;
    weapon_timing_rows.push_back({edge,attempt,phase,us,label});
}
void timing_cases(){
    const auto previous=create_logger.load();create_logger.store(weapon_timing_log);weapon_batch_attempts=0;weapon_batch_timing_active=false;weapon_timing_rows.clear();weapon_timing_unsafe=false;
    try{WeaponBatchTrace failed;const std::lock_guard lock(gameplay_mutex);failed.advance(2);throw Error("fixture batch failure");}catch(const Error&){}
    check(!weapon_batch_timing_active&&weapon_timing_rows.size()==6&&!weapon_timing_unsafe,"failed batch timing flushes once after mutex and TLS unwind");
    check(weapon_timing_rows[0].complete==2&&weapon_timing_rows[2].attempt==1&&weapon_timing_rows[2].phase==2,"failed profile retains original stage and explicit error edge");
    check(weapon_timing_rows[1].phase==1&&weapon_timing_rows[1].us==0,"shared alias validation has no standalone phase duration when capture starts at phase2");
    for(uint32_t attempt=2;attempt<=8;++attempt){WeaponBatchTrace outer;const std::lock_guard lock(gameplay_mutex);
        {WeaponBatchTrace nested;check(!nested.enabled,"nested batch excluded from sample budget");}
        for(uint32_t phase=1;phase<5;++phase)outer.advance(phase);outer.complete=true;
    }
    check(weapon_batch_attempts==8&&weapon_timing_rows.size()==48&&!weapon_timing_unsafe,"eight batch profiles emit at most48 fixed copied rows");
    check(weapon_timing_rows[47].complete==1&&weapon_timing_rows[47].phase==5&&weapon_timing_rows[47].label=="instrumented_total_inclusive","successful copied total is explicitly inclusive");
    {WeaponBatchTrace exhausted;check(!exhausted.enabled,"later batches have no profile clock or output");}
    const auto emitted=weapon_timing_rows.size();weapon_batch_attempts=0;LookupState unrelated;active_lookup=&unrelated;
    {WeaponBatchTrace nested_scope;check(!nested_scope.enabled&&weapon_batch_attempts==0,"unrelated native operation does not borrow profile or consume budget");}active_lookup=nullptr;
    create_logger.store(nullptr);{WeaponBatchTrace disabled;check(!disabled.enabled&&weapon_batch_attempts==0,"missing logger has negligible disabled path");}
    check(weapon_timing_rows.size()==emitted,"excluded/disabled batches emit no rows");create_logger.store(previous);weapon_batch_attempts=0;weapon_batch_timing_active=false;
}
}
int main(){try{metadata_cases();value_cases();text_cases();alignment_cases();alias_capacity_cases();schema_layout_cases();cold_path_cases();configuration_lifecycle_cases();timing_cases();check(std::size(weapon_fields)==25,"complete25 fields");check(sizeof(HsmpGameplay)==88&&offsetof(HsmpGameplay,weapons)==72&&offsetof(HsmpGameplay,initialized)==80&&gameplay_provider.abi==6,"ABI6 retains weapon/initializer offsets while qualifying original roster count");
    std::cout<<"native_gameplay_weapons: "<<checks<<" checks passed\n";return 0;}catch(const std::exception& e){std::cerr<<"native_gameplay_weapons: "<<e.what()<<'\n';return 1;}}
