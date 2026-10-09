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
void* vertex_live(Obj object) {
    require(vertex_flags!=nullptr,"native vertex flags unavailable");void* p=get(object);
    const auto* flags=vertex_flags(p);require(flags&&(*flags&0x40000000u)==0,"native vertex original object garbage");
    const auto cls=keep(vt->class_of(p));void* class_pointer=get(cls);flags=vertex_flags(class_pointer);
    require(flags&&(*flags&0x40000000u)==0,"native vertex original class garbage");get(cls);
    p=get(object);flags=vertex_flags(p);require(flags&&(*flags&0x40000000u)==0,"native vertex original object changed");return p;
}
thread_local Obj vertex_owner{},vertex_component{},vertex_asset{};
struct VertexOperation {
    Obj owner{},component{},mesh{};
    VertexOperation(Obj next_owner,Obj next_component):owner(vertex_owner),component(vertex_component),mesh(vertex_asset){
        if(!next_owner.weak)return;
        vertex_live(next_owner);vertex_live(next_component);
        require(is(next_component,L"/Script/Engine.StaticMeshComponent"),"native asset vertex proof requires static component");
        require(property(next_component,L"StaticMesh",L"ObjectProperty",8).offset==0x560,"native vertex StaticMesh reflected offset");
        auto next_asset=object_property(next_component,L"StaticMesh");vertex_live(next_asset);
        require(is(next_asset,L"/Script/Engine.StaticMesh"),"native vertex static asset class");
        const auto* flags=vertex_flags(vertex_live(next_asset));require(flags&&(*flags&0x40u)==0,"native vertex runtime asset unsupported");
        vertex_owner=next_owner;vertex_component=next_component;vertex_asset=next_asset;
    }
    ~VertexOperation(){vertex_owner=owner;vertex_component=component;vertex_asset=mesh;}
};
void vertex_call_guard(Obj object,Obj function,Obj cls){
    if(!vertex_owner.weak)return;
    vertex_live(vertex_owner);vertex_live(vertex_component);vertex_live(vertex_asset);
    require((*vertex_flags(vertex_live(vertex_asset))&0x40u)==0,"native vertex runtime asset changed");
    require(same(object_property(vertex_component,L"StaticMesh"),vertex_asset),"native vertex original asset link changed");
    vertex_live(object);vertex_live(function);vertex_live(cls);
}
HsmpProp vertex_layout(Obj component) {
    require(vertex_build_admit&&vertex_build_admit(),"native vertex shipping layout unsupported");
    require(spline_api.children&&spline_api.next&&spline_api.inner&&spline_api.structure&&spline_api.size&&spline_api.offset&&
        spline_api.field_name&&spline_api.field_class&&spline_api.variant_name,"native vertex reflected metadata unavailable");
    const auto cls=find(L"/Script/Engine.StaticMeshComponent"),inner_struct=find(L"/Script/Engine.StaticMeshComponentLODInfo");
    vertex_live(cls);vertex_live(inner_struct);std::array<HsmpProp,1> fields{};int32_t size{};
    require(vt->props(vertex_live(inner_struct),fields.data(),1,&size)==0&&size==0x90,"native vertex LOD info reflected size");vertex_live(inner_struct);
    const auto property_info=property(component,L"LODData",L"ArrayProperty",16);
    require(property_info.offset==0x588,"native vertex LODData reflected offset");
    void* field=*spline_api.children(vertex_live(cls));bool found{};
    for(uint32_t n=0;field;++n){require(n<4096,"native vertex property chain bound");vertex_live(cls);
        SplineName key{};spline_api.field_name(field,&key);
        if(spline_name(key)==name(L"LODData")){
            SplineVariant variant{};SplineName type{};spline_api.field_class(field,&variant);spline_api.variant_name(&variant,&type);
            require(spline_name(type)==name(L"ArrayProperty")&&*spline_api.size(field)==16&&*spline_api.offset(field)==0x588,"native vertex reflected array ABI");
            void* inner=*spline_api.inner(field);require(inner!=nullptr,"native vertex array inner missing");
            spline_api.field_class(inner,&variant);spline_api.variant_name(&variant,&type);
            require(spline_name(type)==name(L"StructProperty")&&*spline_api.size(inner)==0x90&&*spline_api.offset(inner)==0&&
                *spline_api.structure(inner)==vertex_live(inner_struct),"native vertex reflected inner ABI");found=true;break;
        }
        vertex_live(cls);field=spline_api.next(field);
    }
    require(found,"native vertex LODData metadata missing");vertex_live(cls);vertex_live(inner_struct);return property_info;
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
    vertex_pure(vertex_owner);void* asset_pointer=vertex_pure(vertex_asset);
    require((*vertex_flags(asset_pointer)&0x40u)==0,"native vertex runtime asset before dispatch");
    void* component_pointer=vertex_pure(vertex_component);uint64_t hard_asset{};
    std::memcpy(&hard_asset,static_cast<const uint8_t*>(component_pointer)+0x560,8);
    require(hard_asset==vertex_asset.address,"native vertex original asset before dispatch");
    vertex_pure(object);vertex_pure(function);vertex_pure(cls);
}
struct VertexSnapshot {uint64_t asset{};Array header{};std::array<uint64_t,16> overrides{};};
VertexSnapshot vertex_copy(const void* component,int32_t offset){
    require(component&&offset==0x588,"native vertex copied component layout");VertexSnapshot copy{};
    std::memcpy(&copy.asset,static_cast<const uint8_t*>(component)+0x560,8);
    std::memcpy(&copy.header,static_cast<const uint8_t*>(component)+offset,sizeof(Array));
    const auto& h=copy.header;
    require(h.count>=0&&h.count<=16&&h.capacity>=h.count&&h.capacity>=0&&(!h.capacity||h.data),"native vertex complete LODData bounds");
    for(int32_t i=0;i<h.count;++i)std::memcpy(&copy.overrides[static_cast<size_t>(i)],static_cast<const uint8_t*>(h.data)+static_cast<size_t>(i)*0x90+0x30,8);
    return copy;
}
bool vertex_equal(const VertexSnapshot& a,const VertexSnapshot& b){return a.asset==b.asset&&a.header.data==b.header.data&&a.header.count==b.header.count&&a.header.capacity==b.header.capacity&&a.overrides==b.overrides;}
HsmpViewVertexState vertex_state(const VertexSnapshot& snapshot){
    require(snapshot.header.count>=0&&snapshot.header.count<=16,"native vertex state bounds");
    const auto end=snapshot.overrides.begin()+snapshot.header.count;
    return {static_cast<uint32_t>(snapshot.header.count),std::all_of(snapshot.overrides.begin(),end,[](uint64_t p){return p==0;})?1u:0u};
}
HsmpViewVertexState vertex_observe(Obj world,Obj owner,Obj component,HsmpViewResult* r){
    VertexOperation operation(owner,component);qualify(world,owner,component,r);const auto p=vertex_layout(component);
    const auto original=vertex_asset;const auto first=vertex_copy(vertex_live(component),p.offset);
    qualify(world,owner,component,r);vertex_live(original);
    require(same(object_property(component,L"StaticMesh"),original),"native vertex asset changed during census");
    const auto second=vertex_copy(vertex_live(component),p.offset);
    require(vertex_equal(first,second),"native vertex original LODData changed during census");
    vertex_live(original);require((*vertex_flags(vertex_live(original))&0x40u)==0,"native vertex runtime asset changed");
    vertex_live(component);check_guard();vertex_pure(owner);void* asset_pointer=vertex_pure(original);
    require((*vertex_flags(asset_pointer)&0x40u)==0,"native vertex runtime asset changed before final copy");
    const auto final=vertex_copy(vertex_pure(component),p.offset);
    require(final.asset==original.address&&vertex_equal(second,final),"native vertex LODData/asset changed after native getters");return vertex_state(final);
}
void vertex_native_asset(Obj world,Obj owner,Obj component,const HsmpViewComponent& recipe,HsmpViewResult* r){
    if(recipe.vertex_state!=0)return;
    require(recipe.kind==1&&recipe.vertex_count==0,"native asset vertex proof supports cooked static mesh only");
    require(vertex_observe(world,owner,component,r).no_override==1,"native asset vertex override appeared");
}
VertexSnapshot vertex_target_pure(const HsmpViewFinishTarget& target){
    vertex_pure(target.owner);void* asset_pointer=vertex_pure(target.asset);
    require((*vertex_flags(asset_pointer)&0x40u)==0,"native vertex aggregate runtime asset");
    const auto copy=vertex_copy(vertex_pure(target.component),0x588);
    require(copy.asset==target.asset.address&&vertex_state(copy).no_override==1,"native vertex aggregate asset/override changed");return copy;
}
