// Matched shipping pose transfer. The hidden Poseable is a calculator only;
// the visible Skeletal has no LeaderPoseComponent. Both arrays remain engine-owned.
uintptr_t pose_image{};
bool pose_shipping_build(){
    const auto* image=reinterpret_cast<const uint8_t*>(GetModuleHandleW(nullptr));if(!image)return false;
    const auto* dos=reinterpret_cast<const IMAGE_DOS_HEADER*>(image);if(dos->e_magic!=IMAGE_DOS_SIGNATURE||dos->e_lfanew<0||dos->e_lfanew>65536)return false;
    const auto* pe=reinterpret_cast<const IMAGE_NT_HEADERS64*>(image+dos->e_lfanew);
    if(pe->Signature!=IMAGE_NT_SIGNATURE||pe->FileHeader.Machine!=IMAGE_FILE_MACHINE_AMD64||pe->OptionalHeader.Magic!=IMAGE_NT_OPTIONAL_HDR64_MAGIC||pe->OptionalHeader.SizeOfImage<0x765a7b0)return false;
    const auto match=[&](size_t at,std::initializer_list<uint8_t> bytes){return std::equal(bytes.begin(),bytes.end(),image+at);};
    if(!match(0x3bc7ea1,{0xc6,0x87,0x18,0x0a,0,0,1})||
       !match(0x3bc7137,{0x49,0x8b,0xb7,0xb8,0x08,0,0})||
       !match(0x3bc7154,{0x49,0x8b,0xbc,0xc7,0xc8,0x05,0,0})||
       !match(0x3bc79e1,{0x41,0x80,0x8f,0x98,0x07,0,0,0x40})||
       !match(0x3bc7c97,{0x48,0x8b,0xcb,0xe8,0x51,0xf4,0xff,0xff})||
       !match(0x3c15516,{0x0f,0xb6,0x81,0x98,0x07,0,0})||
       !match(0x3c1555c,{0x89,0x83,0x10,0x06,0,0,0x44,0x89,0x83,0x0c,0x06,0,0})||
       !match(0x3c195b0,{0x49,0x63,0x8e,0x10,0x06,0,0})||
       !match(0x3b541a5,{0x48,0x8b,0x81,0x90,0,0,0})||
       !match(0x3b541c5,{0x0f,0xb6,0x81,0x8a,0,0,0,0xc0,0xe8,3,0x24,1})||
       !match(0x3c0ff5c,{0x48,0x89,0xbb,0x68,0x05,0,0})||
       !match(0x14dea40,{0x44,0x8b,0x41,4,0x45,0x85,0xc0,0x74,0x49})||
       !match(0x3c02939,{0xe8,0x72,0x1d,1,0})||
       !match(0x3bf7370,{0x40,0x57,0x41,0x56,0x48,0x83,0xec,0x28,0x83,0xb9,0xd0,0,0,0,0})||
       !match(0x3c23f35,{0x0f,0x2f,0xb1,8,6,0,0})||
       !match(0x3b51a10,{0x0f,0xb6,0x81,0x88,0,0,0,0x24,3,0x3c,3,0x75,0x0c,0x80,0x89,0x89,0,0,0,1})||
       !match(0x3b517d0,{0x0f,0xb6,0x81,0x88,0,0,0,0x24,3,0x3c,3,0x75,0x0c,0x80,0x89,0x89,0,0,0,2})||
       !match(0x3b51630,{0x80,0x79,0x3b,0,0x0f,0x95,0xc0,0xc3}))return false;
    const auto base=reinterpret_cast<uintptr_t>(image);uint64_t entry{};
    for(const auto [table,slot,target]:{std::tuple<size_t,size_t,size_t>{0x7646b38,0xab0,0x3bc7c70},{0x7646b38,0xaf0,0x3c146b0},
        {0x7659cb0,0xaf0,0x3c02930},{0x7659cb0,0x560,0x3c23f20},{0x7659cb0,0x4d0,0x3c1cbc0}}){
        std::memcpy(&entry,image+table+slot,8);if(entry!=base+target)return false;}
    pose_image=base;return true;
}
bool (*pose_build_admit)()=pose_shipping_build;
struct PoseBinding {Obj world{},owner{},level{},calculator{},render{},asset{};uint32_t count{};};
struct PoseBuffers {Array arrays[2]{};int32_t editable{},read{};uint8_t flags{};};
bool pose_array_equal(const Array& a,const Array& b){return a.data==b.data&&a.count==b.count&&a.capacity==b.capacity;}
bool pose_null_leader(uint64_t weak){return weak==0||weak==UINT32_MAX;}
Array pose_local(const void* calculator,uint32_t count){Array out{};std::memcpy(&out,static_cast<const uint8_t*>(calculator)+0x8b8,sizeof(Array));
    require(out.count==static_cast<int32_t>(count)&&out.capacity>=out.count&&out.capacity>=0&&out.data&&reinterpret_cast<uintptr_t>(out.data)%16==0,
        "native pose complete calculator local buffer mismatch");return out;}
