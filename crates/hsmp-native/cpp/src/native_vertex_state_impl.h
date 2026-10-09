// Matched shipping static override slots; never dereference a color buffer.
// Reflection admits the complete TArray/struct layout, and code signatures pin
// the private slot semantics to the independently disassembled game build.
using VertexFlags=const uint32_t*(*)(const void*);
VertexFlags vertex_flags{};
constexpr uint8_t vertex_count_code[]={0x48,0x8b,0x87,0x88,0x05,0,0,0x48,0x8d,0x0c,0xf6,0x48,0x03,0xc9,0x48,0x8b,0x44,0xc8,0x30};
constexpr uint8_t vertex_colors_code[]={0x49,0x8b,0x87,0x88,0x05,0,0,0x4a,0x8d,0x0c,0xe5,0,0,0,0,0x49,0x03,0xcc,0x48,0x03,0xc9,0x48,0x8b,0x4c,0xc8,0x30};
bool vertex_code_ranges(const uint8_t* count,size_t count_bytes,const uint8_t* colors,size_t color_bytes){
    return count&&colors&&count_bytes==sizeof(vertex_count_code)&&color_bytes==sizeof(vertex_colors_code)&&
        std::memcmp(count,vertex_count_code,count_bytes)==0&&std::memcmp(colors,vertex_colors_code,color_bytes)==0;
}
bool vertex_code_match(const uint8_t* image,size_t bytes) {
    constexpr size_t a=0x4a697b7,b=0x4a6d26d;
    return image&&bytes>=b+sizeof(vertex_colors_code)&&vertex_code_ranges(image+a,sizeof(vertex_count_code),image+b,sizeof(vertex_colors_code));
}
bool vertex_shipping_build() {
    const auto* image=reinterpret_cast<const uint8_t*>(GetModuleHandleW(nullptr));if(!image)return false;
    const auto* dos=reinterpret_cast<const IMAGE_DOS_HEADER*>(image);
    if(dos->e_magic!=IMAGE_DOS_SIGNATURE||dos->e_lfanew<0||dos->e_lfanew>65536)return false;
    const auto* pe=reinterpret_cast<const IMAGE_NT_HEADERS64*>(image+dos->e_lfanew);
    return pe->Signature==IMAGE_NT_SIGNATURE&&pe->FileHeader.Machine==IMAGE_FILE_MACHINE_AMD64&&
        pe->OptionalHeader.Magic==IMAGE_NT_OPTIONAL_HDR64_MAGIC&&vertex_code_match(image,pe->OptionalHeader.SizeOfImage);
}
bool (*vertex_build_admit)()=vertex_shipping_build;
uintptr_t vertex_empty_image{};
bool vertex_empty_shipping_build(){
    const auto* image=reinterpret_cast<const uint8_t*>(GetModuleHandleW(nullptr));if(!image)return false;
    const auto* dos=reinterpret_cast<const IMAGE_DOS_HEADER*>(image);if(dos->e_magic!=IMAGE_DOS_SIGNATURE||dos->e_lfanew<0||dos->e_lfanew>65536)return false;
    const auto* pe=reinterpret_cast<const IMAGE_NT_HEADERS64*>(image+dos->e_lfanew);
    if(pe->Signature!=IMAGE_NT_SIGNATURE||pe->FileHeader.Machine!=IMAGE_FILE_MACHINE_AMD64||pe->OptionalHeader.Magic!=IMAGE_NT_OPTIONAL_HDR64_MAGIC||pe->OptionalHeader.SizeOfImage<0x76706d0)return false;
    const auto match=[&](size_t at,std::initializer_list<uint8_t> bytes){return std::equal(bytes.begin(),bytes.end(),image+at);};
    if(!match(0x3956692,{0x48,0x8b,0x89,0x60,0x05,0,0,0x48,0x85,0xc9,0x0f,0x84,0x4e,0x02,0,0})||
       !match(0x39568f0,{0x33,0xc0})||!match(0x3c58b10,{0x48,0x8b,0x89,0x60,0x05,0,0,0x48,0x85,0xc9,0x74,0x44})||
       !match(0x3c58b65,{0x33,0xc0,0x48,0x83,0xc4,0x20,0x5f,0xc3})||
       !match(0x3c58cc3,{0x44,0x8b,0xcd,0x4c,0x8b,0xc3,0x48,0x8b,0xd7,0x48,0x8b,0xce,0xe8,0x4c,0x89,0xf9,0xff})||
       !match(0x3b51630,{0x80,0x79,0x3b,0,0x0f,0x95,0xc0,0xc3}))return false;
    const auto base=reinterpret_cast<uintptr_t>(image);uint64_t entry{};
    for(const auto [slot,target]:{std::pair<size_t,size_t>{0x4d0,0x3c58b70},{0x7d8,0x3956670},{0x3e8,0x3b51630}}){
        std::memcpy(&entry,image+0x766fc60+slot,8);if(entry!=base+target)return false;}
    vertex_empty_image=base;return true;
}
bool (*vertex_empty_build_admit)()=vertex_empty_shipping_build;
uint64_t vertex_empty_vtable(const void* object){uint64_t out{};std::memcpy(&out,object,8);return out;}
uint64_t (*vertex_empty_vtable_read)(const void*)=vertex_empty_vtable;
void* vertex_live(Obj object) {
    require(vertex_flags!=nullptr,"native vertex flags unavailable");void* p=get(object);
    const auto* flags=vertex_flags(p);require(flags&&(*flags&0x40000000u)==0,"native vertex original object garbage");
    const auto cls=keep(vt->class_of(p));void* class_pointer=get(cls);flags=vertex_flags(class_pointer);
    require(flags&&(*flags&0x40000000u)==0,"native vertex original class garbage");get(cls);
    p=get(object);flags=vertex_flags(p);require(flags&&(*flags&0x40000000u)==0,"native vertex original object changed");return p;
}
uintptr_t vertex_skeletal_image{};
bool vertex_skeletal_shipping_build(){
    const auto* image=reinterpret_cast<const uint8_t*>(GetModuleHandleW(nullptr));if(!image)return false;
    const auto* dos=reinterpret_cast<const IMAGE_DOS_HEADER*>(image);if(dos->e_magic!=IMAGE_DOS_SIGNATURE||dos->e_lfanew<0||dos->e_lfanew>65536)return false;
    const auto* pe=reinterpret_cast<const IMAGE_NT_HEADERS64*>(image+dos->e_lfanew);
    if(pe->Signature!=IMAGE_NT_SIGNATURE||pe->FileHeader.Machine!=IMAGE_FILE_MACHINE_AMD64||pe->OptionalHeader.Magic!=IMAGE_NT_OPTIONAL_HDR64_MAGIC||pe->OptionalHeader.SizeOfImage<0x765a7b0)return false;
    const auto match=[&](size_t at,std::initializer_list<uint8_t> bytes){return std::equal(bytes.begin(),bytes.end(),image+at);};
    if(!match(0x3c1c116,{0x48,0x8b,0x81,0xa0,0x07,0,0})||!match(0x3c1c12f,{0x48,0x8b,0x89,0x58,0x05,0,0})||
       !match(0x3c1c14c,{0x48,0x8d,0xb7,0x60,0x05,0,0})||!match(0x3c1c173,{0x48,0x8b,0x5c,0x24,0x30,0x33,0xc0})||
       !match(0x3c13705,{0x48,0x85,0xf6,0x0f,0x84,0x82,0,0,0})||
       !match(0x3bd1403,{0xff,0x90,0xd8,0x07,0,0,0x48,0x89,0x83,0xf0,0x02,0,0,0x48,0x89,0x83,0xf8,0x04,0,0})||
       !match(0x4a698d6,{0x48,0x8b,0x87,0x60,0x07,0,0,0x48,0x8d,0x0c,0xb6,0x48,0x8b,0x44,0xc8,0x10})||
       !match(0x3c1a7c4,{0x3b,0xb1,0x20,0x05,0,0})||!match(0x3c1a7cc,{0x48,0x8b,0x81,0x18,0x05,0,0})||
       !match(0x3bba485,{0x41,0x3b,0xdc,0x7d,0x1f})||!match(0x3bba4c5,{0x41,0x89,0x46,0x08})||
       !match(0x3bd11dc,{0x4d,0x85,0xc0,0x74,0x09})||!match(0x3bd11f2,{0xff,0x90,0x38,0x06,0,0})||
       !match(0x3c1ac64,{0x83,0xb9,0xa0,0x05,0,0,0,0x7e,0x32})||!match(0x3c1ac79,{0x48,0x8b,0x99,0x98,0x05,0,0,0x48,0x8b,0x1b})||!match(0x3c1ac9f,{0x33,0xc0})||
       !match(0x3c15d16,{0xb8,0xff,0xff,0xff,0xff})||!match(0x3b51630,{0x80,0x79,0x3b,0,0x0f,0x95,0xc0,0xc3})||
       !match(0x3c0efa5,{0x80,0x89,0x52,0x0a,0,0,8})||
       !match(0x3c0c325,{0x0f,0xb6,0x81,0x52,0x0a,0,0,0xc0,0xe8,3,0x24,1}))return false;
    const auto base=reinterpret_cast<uintptr_t>(image);uint64_t entry{};
    for(const auto [slot,target]:{std::pair<size_t,size_t>{0x4d0,0x3c1cbc0},{0x7d8,0x3c136b0},{0x7c0,0x3c1b6f0},{0x638,0x3c1a7b0},{0x668,0x3bba450},{0x688,0x3bd11c0},{0x3e8,0x3b51630}}){
        std::memcpy(&entry,image+0x7659cb0+slot,8);if(entry!=base+target)return false;}
    vertex_skeletal_image=base;return true;
}
bool (*vertex_skeletal_build_admit)()=vertex_skeletal_shipping_build;
uint8_t vertex_deformer_mask{};
void vertex_deformer_layout();
void vertex_empty_profile(Obj component,uint32_t kind=1){
    if(kind==0){
        require(vertex_skeletal_build_admit&&vertex_skeletal_build_admit(),"native empty skeletal shipping profile unsupported");
        const auto cls=keep(vt->class_of(vertex_live(component)));require(same(cls,find(L"/Script/Engine.SkeletalMeshComponent")),"native empty exact skeletal class required");
        std::array<HsmpProp,256> props{};int32_t size{};const auto count=vt->props(vertex_live(cls),props.data(),256,&size);
        require(count>=0&&count<=256&&size==0xf70,"native empty skeletal reflected size");vertex_live(cls);
        require(vertex_empty_vtable_read(vertex_live(component))==vertex_skeletal_image+0x7659cb0,"native empty original skeletal vtable");
        require(property(component,L"SkeletalMesh",L"ObjectProperty",8).offset==0x558&&property(component,L"SkinnedAsset",L"ObjectProperty",8).offset==0x560&&
            property(component,L"LeaderPoseComponent",L"WeakObjectProperty",8).offset==0x568&&property(component,L"PhysicsAssetOverride",L"ObjectProperty",8).offset==0x738&&
            property(component,L"MeshDeformer",L"ObjectProperty",8).offset==0x588&&property(component,L"bSetMeshDeformer",L"BoolProperty",1).offset==0x580&&
            property(component,L"MeshDeformerInstances",L"StructProperty",0x20).offset==0x598&&
            property(component,L"AnimBlueprintGeneratedClass",L"ClassProperty",8).offset==0x8c0&&property(component,L"AnimClass",L"ClassProperty",8).offset==0x8c8&&
            property(component,L"AnimScriptInstance",L"ObjectProperty",8).offset==0x8d0&&property(component,L"PostProcessAnimInstance",L"ObjectProperty",8).offset==0x8d8&&
            property(component,L"ClothingInteractor",L"ObjectProperty",8).offset==0xc40,"native empty skeletal reflected assets/state");
        for(const auto [field,offset,mask]:{std::tuple<const wchar_t*,int32_t,uint8_t>{L"bAllowClothActors",0xa42,uint8_t{2}},{L"bDisableClothSimulation",0xa42,uint8_t{4}},{L"bDisablePostProcessBlueprint",0xa41,uint8_t{1}}}){
            const auto p=property(component,field,L"BoolProperty",1);require(p.offset==offset&&p.bool_offset==0&&p.bool_mask==mask,"native empty skeletal reflected simulation flags");}
        const auto deformer_flag=property(component,L"bSetMeshDeformer",L"BoolProperty",1);require(deformer_flag.bool_offset==0&&deformer_flag.bool_mask!=0,"native empty deformer bool layout");
        vertex_deformer_mask=deformer_flag.bool_mask;vertex_deformer_layout();return;
    }
    require(kind==1,"native vertex component kind unsupported");
    require(vertex_empty_build_admit&&vertex_empty_build_admit(),"native empty static shipping profile unsupported");
    const auto cls=keep(vt->class_of(vertex_live(component)));require(same(cls,find(L"/Script/Engine.StaticMeshComponent")),"native empty exact static class required");
    std::array<HsmpProp,64> props{};int32_t size{};const auto count=vt->props(vertex_live(cls),props.data(),64,&size);
    require(count>=0&&count<=64&&size==0x5e0,"native empty static reflected size");vertex_live(cls);
    require(vertex_empty_vtable_read(vertex_live(component))==vertex_empty_image+0x766fc60,"native empty original static vtable");
}
thread_local Obj vertex_owner{},vertex_component{},vertex_asset{};
thread_local bool vertex_empty{};
thread_local uint32_t vertex_kind{1};
struct VertexOperation {
    Obj owner{},component{},mesh{};bool empty{vertex_empty};uint32_t kind{vertex_kind};
    VertexOperation(Obj next_owner,Obj next_component,bool allow_empty=false):owner(vertex_owner),component(vertex_component),mesh(vertex_asset){
        if(!next_owner.weak)return;
        vertex_live(next_owner);vertex_live(next_component);
        if(is(next_component,L"/Script/Engine.SkeletalMeshComponent")){
            require(allow_empty,"native skeletal absence-only proof");vertex_empty_profile(next_component,0);
            require(!object_property(next_component,L"SkeletalMesh").weak&&!object_property(next_component,L"SkinnedAsset").weak,"native empty skeletal original asset present");
            vertex_owner=next_owner;vertex_component=next_component;vertex_asset={};vertex_empty=true;vertex_kind=0;return;
        }
        require(is(next_component,L"/Script/Engine.StaticMeshComponent"),"native vertex exact mesh component required");
        require(property(next_component,L"StaticMesh",L"ObjectProperty",8).offset==0x560,"native vertex StaticMesh reflected offset");
        auto next_asset=object_property(next_component,L"StaticMesh");
        if(next_asset.weak){vertex_live(next_asset);require(is(next_asset,L"/Script/Engine.StaticMesh"),"native vertex static asset class");
            const auto* flags=vertex_flags(vertex_live(next_asset));require(flags&&(*flags&0x40u)==0,"native vertex runtime asset unsupported");}
        else{require(allow_empty,"native vertex original asset unavailable");vertex_empty_profile(next_component);}
        vertex_owner=next_owner;vertex_component=next_component;vertex_asset=next_asset;vertex_empty=!next_asset.weak;vertex_kind=1;
    }
    ~VertexOperation(){vertex_owner=owner;vertex_component=component;vertex_asset=mesh;vertex_empty=empty;vertex_kind=kind;}
};
void vertex_call_guard(Obj object,Obj function,Obj cls){
    if(!vertex_owner.weak)return;
    vertex_live(vertex_owner);vertex_live(vertex_component);
    if(vertex_empty)vertex_empty_profile(vertex_component,vertex_kind);
    else{vertex_live(vertex_asset);require((*vertex_flags(vertex_live(vertex_asset))&0x40u)==0,"native vertex runtime asset changed");}
    if(vertex_kind==0)require(!object_property(vertex_component,L"SkeletalMesh").weak&&!object_property(vertex_component,L"SkinnedAsset").weak,"native empty skeletal asset appeared");
    else require(same(object_property(vertex_component,L"StaticMesh"),vertex_asset),"native vertex original asset link changed");
    vertex_live(object);vertex_live(function);vertex_live(cls);
}
HsmpProp vertex_layout(Obj component,uint32_t kind=1) {
    require(vertex_build_admit&&vertex_build_admit(),"native vertex shipping layout unsupported");
    require(spline_api.children&&spline_api.next&&spline_api.inner&&spline_api.structure&&spline_api.size&&spline_api.offset&&
        spline_api.field_name&&spline_api.field_class&&spline_api.variant_name,"native vertex reflected metadata unavailable");
    require(kind<=1,"native vertex layout kind");const auto cls=find(kind?L"/Script/Engine.StaticMeshComponent":L"/Script/Engine.SkinnedMeshComponent"),inner_struct=find(kind?L"/Script/Engine.StaticMeshComponentLODInfo":L"/Script/Engine.SkelMeshComponentLODInfo");
    const auto key_name=kind?L"LODData":L"LODInfo";const int32_t stride=kind?0x90:0x28,offset=kind?0x588:0x760;
    vertex_live(cls);vertex_live(inner_struct);std::array<HsmpProp,1> fields{};int32_t size{};
    require(vt->props(vertex_live(inner_struct),fields.data(),1,&size)==(kind?0:1)&&size==stride,"native vertex LOD info reflected size");vertex_live(inner_struct);
    const auto property_info=property(component,key_name,L"ArrayProperty",16);
    require(property_info.offset==offset,"native vertex LOD reflected offset");
    void* field=*spline_api.children(vertex_live(cls));bool found{};
    for(uint32_t n=0;field;++n){require(n<4096,"native vertex property chain bound");vertex_live(cls);
        SplineName key{};spline_api.field_name(field,&key);
        if(spline_name(key)==name(key_name)){
            SplineVariant variant{};SplineName type{};spline_api.field_class(field,&variant);spline_api.variant_name(&variant,&type);
            require(spline_name(type)==name(L"ArrayProperty")&&*spline_api.size(field)==16&&*spline_api.offset(field)==offset,"native vertex reflected array ABI");
            void* inner=*spline_api.inner(field);require(inner!=nullptr,"native vertex array inner missing");
            spline_api.field_class(inner,&variant);spline_api.variant_name(&variant,&type);
            require(spline_name(type)==name(L"StructProperty")&&*spline_api.size(inner)==stride&&*spline_api.offset(inner)==0&&
                *spline_api.structure(inner)==vertex_live(inner_struct),"native vertex reflected inner ABI");found=true;break;
        }
        vertex_live(cls);field=spline_api.next(field);
    }
    require(found,"native vertex LODData metadata missing");vertex_live(cls);vertex_live(inner_struct);return property_info;
}
void vertex_object_array_layout(Obj cls,const wchar_t* key_name,int32_t offset){
    vertex_live(cls);void* field=*spline_api.children(vertex_live(cls));bool found{};
    for(uint32_t n=0;field;++n){require(n<4096,"native material property chain bound");vertex_live(cls);SplineName key{};spline_api.field_name(field,&key);
        if(spline_name(key)==name(key_name)){
            SplineVariant variant{};SplineName type{};spline_api.field_class(field,&variant);spline_api.variant_name(&variant,&type);
            require(spline_name(type)==name(L"ArrayProperty")&&*spline_api.size(field)==16&&*spline_api.offset(field)==offset,"native material array ABI");
            void* inner=*spline_api.inner(field);require(inner!=nullptr,"native material array inner missing");spline_api.field_class(inner,&variant);spline_api.variant_name(&variant,&type);
            require(spline_name(type)==name(L"ObjectProperty")&&*spline_api.size(inner)==8&&*spline_api.offset(inner)==0,"native material object inner ABI");found=true;break;}
        vertex_live(cls);field=spline_api.next(field);
    }require(found,"native material override metadata missing");vertex_live(cls);
}
void vertex_material_layout(Obj component){
    require(property(component,L"OverrideMaterials",L"ArrayProperty",16).offset==0x518,"native reflected material override offset");
    vertex_object_array_layout(find(L"/Script/Engine.MeshComponent"),L"OverrideMaterials",0x518);
}
void vertex_deformer_layout(){
    require(spline_api.children&&spline_api.next&&spline_api.inner&&spline_api.size&&spline_api.offset&&spline_api.field_name&&spline_api.field_class&&spline_api.variant_name,"native deformer metadata unavailable");
    const auto cls=find(L"/Script/Engine.MeshDeformerInstanceSet");std::array<HsmpProp,1> fields{};int32_t size{};
    require(vt->props(vertex_live(cls),fields.data(),1,&size)==1&&size==0x20&&fields[0].name==name(L"DeformerInstances")&&fields[0].cls==name(L"ArrayProperty")&&fields[0].offset==0&&fields[0].size==16,"native deformer instance set member ABI");
    vertex_object_array_layout(cls,L"DeformerInstances",0);
}
// Final memory admission cannot invoke get/check_guard or a virtual world
// getter. Original native slot/name/class/flags are copied through pure exports.
void* vertex_pure(Obj object){
    require(vt&&vertex_flags&&object_name,"native vertex pure metadata unavailable");
    const auto found=identities.find(object.weak);require(found!=identities.end(),"native vertex original identity missing");const auto& id=found->second;
    void* p=vt->resolve(object.weak);require(object.weak&&p&&reinterpret_cast<uint64_t>(p)==object.address&&id.address==object.address,"native vertex original slot changed");
    const auto* flags=vertex_flags(p);require(flags&&(*flags&0x40000000u)==0,"native vertex original garbage before final copy");
    void* cls=vt->resolve(id.class_weak);require(cls&&reinterpret_cast<uint64_t>(cls)==id.class_address,"native vertex original class slot changed");
    flags=vertex_flags(cls);require(flags&&(*flags&0x40000000u)==0,"native vertex original class garbage before final copy");
    const auto original_class=identities.find(id.class_weak);require(original_class!=identities.end(),"native vertex original class identity missing");
    const auto n=object_name(p),class_name_value=object_name(cls);
    require(n&&*n==id.name&&class_name_value&&*class_name_value==original_class->second.name&&vt->class_of(p)==cls,"native vertex original name/class changed before final copy");return p;
}
void vertex_dispatch_guard(Obj object,Obj function,Obj cls){
    if(!vertex_owner.weak)return;
    vertex_pure(vertex_owner);if(vertex_asset.weak){void* asset_pointer=vertex_pure(vertex_asset);
        require((*vertex_flags(asset_pointer)&0x40u)==0,"native vertex runtime asset before dispatch");}
    void* component_pointer=vertex_pure(vertex_component);uint64_t hard_asset{};
    if(vertex_empty)require(vertex_empty_vtable_read(component_pointer)==(vertex_kind?vertex_empty_image+0x766fc60:vertex_skeletal_image+0x7659cb0),"native empty original vtable before dispatch");
    std::memcpy(&hard_asset,static_cast<const uint8_t*>(component_pointer)+0x560,8);
    require(hard_asset==vertex_asset.address,"native vertex original asset before dispatch");
    if(vertex_kind==0){std::memcpy(&hard_asset,static_cast<const uint8_t*>(component_pointer)+0x558,8);require(!hard_asset,"native empty deprecated skeletal asset before dispatch");}
    vertex_pure(object);vertex_pure(function);vertex_pure(cls);
}
struct VertexSnapshot {uint64_t asset{},deprecated{},mesh_object{},proxy{},proxy_copy{},leader{},clothing_interactor{};Array header{},materials{},deformers{};
    std::array<uint64_t,4> animation{};std::array<uint8_t,16> deformer_tail{};uint8_t cloth{},suspended{},postprocess{},deformer_byte{},deformer_mask{};
    std::array<uint64_t,16> overrides{};std::array<uint64_t,32> material_slots{};uint32_t kind{1};};
