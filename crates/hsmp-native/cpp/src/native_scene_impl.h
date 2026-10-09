// Exact shipping Camera/SpringArm profiles. No pointer or native FName is sent
// over the network; only the authoritative socket output crosses the boundary.
uintptr_t scene_image{};
bool scene_shipping_profile(){
    const auto* image=reinterpret_cast<const uint8_t*>(GetModuleHandleW(nullptr));if(!image)return false;
    const auto* dos=reinterpret_cast<const IMAGE_DOS_HEADER*>(image);if(dos->e_magic!=IMAGE_DOS_SIGNATURE||dos->e_lfanew<0||dos->e_lfanew>65536)return false;
    const auto* pe=reinterpret_cast<const IMAGE_NT_HEADERS64*>(image+dos->e_lfanew);
    if(pe->Signature!=IMAGE_NT_SIGNATURE||pe->FileHeader.Machine!=IMAGE_FILE_MACHINE_AMD64||pe->OptionalHeader.Magic!=IMAGE_NT_OPTIONAL_HDR64_MAGIC||pe->OptionalHeader.SizeOfImage<0x8d70ce8)return false;
    const auto match=[&](size_t at,std::initializer_list<uint8_t> bytes){return std::equal(bytes.begin(),bytes.end(),image+at);};
    if(!match(0x3afd800,{0xe9,0x4b,0x62,0x0f,0})||
       !match(0x3b51630,{0x80,0x79,0x3b,0,0x0f,0x95,0xc0,0xc3})||
       !match(0x3cba5e2,{0x44,0x0f,0x10,0x81,0x10,0x03,0,0})||
       !match(0x3cba5ed,{0x0f,0x10,0xa1,0x20,0x03,0,0})||
       !match(0x3cba5f7,{0xf2,0x0f,0x10,0x91,0,0x03,0,0})||
       !match(0x3cba625,{0x66,0x0f,0x10,0x89,0xf0,0x02,0,0})||
       !match(0x3cbc3bd,{0xe8,0xae,0xaf,0xf3,0xff})||
       !match(0x3bf7370,{0x40,0x57,0x41,0x56,0x48,0x83,0xec,0x28,0x83,0xb9,0xd0,0,0,0,0})||
       !match(0x3cbb6d6,{0x48,0x8b,0x05,0x03,0x56,0x0b,0x05,0x48,0x89,0x02,0xc6,0x42,0x08,0x02}))return false;
    const auto base=reinterpret_cast<uintptr_t>(image);uint64_t tick{};
    for(const auto table:{size_t{0x7412680},size_t{0x76085c0},size_t{0x76952b8}}){
        std::memcpy(&tick,image+table+0x3e8,8);if(tick!=base+0x3b51630)return false;}
    const std::array<double,2> unit_xy{1,1},unit_z{1,0};
    if(std::memcmp(image+0x6cb3120,unit_xy.data(),16)||std::memcmp(image+0x6cadc90,unit_z.data(),16))return false;
    scene_image=base;return true;
}
bool (*scene_build_admit)()=scene_shipping_profile;
uint64_t scene_vtable(const void* object){uint64_t out{};std::memcpy(&out,object,8);return out;}
uint64_t (*scene_vtable_read)(const void*)=scene_vtable;
uint64_t scene_socket_bits(){uint64_t out{};require(scene_image!=0,"native scene image unavailable");std::memcpy(&out,reinterpret_cast<const void*>(scene_image+0x8d70ce0),8);return out;}
uint64_t (*scene_socket_read)()=scene_socket_bits;
uint32_t scene_kind(Obj component){
    const auto cls=keep(vt->class_of(vertex_live(component)));
    if(same(cls,find(L"/Script/Engine.CameraComponent")))return 6;
    if(same(cls,find(L"/Script/Engine.SpringArmComponent")))return 7;
    return 0;
}
void scene_profile(Obj component,uint32_t kind){
    require((kind==6||kind==7)&&scene_build_admit&&scene_build_admit(),"native scene shipping profile unsupported");
    const auto cls=keep(vt->class_of(vertex_live(component)));
    require(same(cls,find(kind==6?L"/Script/Engine.CameraComponent":L"/Script/Engine.SpringArmComponent")),"native scene exact class mismatch");
    std::array<HsmpProp,64> props{};int32_t bytes{};const auto count=vt->props(vertex_live(cls),props.data(),64,&bytes);
    require(count>=0&&count<=64&&bytes==(kind==6?0x9e0:0x330),"native scene reflected class size");vertex_live(cls);
    require(scene_vtable_read(vertex_live(component))==scene_image+(kind==6?0x76085c0:0x76952b8),"native scene original shipping vtable");
    if(kind==7){const auto flag=property(component,L"bDrawDebugLagMarkers",L"BoolProperty",1);
        require(flag.offset==0x271&&flag.bool_offset==0&&flag.bool_mask==1,"native spring arm debug ABI");
        require(!bool_property(component,L"bDrawDebugLagMarkers"),"native spring arm debug rendering unsupported");}
}
thread_local Obj scene_owner{},scene_component{};thread_local uint32_t active_scene_kind{};
struct SceneOperation{
    Obj previous_owner{scene_owner},previous_component{scene_component};uint32_t previous_kind{active_scene_kind};
    SceneOperation(Obj owner,Obj component,uint32_t kind){if(!owner.weak)return;vertex_live(owner);scene_profile(component,kind);scene_owner=owner;scene_component=component;active_scene_kind=kind;}
    ~SceneOperation(){scene_owner=previous_owner;scene_component=previous_component;active_scene_kind=previous_kind;}
};
void scene_call_guard(Obj object,Obj function,Obj cls){
    if(!scene_owner.weak)return;vertex_live(scene_owner);scene_profile(scene_component,active_scene_kind);
    vertex_live(object);vertex_live(function);vertex_live(cls);
}
void scene_pure_profile(Obj component,uint32_t kind){
    void* p=vertex_pure(component);require(scene_vtable_read(p)==scene_image+(kind==6?0x76085c0:0x76952b8),"native scene final original vtable");
    if(kind==7){uint8_t flag{};std::memcpy(&flag,static_cast<const uint8_t*>(p)+0x271,1);require(!(flag&1),"native spring arm debug changed");}
}
void scene_dispatch_guard(Obj object,Obj function,Obj cls){
    if(!scene_owner.weak)return;vertex_pure(scene_owner);scene_pure_profile(scene_component,active_scene_kind);
    vertex_pure(object);vertex_pure(function);vertex_pure(cls);
}
void arm_valid(const HsmpViewSpringArmFrame& a){for(double x:a.translation)require(std::isfinite(x),"native spring arm translation invalid");for(double x:a.rotation)require(std::isfinite(x),"native spring arm rotation invalid");}
HsmpViewSpringArmFrame arm_copy(const void* p){
    HsmpViewSpringArmFrame a{};std::memcpy(a.translation,static_cast<const uint8_t*>(p)+0x2f0,24);std::memcpy(a.rotation,static_cast<const uint8_t*>(p)+0x310,32);arm_valid(a);return a;
}
bool arm_equal(const HsmpViewSpringArmFrame& a,const HsmpViewSpringArmFrame& b){return std::memcmp(&a,&b,sizeof(a))==0;}
Transform arm_transform(const HsmpViewSpringArmFrame& a){Transform t{};std::copy_n(a.translation,3,t.p);std::copy_n(a.rotation,4,t.q);std::fill_n(t.scale,3,1.0);return t;}
Transform arm_getter(Obj component,HsmpViewText socket,HsmpViewResult* r){
    Function f(L"/Script/Engine.SceneComponent:GetSocketTransform");f.put(L"InSocketName",L"NameProperty",name(socket));f.enumeration(L"TransformSpace",2);f.call(component,r);
    return wire(f.value<EngineTransform>(L"ReturnValue",L"StructProperty",L"Transform"));
}
void arm_socket(HsmpViewText socket){const auto s=text(socket);require(!s.empty()&&s.size()<=128&&s!=L"None"&&name(socket)==scene_socket_read(),"native spring arm original singleton socket changed");}
struct ArmOwnedChild {Obj component{},parent{};uint64_t socket{};};
struct ArmChildSnapshot {Array children{};std::vector<uint64_t> slots;};
struct ArmPublication {Obj world{},owner{},component{},level{};int32_t world_offset{};uint64_t socket{};HsmpViewSpringArmFrame arm{};std::vector<ArmOwnedChild> owned;std::vector<ArmChildSnapshot> snapshots;};
bool arm_array_equal(const Array& a,const Array& b){return a.data==b.data&&a.count==b.count&&a.capacity==b.capacity;}
ArmChildSnapshot arm_children_copy(Obj owner,const ArmOwnedChild& target,const std::vector<ArmOwnedChild>& owned,const ArmChildSnapshot* original=nullptr){
    const auto* p=static_cast<const uint8_t*>(vertex_pure(target.component));uint64_t actual_owner{},parent{},socket{};
    std::memcpy(&actual_owner,p+0x90,8);std::memcpy(&parent,p+0xb0,8);std::memcpy(&socket,p+0xb8,8);
    require(actual_owner==owner.address&&parent==target.parent.address&&socket==target.socket,"native spring original child owner/parent/socket changed");
    ArmChildSnapshot out{};std::memcpy(&out.children,p+0xc8,sizeof(Array));
    if(original)require(arm_array_equal(out.children,original->children),"native spring child allocation changed before slot read");
    require(out.children.count>=0&&static_cast<size_t>(out.children.count)<=owned.size()&&out.children.capacity>=out.children.count&&
        (!out.children.capacity||out.children.data),"native spring child array malformed");
    const auto bytes=static_cast<size_t>(out.children.count)*8;const auto data=reinterpret_cast<uintptr_t>(out.children.data);
    require(!bytes||(data&&data%alignof(void*)==0&&data<=UINTPTR_MAX-bytes),"native spring child array storage invalid");
    out.slots.resize(static_cast<size_t>(out.children.count));if(bytes)std::memcpy(out.slots.data(),out.children.data,bytes);
    size_t expected{};for(const auto& child:owned)if(same(child.parent,target.component))++expected;
    require(expected==out.slots.size(),"native spring owned child census changed");
    for(size_t i=0;i<out.slots.size();++i){const auto slot=out.slots[i];require(slot!=0&&std::find(out.slots.begin(),out.slots.begin()+static_cast<ptrdiff_t>(i),slot)==out.slots.begin()+static_cast<ptrdiff_t>(i),"native spring duplicate/null child");
        const auto found=std::find_if(owned.begin(),owned.end(),[&](const ArmOwnedChild& child){return child.component.address==slot&&same(child.parent,target.component);});
        require(found!=owned.end(),"native spring foreign child unsupported");vertex_pure(found->component);}
    return out;
}
void arm_children_final(Obj world,Obj owner,Obj level,int32_t world_offset,const std::vector<ArmOwnedChild>& owned,const std::vector<ArmChildSnapshot>& snapshots){
    vertex_pure(world);const auto* owner_pointer=vertex_pure(owner);require(source_outer&&source_outer(owner_pointer)&&*source_outer(owner_pointer)==reinterpret_cast<void*>(level.address),"native spring original owner level changed");
    require(world_offset==0xc0,"native spring original level world layout");void* actual_world{};std::memcpy(&actual_world,static_cast<const uint8_t*>(vertex_pure(level))+world_offset,8);
    require(actual_world==reinterpret_cast<void*>(world.address),"native spring original child world changed");
    require(owned.size()==snapshots.size(),"native spring original child snapshot missing");
    for(size_t i=0;i<owned.size();++i){const auto actual=arm_children_copy(owner,owned[i],owned,&snapshots[i]);
        require(arm_array_equal(actual.children,snapshots[i].children)&&actual.slots==snapshots[i].slots,"native spring child allocation/slots changed");}
}
void arm_notify_native(Obj component){reinterpret_cast<void(*)(void*,uint32_t,uint8_t)>(scene_image+0x3bf7370)(vertex_pure(component),0,0);}
void (*arm_notify_children)(Obj)=arm_notify_native;
void arm_publication_final(const ArmPublication& original){
    arm_children_final(original.world,original.owner,original.level,original.world_offset,original.owned,original.snapshots);scene_pure_profile(original.component,7);
    require(scene_socket_read()==original.socket&&arm_equal(arm_copy(vertex_pure(original.component)),original.arm),"native spring final published output changed");
}
ArmPublication arm_apply(Obj world,Obj owner,Obj component,HsmpViewText socket,const HsmpViewSpringArmFrame& value,const std::vector<ArmOwnedChild>& owned,HsmpViewResult* r){
    require(!owned.empty()&&owned.size()<=64*2,"native spring owned component bound");arm_valid(value);scene_profile(component,7);arm_socket(socket);
    require(std::any_of(owned.begin(),owned.end(),[&](const ArmOwnedChild& target){return same(target.component,component);}),"native spring original component not owned");
    const auto scene_class=find(L"/Script/Engine.SceneComponent");
    require(spline_api.children&&spline_api.next&&spline_api.inner&&spline_api.size&&spline_api.offset&&spline_api.field_name&&spline_api.field_class&&spline_api.variant_name,"native spring child metadata unavailable");
    vertex_object_array_layout(scene_class,L"AttachChildren",0xc8);
    const auto level=returned(owner,L"/Script/Engine.Actor:GetLevel",r);const auto world_field=property(level,L"OwningWorld",L"ObjectProperty",8);
    require(world_field.offset==0xc0,"native spring reflected level world layout");
    const auto expected_socket=name(socket);
    for(size_t i=0;i<owned.size();++i){require(owned[i].component.weak&&std::none_of(owned.begin(),owned.begin()+static_cast<ptrdiff_t>(i),[&](const ArmOwnedChild& prior){return prior.component.address==owned[i].component.address;}),"native spring owned component duplicate");
        qualify(world,owner,owned[i].component,r);require(property(owned[i].component,L"AttachParent",L"ObjectProperty",8).offset==0xb0&&property(owned[i].component,L"AttachSocketName",L"NameProperty",8).offset==0xb8&&property(owned[i].component,L"AttachChildren",L"ArrayProperty",16).offset==0xc8,"native spring child reflected layout");}
    qualify(world,owner,component,r);check_guard();vertex_pure(owner);scene_pure_profile(component,7);
    std::vector<ArmChildSnapshot> snapshots;snapshots.reserve(owned.size());for(const auto& child:owned)snapshots.push_back(arm_children_copy(owner,child,owned));
    arm_children_final(world,owner,level,world_field.offset,owned,snapshots);
    require(scene_socket_read()==expected_socket,"native spring singleton changed before publication");
    auto* p=static_cast<uint8_t*>(vertex_pure(component));std::memcpy(p+0x2f0,value.translation,24);std::memcpy(p+0x310,value.rotation,32);
    // Exact shipping UpdateDesiredArmLocation publishes this cache through
    // UpdateChildTransforms(this,0,0). It can update the complete owned subtree.
    require(arm_notify_children!=nullptr,"native spring child publication unavailable");arm_notify_children(component);
    check_guard();for(const auto& child:owned)qualify(world,owner,child.component,r);qualify(world,owner,component,r);check_guard();
    arm_children_final(world,owner,level,world_field.offset,owned,snapshots);scene_pure_profile(component,7);
    require(scene_socket_read()==expected_socket&&arm_equal(arm_copy(vertex_pure(component)),value),"native spring output changed during child publication");if(r)++r->operations;
    require(close(arm_transform(value),arm_getter(component,socket,r)),"mirror native spring socket readback");
    check_guard();arm_children_final(world,owner,level,world_field.offset,owned,snapshots);scene_pure_profile(component,7);
    require(scene_socket_read()==expected_socket&&arm_equal(arm_copy(vertex_pure(component)),value),"native spring output changed after publication readback");
    return {world,owner,component,level,world_field.offset,expected_socket,value,owned,std::move(snapshots)};
}
HsmpViewSpringArmFrame arm_observe(Obj world,Obj owner,Obj component,HsmpViewText socket,HsmpViewResult* r){
    SceneOperation operation(owner,component,7);qualify(world,owner,component,r);arm_socket(socket);
    const auto first=arm_copy(vertex_live(component));const auto native=arm_getter(component,socket,r);
    require(close(arm_transform(first),native),"native spring arm getter/cache mismatch");qualify(world,owner,component,r);scene_profile(component,7);arm_socket(socket);
    const auto second=arm_copy(vertex_live(component));require(arm_equal(first,second),"native spring arm cache changed during capture");
    const auto expected_name=name(socket);check_guard();vertex_pure(owner);scene_pure_profile(component,7);
    require(scene_socket_read()==expected_name,"native spring arm singleton changed after getters");const auto final=arm_copy(vertex_pure(component));
    require(arm_equal(second,final),"native spring arm cache changed after getters");return final;
}
void scene_inert(Obj component,HsmpViewResult* r){
    Function deactivate(L"/Script/Engine.ActorComponent:Deactivate");deactivate.call(component,r);
    Function tick(L"/Script/Engine.ActorComponent:SetComponentTickEnabled");tick.boolean(L"bEnabled",false);tick.call(component,r);
    for(const auto path:{L"/Script/Engine.ActorComponent:IsActive",L"/Script/Engine.ActorComponent:IsComponentTickEnabled"}){
        Function f(path);f.call(component,r);const auto p=f.field(L"ReturnValue",L"BoolProperty",1);
        require((f.buf[static_cast<size_t>(p.offset+p.bool_offset)]&p.bool_mask)==0,"native mirror camera/arm remains active");}
}
struct SceneSnapshot {VertexSnapshot vertex{};HsmpViewSpringArmFrame arm{};uint64_t socket{};HsmpProp active{},tick{};};
void scene_pure_inert(const void* object,const SceneSnapshot& snapshot){
    const auto* p=static_cast<const uint8_t*>(object);uint8_t active{},tick_enabled{};
    require(snapshot.active.offset==0x8a&&snapshot.active.bool_offset==0&&snapshot.active.bool_mask==8&&snapshot.tick.offset==0x30&&snapshot.tick.size==0x30,"native mirror tick/active layout");
    std::memcpy(&active,p+snapshot.active.offset,1);std::memcpy(&tick_enabled,p+0x3b,1);
    // The matched native IsComponentTickEnabled body returns byte[0x3b]!=0.
    // A later metadata/guard callback must never make an enabled snapshot valid.
    require(!(active&snapshot.active.bool_mask)&&tick_enabled==0,"native aggregate mirror activation/tick changed");
}
SceneSnapshot scene_target_prepare(Obj world,const HsmpViewFinishTarget& target,HsmpViewResult* r){
    SceneSnapshot out{};if(target.scene_kind==0||target.scene_kind==8||target.scene_kind==9){VertexOperation operation(target.owner,target.component,target.scene_kind==8||target.scene_kind==9);
        require(target.owned_mirror<=1&&same(vertex_asset,target.asset),"native aggregate original asset replaced");
        if(target.asset.weak)vertex_live(target.asset);else require(target.scene_kind==8||target.scene_kind==9,"native aggregate original asset missing");
        const auto proof=vertex_observe(world,target.owner,target.component,r);
        require(proof.no_override==1&&proof.asset_present==(target.scene_kind==8||target.scene_kind==9?0u:1u),"native aggregate asset/override appeared");
        if((target.scene_kind==8||target.scene_kind==9)&&target.owned_mirror){scene_inert(target.component,r);
            out.active=property(target.component,L"bIsActive",L"BoolProperty",1);out.tick=property(target.component,L"PrimaryComponentTick",L"StructProperty",0x30);
            scene_pure_inert(vertex_pure(target.component),out);}
        out.vertex=vertex_target_pure(target);return out;}
    require((target.scene_kind==6||target.scene_kind==7)&&!target.asset.weak&&target.owned_mirror<=1,"native aggregate scene kind");
    SceneOperation operation(target.owner,target.component,target.scene_kind);qualify(world,target.owner,target.component,r);
    if(target.scene_kind==7){arm_valid(target.arm);out.arm=arm_observe(world,target.owner,target.component,target.socket,r);require(arm_equal(out.arm,target.arm),"native aggregate original arm output changed");out.socket=name(target.socket);}
    else require(target.socket.len==0,"native camera spring socket unexpected");
    if(target.owned_mirror){scene_inert(target.component,r);out.active=property(target.component,L"bIsActive",L"BoolProperty",1);out.tick=property(target.component,L"PrimaryComponentTick",L"StructProperty",0x30);
        scene_pure_inert(vertex_pure(target.component),out);}
    return out;
}
void scene_target_final(const HsmpViewFinishTarget& target,const SceneSnapshot& snapshot,Obj world={}){
    if(target.scene_kind==0||target.scene_kind==8||target.scene_kind==9){const auto final=vertex_target_pure(target);require(vertex_equal(snapshot.vertex,final),"native aggregate vertex census changed");
        if(target.scene_kind==9)vertex_source_materials_final(world,target,final);
        if((target.scene_kind==8||target.scene_kind==9)&&target.owned_mirror)scene_pure_inert(vertex_pure(target.component),snapshot);return;}
    vertex_pure(target.owner);scene_pure_profile(target.component,target.scene_kind);const auto* p=static_cast<const uint8_t*>(vertex_pure(target.component));
    if(target.scene_kind==7)require(scene_socket_read()==snapshot.socket&&arm_equal(arm_copy(p),snapshot.arm),"native aggregate socket/cache changed");
    if(target.owned_mirror)scene_pure_inert(p,snapshot);
}
void finish_scene_set(Obj world,const std::vector<HsmpViewFinishTarget>& targets,HsmpViewResult* r){
    require(targets.size()<=32*64,"native scene aggregate bound");std::vector<SceneSnapshot> originals;originals.reserve(targets.size());
    for(const auto& target:targets)originals.push_back(scene_target_prepare(world,target,r));
    vertex_live(world);if(active_game_instance.weak)vertex_live(active_game_instance);check_guard();
    vertex_pure(world);if(active_game_instance.weak)vertex_pure(active_game_instance);
    for(size_t i=0;i<targets.size();++i)scene_target_final(targets[i],originals[i],world);
}