PoseBuffers pose_buffers(const void* component,uint32_t count){
    PoseBuffers out{};const auto* p=static_cast<const uint8_t*>(component);
    std::memcpy(out.arrays,p+0x5c8,32);std::memcpy(&out.editable,p+0x60c,4);std::memcpy(&out.read,p+0x610,4);out.flags=p[0x798];
    require(out.editable>=0&&out.editable<=1&&out.read>=0&&out.read<=1&&out.editable!=out.read&&(out.flags&0x20),"native pose double buffers unsupported");
    for(const auto& a:out.arrays)require(a.count==static_cast<int32_t>(count)&&a.capacity>=a.count&&a.capacity>=0&&a.data&&
        reinterpret_cast<uintptr_t>(a.data)%16==0,"native pose complete engine buffer mismatch");
    const auto first=reinterpret_cast<uintptr_t>(out.arrays[0].data),second=reinterpret_cast<uintptr_t>(out.arrays[1].data),bytes=count*sizeof(EngineTransform);
    require(first<=UINTPTR_MAX-bytes&&second<=UINTPTR_MAX-bytes&&(first+bytes<=second||second+bytes<=first),"native pose overlapping engine buffers");return out;
}
void pose_profile(Obj component,bool calculator){
    require(pose_build_admit&&pose_build_admit(),"native pose shipping profile unsupported");
    const auto cls=keep(vt->class_of(vertex_live(component)));require(same(cls,find(calculator?L"/Script/Engine.PoseableMeshComponent":L"/Script/Engine.SkeletalMeshComponent")),"native pose exact class required");
    std::array<HsmpProp,256> props{};int32_t bytes{};const auto count=vt->props(vertex_live(cls),props.data(),256,&bytes);
    require(count>=0&&count<=256&&bytes==(calculator?0xa20:0xf70),"native pose reflected class size");
    require(property(component,L"SkeletalMesh",L"ObjectProperty",8).offset==0x558&&property(component,L"SkinnedAsset",L"ObjectProperty",8).offset==0x560&&
        property(component,L"LeaderPoseComponent",L"WeakObjectProperty",8).offset==0x568,"native pose asset/leader layout");
    require(property(component,L"PrimaryComponentTick",L"StructProperty",0x30).offset==0x30,"native pose tick layout");
    if(!calculator)vertex_empty_profile(component,0); // layout/code admission does not require an empty asset
    require(scene_vtable_read(vertex_live(component))==pose_image+(calculator?0x7646b38:0x7659cb0),"native pose original vtable");
}
void pose_pure(const PoseBinding& b){
    vertex_pure(b.world);const auto* owner=vertex_pure(b.owner);const auto* level=vertex_pure(b.level);const auto* asset_pointer=vertex_pure(b.asset);
    require(source_outer&&source_outer(owner)&&*source_outer(owner)==level,"native pose original owner level changed");
    void* owning_world{};std::memcpy(&owning_world,static_cast<const uint8_t*>(level)+0xc0,8);require(owning_world==reinterpret_cast<void*>(b.world.address),"native pose original world changed");
    require((*vertex_flags(asset_pointer)&0x40u)==0,"native pose runtime asset unsupported");
    for(const auto [object,calculator]:{std::pair<Obj,bool>{b.calculator,true},{b.render,false}}){
        const auto* p=static_cast<const uint8_t*>(vertex_pure(object));uint64_t mesh{},asset_value{},leader{},owner_value{};
        std::memcpy(&mesh,p+0x558,8);std::memcpy(&asset_value,p+0x560,8);std::memcpy(&leader,p+0x568,8);std::memcpy(&owner_value,p+0x90,8);
        require(scene_vtable_read(p)==pose_image+(calculator?0x7646b38:0x7659cb0)&&mesh==b.asset.address&&asset_value==b.asset.address&&
            pose_null_leader(leader)&&owner_value==b.owner.address,"native pose original asset/owner/leader changed");
        require(p[0x3b]==0&&!(p[0x8a]&8),"native pose mirror tick/activation enabled");
        if(!calculator){uint64_t anim{},post{},anim_class{},generated{},interactor{},deformer{},physics{};Array deformers{};
            std::memcpy(&anim,p+0x8d0,8);std::memcpy(&post,p+0x8d8,8);std::memcpy(&anim_class,p+0x8c8,8);std::memcpy(&generated,p+0x8c0,8);
            std::memcpy(&interactor,p+0xc40,8);std::memcpy(&deformer,p+0x588,8);std::memcpy(&physics,p+0x738,8);std::memcpy(&deformers,p+0x598,sizeof(Array));
            require(!anim&&!post&&!anim_class&&!generated&&!interactor&&!deformer&&!physics&&vertex_deformer_mask&&!(p[0x580]&vertex_deformer_mask)&&
                deformers.count==0&&deformers.capacity>=0&&(!deformers.capacity||deformers.data)&&(p[0xa41]&1)&&!(p[0xa42]&2)&&(p[0xa42]&4)&&(p[0x88]&3)==3,
                "native pose render state/animation/cloth unsupported");}
    }
}
void pose_native(Obj object,uint32_t stage){
    auto* p=vertex_pure(object);
    if(stage==0)reinterpret_cast<void(*)(void*,void*)>(pose_image+0x3bc7c70)(p,nullptr);
    else if(stage==1)reinterpret_cast<void(*)(void*)>(pose_image+0x3c02930)(p);
    else if(stage==2)reinterpret_cast<void(*)(void*,uint32_t,uint8_t)>(pose_image+0x3bf7370)(p,0,0);
    else if(stage==3)reinterpret_cast<void(*)(void*)>(pose_image+0x3c23f20)(p);
    else reinterpret_cast<void(*)(void*)>(pose_image+(stage==4?0x3b51a10:0x3b517d0))(p);
}
void (*pose_native_call)(Obj,uint32_t)=pose_native;
void pose_call(const PoseBinding& b,Obj component,uint32_t stage,HsmpViewResult* r){
    qualify(b.world,b.owner,b.calculator,r);qualify(b.world,b.owner,b.render,r);check_guard();pose_pure(b);
    const auto calculator=pose_buffers(vertex_pure(b.calculator),b.count),render=pose_buffers(vertex_pure(b.render),b.count);const auto local=pose_local(vertex_pure(b.calculator),b.count);
    pose_native_call(component,stage);
    check_guard();qualify(b.world,b.owner,b.calculator,r);qualify(b.world,b.owner,b.render,r);check_guard();pose_pure(b);if(r)++r->operations;
    const auto after_calculator=pose_buffers(vertex_pure(b.calculator),b.count),after_render=pose_buffers(vertex_pure(b.render),b.count);
    require(pose_array_equal(local,pose_local(vertex_pure(b.calculator),b.count)),"native pose local allocation changed during callback");
    for(size_t i=0;i<2;++i)require(pose_array_equal(calculator.arrays[i],after_calculator.arrays[i])&&pose_array_equal(render.arrays[i],after_render.arrays[i]),
        "native pose engine allocation changed during callback");
}
PoseBinding pose_bind(Obj world,Obj owner,Obj calculator,Obj render,Obj mesh,uint32_t count,HsmpViewResult* r){
    require(count>0&&count<=512,"native pose full bone dictionary bound");vertex_live(mesh);pose_profile(calculator,true);pose_profile(render,false);
    const auto level=returned(owner,L"/Script/Engine.Actor:GetLevel",r);require(level.weak&&property(level,L"OwningWorld",L"ObjectProperty",8).offset==0xc0,"native pose owner level layout");
    PoseBinding out{world,owner,level,calculator,render,mesh,count};
    vertex_live(world);vertex_live(owner);vertex_live(level);qualify(world,owner,calculator,r);qualify(world,owner,render,r);
    for(auto component:{calculator,render}){Function n(L"/Script/Engine.SkinnedMeshComponent:GetNumBones");n.call(component,r);require(n.value<int32_t>(L"ReturnValue",L"IntProperty")==static_cast<int32_t>(count),"native pose full dictionary count changed");}
    check_guard();pose_pure(out);pose_buffers(vertex_pure(render),count);pose_buffers(vertex_pure(calculator),count);pose_local(vertex_pure(calculator),count);return out;
}
struct PosePublished {PoseBuffers calculator{},render{};std::vector<EngineTransform> values;};
PosePublished pose_transfer(const PoseBinding& b,HsmpViewResult* r){
    pose_call(b,b.calculator,0,r);check_guard();pose_pure(b);
    auto calculator=pose_buffers(vertex_pure(b.calculator),b.count);auto render=pose_buffers(vertex_pure(b.render),b.count);
    const auto bytes=static_cast<size_t>(b.count)*sizeof(EngineTransform);
    for(const auto& a:calculator.arrays)for(const auto& target:render.arrays){const auto source=reinterpret_cast<uintptr_t>(a.data),destination=reinterpret_cast<uintptr_t>(target.data);
        require(source+bytes<=destination||destination+bytes<=source,"native pose calculator/render storage overlaps");}
    std::vector<EngineTransform> values(b.count);std::memcpy(values.data(),calculator.arrays[calculator.read].data,values.size()*sizeof(EngineTransform));
    for(const auto& value:values)wire(value);
    check_guard();pose_pure(b);const auto current_calculator=pose_buffers(vertex_pure(b.calculator),b.count);const auto current_render=pose_buffers(vertex_pure(b.render),b.count);
    require(current_calculator.read==calculator.read&&current_render.editable==render.editable&&current_render.read==render.read,"native pose buffers changed before copy");
    for(size_t i=0;i<2;++i)require(pose_array_equal(calculator.arrays[i],current_calculator.arrays[i])&&pose_array_equal(render.arrays[i],current_render.arrays[i]),"native pose engine allocation changed before copy");
    require(std::memcmp(values.data(),calculator.arrays[calculator.read].data,values.size()*sizeof(EngineTransform))==0,"native pose calculator changed before copy");
    std::memcpy(render.arrays[render.editable].data,values.data(),values.size()*sizeof(EngineTransform));
    static_cast<uint8_t*>(vertex_pure(b.render))[0x798]|=0x40; // exact FillCS publication flag
    pose_call(b,b.render,1,r);
    for(uint32_t stage=2;stage<=5;++stage)pose_call(b,b.render,stage,r);
    check_guard();pose_pure(b);const auto final_render=pose_buffers(vertex_pure(b.render),b.count);const auto final_calculator=pose_buffers(vertex_pure(b.calculator),b.count);
    for(size_t i=0;i<2;++i)require(pose_array_equal(render.arrays[i],final_render.arrays[i])&&pose_array_equal(calculator.arrays[i],final_calculator.arrays[i]),"native pose engine allocation changed during publication");
    require(final_render.read==render.editable&&final_render.editable==render.read&&!(final_render.flags&0x40)&&
        std::memcmp(values.data(),final_render.arrays[final_render.read].data,values.size()*sizeof(EngineTransform))==0,"native pose publication/read buffer mismatch");
    return {final_calculator,final_render,std::move(values)};
}
void pose_final(const PoseBinding& b,const PosePublished& expected){
    pose_pure(b);const auto calculator=pose_buffers(vertex_pure(b.calculator),b.count),render=pose_buffers(vertex_pure(b.render),b.count);
    require(render.read==expected.render.read&&render.editable==expected.render.editable&&calculator.read==expected.calculator.read&&!(render.flags&0x40)&&!(calculator.flags&0x40),"native pose final indices/dirty state changed");
    for(size_t i=0;i<2;++i)require(pose_array_equal(render.arrays[i],expected.render.arrays[i])&&pose_array_equal(calculator.arrays[i],expected.calculator.arrays[i]),"native pose final allocation changed");
    require(std::memcmp(expected.values.data(),render.arrays[render.read].data,expected.values.size()*sizeof(EngineTransform))==0&&
        std::memcmp(expected.values.data(),calculator.arrays[calculator.read].data,expected.values.size()*sizeof(EngineTransform))==0,"native pose final complete transform changed");
}