VertexSnapshot vertex_copy(const void* component,int32_t offset){
    require(component&&(offset==0x588||offset==0x760),"native vertex copied component layout");VertexSnapshot copy{};copy.kind=offset==0x588?1u:0u;
    std::memcpy(&copy.asset,static_cast<const uint8_t*>(component)+0x560,8);
    if(!copy.kind){
        std::memcpy(&copy.deprecated,static_cast<const uint8_t*>(component)+0x558,8);
        std::memcpy(&copy.mesh_object,static_cast<const uint8_t*>(component)+0x7a0,8);
        std::memcpy(&copy.proxy,static_cast<const uint8_t*>(component)+0x2f0,8);
        std::memcpy(&copy.proxy_copy,static_cast<const uint8_t*>(component)+0x4f8,8);
        std::memcpy(&copy.leader,static_cast<const uint8_t*>(component)+0x568,8);
        std::memcpy(copy.animation.data(),static_cast<const uint8_t*>(component)+0x8c0,32);
        std::memcpy(&copy.cloth,static_cast<const uint8_t*>(component)+0xa42,1);
        copy.suspended=static_cast<const uint8_t*>(component)[0xa52]&8;
        std::memcpy(&copy.postprocess,static_cast<const uint8_t*>(component)+0xa41,1);
        std::memcpy(&copy.clothing_interactor,static_cast<const uint8_t*>(component)+0xc40,8);
        require(!copy.asset&&!copy.deprecated&&!copy.mesh_object&&!copy.proxy&&!copy.proxy_copy,"native empty skeletal asset/render cache present");
        uint64_t physics{};std::memcpy(&physics,static_cast<const uint8_t*>(component)+0x738,8);require(!physics,"native empty skeletal physics override unsupported");
        uint64_t deformer{};std::memcpy(&deformer,static_cast<const uint8_t*>(component)+0x588,8);std::memcpy(&copy.deformer_byte,static_cast<const uint8_t*>(component)+0x580,1);copy.deformer_mask=vertex_deformer_mask;
        std::memcpy(&copy.deformers,static_cast<const uint8_t*>(component)+0x598,sizeof(Array));
        std::memcpy(copy.deformer_tail.data(),static_cast<const uint8_t*>(component)+0x5a8,16);
        require(copy.deformer_mask&&!(copy.deformer_byte&copy.deformer_mask)&&!deformer&&copy.deformers.count==0&&copy.deformers.capacity>=0&&(!copy.deformers.capacity||copy.deformers.data),"native empty skeletal deformer state unsupported");
    }
    std::memcpy(&copy.header,static_cast<const uint8_t*>(component)+offset,sizeof(Array));
    const auto& h=copy.header;
    require(h.count>=0&&h.count<=16&&h.capacity>=h.count&&h.capacity>=0&&(!h.capacity||h.data),"native vertex complete LODData bounds");
    for(int32_t i=0;i<h.count;++i)std::memcpy(&copy.overrides[static_cast<size_t>(i)],static_cast<const uint8_t*>(h.data)+static_cast<size_t>(i)*(copy.kind?0x90:0x28)+(copy.kind?0x30:0x10),8);
    std::memcpy(&copy.materials,static_cast<const uint8_t*>(component)+0x518,sizeof(Array));
    const auto& m=copy.materials;require(m.count>=0&&m.count<=32&&m.capacity>=m.count&&m.capacity>=0&&(!m.capacity||m.data),"native complete material override bounds");
    for(int32_t i=0;i<m.count;++i)std::memcpy(&copy.material_slots[static_cast<size_t>(i)],static_cast<const uint8_t*>(m.data)+static_cast<size_t>(i)*8,8);
    return copy;
}
bool vertex_equal(const VertexSnapshot& a,const VertexSnapshot& b){return a.kind==b.kind&&a.asset==b.asset&&a.deprecated==b.deprecated&&a.mesh_object==b.mesh_object&&a.proxy==b.proxy&&a.proxy_copy==b.proxy_copy&&a.leader==b.leader&&
    a.animation==b.animation&&a.cloth==b.cloth&&a.suspended==b.suspended&&a.postprocess==b.postprocess&&a.clothing_interactor==b.clothing_interactor&&
    a.deformer_byte==b.deformer_byte&&a.deformer_mask==b.deformer_mask&&a.deformer_tail==b.deformer_tail&&
    a.header.data==b.header.data&&a.header.count==b.header.count&&a.header.capacity==b.header.capacity&&a.overrides==b.overrides&&
    a.deformers.data==b.deformers.data&&a.deformers.count==b.deformers.count&&a.deformers.capacity==b.deformers.capacity&&
    a.materials.data==b.materials.data&&a.materials.count==b.materials.count&&a.materials.capacity==b.materials.capacity&&a.material_slots==b.material_slots;}
