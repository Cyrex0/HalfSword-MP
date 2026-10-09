// One complete synchronous source frame. Never retained across API calls.
struct CaptureReceiver {
    Obj owner{},component{},level{},parent{},root{};
    LookupEntry owner_path,component_path,level_path,parent_path,root_path;
    uint64_t socket{};int32_t world_offset{},root_offset{};
};
struct CaptureWatch {Obj world{};std::vector<CaptureReceiver> receivers;std::vector<LookupEntry> dispatch;};
bool capture_owner_build(){
    const auto* image=reinterpret_cast<const uint8_t*>(GetModuleHandleW(nullptr));if(!image)return false;
    const auto* dos=reinterpret_cast<const IMAGE_DOS_HEADER*>(image);if(dos->e_magic!=IMAGE_DOS_SIGNATURE||dos->e_lfanew<0||dos->e_lfanew>65536)return false;
    const auto* pe=reinterpret_cast<const IMAGE_NT_HEADERS64*>(image+dos->e_lfanew);
    if(pe->Signature!=IMAGE_NT_SIGNATURE||pe->OptionalHeader.Magic!=IMAGE_NT_OPTIONAL_HDR64_MAGIC||pe->OptionalHeader.SizeOfImage<0x3b541b0)return false;
    const uint8_t code[]={0x48,0x8b,0x42,0x20,0x45,0x33,0xc9,0x48,0x85,0xc0,0x41,0x0f,0x95,0xc1,0x4c,0x03,0xc8,0x4c,0x89,0x4a,0x20,0x48,0x8b,0x81,0x90,0,0,0,0x49,0x89,0,0xc3};
    return std::memcmp(image+0x3b54190,code,sizeof(code))==0;
}
bool (*capture_owner_admit)()=capture_owner_build;
LookupEntry capture_path(Obj original){
    require(source_package_name,"native batch package metadata missing");LookupEntry entry;entry.package=*source_package_name;
    auto node=lookup_node(get(original),entry,false,nullptr);const auto first=node;node.weak=original.weak;
    for(;;){require(entry.original.size()<64,"native batch original path bound");for(const auto& prior:entry.original)require(prior.address!=node.address,"native batch original path cycle");entry.original.push_back(node);
        const auto* p=lookup_node_get(node,entry.zero_item);const auto* flags=retirement_flags(p);const auto* cls_flags=retirement_flags(vt->resolve(node.class_weak));require(flags&&cls_flags,"native batch RF metadata missing");entry.flags.push_back(*flags);entry.class_flags.push_back(*cls_flags);
        const auto* outer=source_outer(p);require(outer,"native batch original Outer missing");if(!*outer)break;node=lookup_node(const_cast<void*>(*outer),entry,true,nullptr);}
    entry.pinned=entry.original;entry.pinned.front()=first;lookup_pin(entry);lookup_entry_final(entry);return entry;
}
void capture_watch_raw(const CaptureWatch& watch){
    require(capture_owner_admit&&capture_owner_admit(),"native batch GetOwner profile changed");
    require(same(watch.world,active_lookup->world),"native batch original world changed");
    for(const auto& row:watch.receivers){const auto* owner=reinterpret_cast<const uint8_t*>(row.owner.address);const auto* level=reinterpret_cast<const uint8_t*>(row.level.address);const auto* p=reinterpret_cast<const uint8_t*>(row.component.address);
        require(source_outer(owner)&&*source_outer(owner)==reinterpret_cast<const void*>(row.level.address),"native batch original owner Level changed");
        uint64_t world{},actual_owner{},parent{},socket{},root{};std::memcpy(&world,level+row.world_offset,8);std::memcpy(&actual_owner,p+0x90,8);std::memcpy(&parent,p+0xb0,8);std::memcpy(&socket,p+0xb8,8);std::memcpy(&root,owner+row.root_offset,8);
        require(row.world_offset==0xc0&&world==watch.world.address&&actual_owner==row.owner.address&&parent==row.parent.address&&socket==row.socket&&root==row.root.address,"native batch original world/owner/root/parent/socket changed");
    }
}
void capture_watch_links(){if(active_capture_watch){require(active_lookup,"native batch lookup scope missing");capture_watch_raw(*active_capture_watch);}}
void capture_watch_finish(){if(active_capture_watch)lookup_finish();}
struct CaptureWatchScope {
    const CaptureWatch* previous{active_capture_watch};
    explicit CaptureWatchScope(const CaptureWatch& watch){active_capture_watch=&watch;}
    ~CaptureWatchScope(){active_capture_watch=previous;}
};
CaptureReceiver capture_receiver(Obj world,const HsmpViewCaptureTarget& target,HsmpViewResult* r){
    require(target.recipe&&target.output,"native batch row arguments");qualify(world,target.owner,target.component,r);
    require(property(target.component,L"AttachParent",L"ObjectProperty",8).offset==0xb0&&property(target.component,L"AttachSocketName",L"NameProperty",8).offset==0xb8,"native batch receiver hard layout");
    CaptureReceiver row;row.owner=target.owner;row.component=target.component;row.owner_path=capture_path(row.owner);row.component_path=capture_path(row.component);
    row.root_offset=property(row.owner,L"RootComponent",L"ObjectProperty",8).offset;row.root=object_property(row.owner,L"RootComponent");if(row.root.weak)row.root_path=capture_path(row.root);
    row.level=returned(row.owner,L"/Script/Engine.Actor:GetLevel",r);require(is(row.level,L"/Script/Engine.Level"),"native batch original Level class");row.world_offset=property(row.level,L"OwningWorld",L"ObjectProperty",8).offset;require(row.world_offset==0xc0,"native batch Level world layout");row.level_path=capture_path(row.level);
    const auto parent=object_property(row.component,L"AttachParent");row.parent=parent;if(parent.weak)row.parent_path=capture_path(parent);
    row.socket=read<uint64_t>(row.component,L"AttachSocketName",L"NameProperty");qualify(world,row.owner,row.component,r);check_guard();return row;
}
void capture_textures(const HsmpViewCaptureTarget& row){
    require(pointers(row.textures,row.texture_count,4096)&&row.output->texture_count==row.texture_count,"native batch texture dictionary mismatch");
    for(uint32_t i=0;i<row.texture_count;++i){const auto path=text(row.textures[i]);const auto actual=row.output->textures[i];const auto expected=path.empty()?Obj{}:find(path.c_str());
        require(actual.address==expected.address&&(!actual.address||reinterpret_cast<uint64_t>(get(actual))==actual.address),"source texture recipe changed");}
}
int32_t capture_frame(Obj world,const HsmpViewCaptureTarget* rows,uint32_t count,const HsmpViewGuard* guard,HsmpViewResult* r){
    const bool trace_allowed=active_capture_watch==nullptr;
    try{initialize_result(r);thread();require(pointers(rows,count,32*64)&&count>0,"native batch complete-frame bound");
        {OperationScope scope(guard,world);require(capture_owner_admit&&capture_owner_admit(),"native batch GetOwner profile unsupported");CaptureWatch watch;watch.world=world;watch.receivers.reserve(count);CaptureWatchScope watched(watch);
            for(const auto* path:{L"/Script/Engine.ActorComponent:GetOwner",L"/Script/Engine.Actor:GetLevel"}){Function f(path);require(f.fields.size()==1&&f.field(L"ReturnValue",L"ObjectProperty",8).offset==0,"native batch owner/Level getter ABI");watch.dispatch.push_back(capture_path(f.function));watch.dispatch.push_back(capture_path(f.cls));}
            for(uint32_t i=0;i<count;++i){require(rows[i].recipe&&rows[i].output,"native batch row missing");for(uint32_t j=0;j<i;++j)require(rows[i].component.address!=rows[j].component.address,"native batch duplicate original component");watch.receivers.push_back(capture_receiver(world,rows[i],r));capture_watch_finish();}
            for(const auto& row:watch.receivers)require(!row.parent.weak||std::any_of(watch.receivers.begin(),watch.receivers.end(),[&](const auto& parent){return parent.component.address==row.parent.address;}),"native batch parent outside complete original roster");
            for(uint32_t i=0;i<count;++i){CaptureTrace trace(trace_allowed);capture_body(world,rows[i].owner,rows[i].component,rows[i].recipe,rows[i].output,trace,r);trace.row.complete=0;capture_textures(rows[i]);check_guard();lookup_finish();trace.row.complete=1;}
            check_guard();lookup_finish();}
        lookup_trace_flush();r->complete=1;return 1;
    }catch(const std::exception& e){lookup_trace_flush();failure(r,e.what());return -1;}
}