HsmpViewVertexState vertex_state(const VertexSnapshot& snapshot){
    require(snapshot.header.count>=0&&snapshot.header.count<=16,"native vertex state bounds");
    const auto end=snapshot.overrides.begin()+snapshot.header.count;
    uint32_t null_mask{};for(int32_t i=0;i<snapshot.materials.count;++i)if(!snapshot.material_slots[static_cast<size_t>(i)])null_mask|=uint32_t{1}<<i;
    return {static_cast<uint32_t>(snapshot.header.count),std::all_of(snapshot.overrides.begin(),end,[](uint64_t p){return p==0;})?1u:0u,(snapshot.asset||snapshot.deprecated)?1u:0u,
        static_cast<uint32_t>(snapshot.materials.count),null_mask,snapshot.kind};
}
std::vector<Obj> vertex_material_objects(const VertexSnapshot&);
void vertex_materials_pure(const VertexSnapshot&,const std::vector<Obj>&);
HsmpViewVertexState vertex_observe(Obj world,Obj owner,Obj component,HsmpViewResult* r){
    VertexOperation operation(owner,component,true);qualify(world,owner,component,r);const auto p=vertex_layout(component,vertex_kind);
    vertex_material_layout(component);
    const auto original=vertex_asset;const auto first=vertex_copy(vertex_live(component),p.offset);const auto materials=vertex_material_objects(first);
    qualify(world,owner,component,r);if(original.weak)vertex_live(original);else vertex_empty_profile(component,vertex_kind);
    if(vertex_kind==0)require(!object_property(component,L"SkeletalMesh").weak&&!object_property(component,L"SkinnedAsset").weak,"native empty skeletal asset changed during census");
    else require(same(object_property(component,L"StaticMesh"),original),"native vertex asset changed during census");
    const auto second=vertex_copy(vertex_live(component),p.offset);
    require(vertex_equal(first,second),"native vertex original LODData changed during census");
    if(original.weak){vertex_live(original);require((*vertex_flags(vertex_live(original))&0x40u)==0,"native vertex runtime asset changed");}
    vertex_live(component);check_guard();vertex_pure(owner);
    if(original.weak){void* asset_pointer=vertex_pure(original);require((*vertex_flags(asset_pointer)&0x40u)==0,"native vertex runtime asset changed before final copy");}
    else require(vertex_empty_vtable_read(vertex_pure(component))==(vertex_kind?vertex_empty_image+0x766fc60:vertex_skeletal_image+0x7659cb0),"native empty original class changed before final copy");
    const auto final=vertex_copy(vertex_pure(component),p.offset);
    require(final.asset==original.address&&vertex_equal(second,final),"native vertex LODData/asset changed after native getters");vertex_materials_pure(final,materials);return vertex_state(final);
}
void vertex_native_asset(Obj world,Obj owner,Obj component,const HsmpViewComponent& recipe,HsmpViewResult* r){
    if(recipe.kind==8||recipe.kind==9){require(recipe.vertex_state==4&&recipe.vertex_count==0,"native empty mesh profile mismatch");
        const auto proof=vertex_observe(world,owner,component,r);require(proof.component_kind==(recipe.kind==9?0u:1u)&&proof.asset_present==0&&proof.no_override==1,"native empty mesh asset/override appeared");
        if(recipe.kind==9){require(proof.material_count==recipe.material_count,"native empty skeletal material count changed");uint32_t mask{};
            for(uint32_t i=0;i<recipe.material_count;++i){require(recipe.materials[i].slot==i,"native empty material slots incomplete");if(text(recipe.materials[i].base).empty())mask|=uint32_t{1}<<i;}
            require(proof.material_null_mask==mask,"native empty skeletal null material changed");}return;}
    if(recipe.vertex_state!=0)return;
    require(recipe.kind==1&&recipe.vertex_count==0,"native asset vertex proof supports cooked static mesh only");
    const auto proof=vertex_observe(world,owner,component,r);require(proof.asset_present==1&&proof.no_override==1,"native asset vertex override appeared");
}
VertexSnapshot vertex_target_pure(const HsmpViewFinishTarget& target){
    vertex_pure(target.owner);if(target.scene_kind==8||target.scene_kind==9){require(!target.asset.weak&&!target.asset.address,"native empty aggregate unexpected asset");
        require(vertex_empty_vtable_read(vertex_pure(target.component))==(target.scene_kind==9?vertex_skeletal_image+0x7659cb0:vertex_empty_image+0x766fc60),"native empty aggregate original mesh changed");}
    else{void* asset_pointer=vertex_pure(target.asset);require((*vertex_flags(asset_pointer)&0x40u)==0,"native vertex aggregate runtime asset");}
    const auto copy=vertex_copy(vertex_pure(target.component),target.scene_kind==9?0x760:0x588);
    require(copy.asset==target.asset.address&&vertex_state(copy).no_override==1,"native vertex aggregate asset/override changed");return copy;
}
void vertex_skeletal_mirror_state(const VertexSnapshot& snapshot){
    require(!snapshot.kind,"native empty skeletal mirror class changed");
    require(std::all_of(snapshot.animation.begin(),snapshot.animation.end(),[](uint64_t p){return p==0;}),"native empty skeletal mirror animation class/instance active");
    require(!vt->resolve(snapshot.leader),"native empty skeletal mirror leader present");
    require(!(snapshot.cloth&2u),"native empty skeletal mirror cloth actors allowed");
    require(snapshot.suspended==8,"native empty skeletal mirror cloth not suspended");
    require(snapshot.postprocess&1u,"native empty skeletal mirror postprocess enabled");
    require(!snapshot.clothing_interactor,"native empty skeletal mirror clothing interactor present");
}
struct VertexMaterialBinding {Obj world{},owner{},component{};VertexSnapshot snapshot{};std::vector<Obj> materials;uint32_t owned{};};
std::map<uint64_t,VertexMaterialBinding> vertex_source_materials;
Obj vertex_material_world{};
std::vector<Obj> vertex_material_objects(const VertexSnapshot& snapshot){
    std::vector<Obj> out;out.reserve(static_cast<size_t>(snapshot.materials.count));for(int32_t i=0;i<snapshot.materials.count;++i){
        const auto raw=snapshot.material_slots[static_cast<size_t>(i)];if(raw){const auto object=keep(reinterpret_cast<void*>(raw));vertex_live(object);require(is(object,L"/Script/Engine.MaterialInterface"),"native original material class invalid");out.push_back(object);}else out.push_back({});}return out;
}
void vertex_materials_pure(const VertexSnapshot& snapshot,const std::vector<Obj>& originals){
    require(originals.size()==static_cast<size_t>(snapshot.materials.count),"native original material census count changed");
    for(size_t i=0;i<originals.size();++i){require(snapshot.material_slots[i]==originals[i].address,"native original material slot replaced");if(originals[i].weak)vertex_pure(originals[i]);else require(!originals[i].address,"native null material identity malformed");}
}
void vertex_bind_source_materials(Obj world,Obj owner,Obj component,uint32_t owned=0,const std::vector<Obj>* expected=nullptr){
    require(owned<=1,"native material binding ownership");
    if(!same(vertex_material_world,world)){vertex_source_materials.clear();vertex_material_world=world;}
    const auto first=vertex_copy(vertex_live(component),0x760);auto objects=vertex_material_objects(first);vertex_live(world);vertex_live(owner);check_guard();
    vertex_pure(world);vertex_pure(owner);const auto final=vertex_copy(vertex_pure(component),0x760);require(vertex_equal(first,final),"native source original materials changed before capture");vertex_materials_pure(final,objects);
    if(owned){require(expected!=nullptr,"native mirror original material identities missing");vertex_materials_pure(final,*expected);vertex_skeletal_mirror_state(final);}
    require(vertex_source_materials.contains(component.weak)||vertex_source_materials.size()<32*64,"native source material snapshot bound");
    vertex_source_materials[component.weak]={world,owner,component,final,std::move(objects),owned};
}
void vertex_source_materials_final(Obj world,const HsmpViewFinishTarget& target,const VertexSnapshot& current){
    const auto found=vertex_source_materials.find(target.component.weak);require(found!=vertex_source_materials.end(),"native source material frame snapshot missing");const auto& original=found->second;
    require(same(original.world,world)&&same(original.owner,target.owner)&&same(original.component,target.component),"native source material original scope changed");
    require(original.owned==target.owned_mirror,"native material original ownership changed");
    vertex_pure(original.world);vertex_pure(original.owner);vertex_pure(original.component);
    require(vertex_equal(original.snapshot,current),"native source material state changed after component capture");vertex_materials_pure(current,original.materials);
    if(original.owned)vertex_skeletal_mirror_state(current);
}
