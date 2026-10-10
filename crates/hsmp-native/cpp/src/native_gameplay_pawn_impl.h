// Included inside native_presentation.cpp's private namespace. Real native
// Willie construction is separate from the inert visual-mirror provider.
struct GameplayNativeProof {LookupEntry root_path;uintptr_t image{};uint64_t table{};uint8_t body_flags{},notification_flags{};};
void gameplay_native_guard();
struct GameplayApplied {
    HsmpGameplayState expected{};HsmpGameplayProof proof{};
    Obj root{},movement{},mesh{},asset{};
    LookupEntry root_path,movement_path,mesh_path,asset_path,camera_path,hud_path,owner_api_path,transform_api_path;
    std::array<double,3> relative_rotation{},scale{};
    HsmpProp root_field{},parent{},position{},rotation{},velocity{},health{},stamina{},mesh_field{},visible{},hidden{},skinned{},skeletal{};
    HsmpProp manager{},pc_owner{},view_target{},pending_target{},hud_field{},show_hud{};
    GameplayNativeProof native;
};
struct GameplayCurrent {
    Obj controller_level{};LookupEntry level_path,get_level_path,local_path;
    HsmpProp level_world{},local{};uint64_t vtable{};
};
enum GameplayCurrentReason:uint32_t {GP_CURRENT_UNENTERED,GP_CURRENT_WORLD,GP_CURRENT_ABSENT,GP_CURRENT_SCHEMA,GP_CURRENT_CODE,GP_CURRENT_VTABLE,GP_CURRENT_TARGET,GP_CURRENT_ZERO,GP_CURRENT_HOT,GP_CURRENT_LEVEL};
struct GameplayCurrentObservation {uint32_t reason{GP_CURRENT_UNENTERED},code{},cold_code{},found{},type{},stage{},operations{},complete{};HsmpProp schema{};bool hot{};};
std::atomic<uint32_t> gameplay_current_attempts{};
struct GameplayQuatSnapshot {uint32_t available{},reason{},cache{};uint64_t class_name{},slot{};std::array<double,4> world{},cached{};std::array<double,3> relative{},euler{};};
bool gameplay_quat_readable(const void* pointer,size_t bytes){
    uintptr_t at=reinterpret_cast<uintptr_t>(pointer);if(!at||bytes>UINTPTR_MAX-at)return false;const auto end=at+bytes;
    while(at<end){MEMORY_BASIC_INFORMATION region{};if(VirtualQuery(reinterpret_cast<const void*>(at),&region,sizeof(region))!=sizeof(region)||region.State!=MEM_COMMIT||(region.Protect&(PAGE_GUARD|PAGE_NOACCESS)))return false;
        const auto protection=region.Protect&0xff;const bool read=protection==PAGE_READONLY||protection==PAGE_READWRITE||protection==PAGE_WRITECOPY||protection==PAGE_EXECUTE_READ||protection==PAGE_EXECUTE_READWRITE||protection==PAGE_EXECUTE_WRITECOPY;
        const auto begin=reinterpret_cast<uintptr_t>(region.BaseAddress);if(!read||region.RegionSize>UINTPTR_MAX-begin||begin+region.RegionSize<=at)return false;at=begin+region.RegionSize;}
    return true;
}
uint64_t gameplay_quat_hash(const uint8_t* bytes,size_t count){uint64_t hash=14695981039346656037ULL;for(size_t i=0;i<count;++i)hash=(hash^bytes[i])*1099511628211ULL;return hash;}
bool gameplay_quat_image(uintptr_t& image,uint32_t& size){
    image=reinterpret_cast<uintptr_t>(GetModuleHandleW(nullptr));if(!gameplay_quat_readable(reinterpret_cast<const void*>(image),sizeof(IMAGE_DOS_HEADER)))return false;
    const auto* dos=reinterpret_cast<const IMAGE_DOS_HEADER*>(image);if(dos->e_magic!=IMAGE_DOS_SIGNATURE||dos->e_lfanew<=0||dos->e_lfanew>=65536||!gameplay_quat_readable(reinterpret_cast<const void*>(image+dos->e_lfanew),sizeof(IMAGE_NT_HEADERS64)))return false;
    const auto* pe=reinterpret_cast<const IMAGE_NT_HEADERS64*>(image+dos->e_lfanew);size=pe->OptionalHeader.SizeOfImage;
    if(pe->Signature!=IMAGE_NT_SIGNATURE||pe->FileHeader.Machine!=IMAGE_FILE_MACHINE_AMD64||pe->OptionalHeader.Magic!=IMAGE_NT_OPTIONAL_HDR64_MAGIC)return false;
    for(const auto& [rva,count,hash]:{std::tuple<uint32_t,size_t,uint64_t>{0x22177d0,149,0x8d52cc5f8773eb8bULL},{0x3bf1e1a,245,0x662bb9275bcf9788ULL},{0x3bf54ed,7,0x614130ebfc0c0c2aULL}}){
        if(rva>size||count>size-rva||!gameplay_quat_readable(reinterpret_cast<const void*>(image+rva),count)||gameplay_quat_hash(reinterpret_cast<const uint8_t*>(image+rva),count)!=hash)return false;}
    return true;
}
bool gameplay_quat_slot(const void* root,uintptr_t image,uint32_t size,uint64_t& rva){
    uint64_t table{},target{};std::memcpy(&table,root,8);if(!gameplay_quat_readable(reinterpret_cast<const void*>(table),0x540))return false;
    std::memcpy(&target,reinterpret_cast<const uint8_t*>(table)+0x538,8);if(target<image||target-image>=size)return false;
    MEMORY_BASIC_INFORMATION region{};if(VirtualQuery(reinterpret_cast<const void*>(target),&region,sizeof(region))!=sizeof(region)||region.State!=MEM_COMMIT||(region.Protect&(PAGE_GUARD|PAGE_NOACCESS)))return false;
    const auto protection=region.Protect&0xff;if(protection!=PAGE_EXECUTE&&protection!=PAGE_EXECUTE_READ&&protection!=PAGE_EXECUTE_READWRITE&&protection!=PAGE_EXECUTE_WRITECOPY)return false;rva=target-image;return true;
}
bool gameplay_quat_copy(const void* root,GameplayQuatSnapshot& out){
    if(!gameplay_quat_readable(root,0x1f0)){out.reason=2;return false;}
    std::memcpy(out.relative.data(),static_cast<const uint8_t*>(root)+0x140,24);std::memcpy(out.world.data(),static_cast<const uint8_t*>(root)+0x1d0,32);
    const void* cache{};std::memcpy(&cache,static_cast<const uint8_t*>(root)+0x1c0,8);
    if(!cache){out.reason=3;return true;}
    if(reinterpret_cast<uintptr_t>(cache)%alignof(double)||!gameplay_quat_readable(cache,64)){out.reason=4;return true;}
    std::memcpy(out.cached.data(),cache,32);std::memcpy(out.euler.data(),static_cast<const uint8_t*>(cache)+0x20,24);
    const void* final{};std::memcpy(&final,static_cast<const uint8_t*>(root)+0x1c0,8);if(final!=cache){out.reason=5;return false;}out.cache=1;return true;
}
std::atomic<uint32_t> gameplay_quat_attempts{};std::atomic<bool> gameplay_quat_failure{};
struct GameplayQuatTrace {
    HsmpPresentationCreateLog logger{create_logger.load()};uint32_t attempt{},complete{};bool active{};
    GameplayQuatSnapshot before,after;std::array<double,4> requested{};
    HsmpNativePathNode original_root{};uint32_t root_flags{},root_class_flags{};bool root_bound{};
    GameplayQuatTrace(){if(!logger)return;auto used=gameplay_quat_attempts.load();while(used<8){if(gameplay_quat_attempts.compare_exchange_weak(used,used+1)){attempt=used+1;break;}}active=attempt||!gameplay_quat_failure.load();}
    void values(const char* label,const double* data,size_t count,uint32_t edge)const{
        char text[320]{};size_t at{};for(size_t i=0;i<count;++i){uint64_t bits{};std::memcpy(&bits,data+i,8);const auto n=std::snprintf(text+at,sizeof(text)-at,"%s%zu=%.17g/%016llx ",label,i,data[i],static_cast<unsigned long long>(bits));if(n<0||static_cast<size_t>(n)>=sizeof(text)-at)break;at+=static_cast<size_t>(n);}
        logger("gameplay_quat",edge,0,attempt,complete,0,0,text);
    }
    ~GameplayQuatTrace(){if(!active)return;if(!attempt&&!complete){if(!gameplay_quat_failure.exchange(true))attempt=9;}if(!attempt)return;
        values("requested_q",requested.data(),4,0);
        for(const auto& [edge,row]:{std::pair<uint32_t,const GameplayQuatSnapshot*>{1,&before},{2,&after}}){char label[192]{};
            std::snprintf(label,sizeof(label),"available=%u reason=%u cache=%u class_fname=%016llx move_rva=%llx cached_equals_requested=%u evidence_only=true",row->available,row->reason,row->cache,static_cast<unsigned long long>(row->class_name),static_cast<unsigned long long>(row->slot),row->cache&&std::memcmp(row->cached.data(),requested.data(),32)==0?1u:0u);
            logger("gameplay_quat",edge,0,attempt,complete,0,0,label);if(!row->available)continue;
            values("world_q",row->world.data(),4,edge);values("relative",row->relative.data(),3,edge);
            if(row->cache){values("cached_q",row->cached.data(),4,edge);values("cached_euler",row->euler.data(),3,edge);}
        }
    }
};
struct GameplayCurrentTrace {
    GameplayCurrentObservation observation;HsmpPresentationCreateLog logger{create_logger.load()};uint32_t attempt{};
    std::chrono::steady_clock::time_point started{};
    GameplayCurrentTrace(){if(logger){auto used=gameplay_current_attempts.load();while(used<8){if(gameplay_current_attempts.compare_exchange_weak(used,used+1)){attempt=used+1;started=std::chrono::steady_clock::now();break;}}}}
    ~GameplayCurrentTrace(){if(!attempt)return;
        constexpr const char* reasons[]{"unentered","world_qualification","cold_profile_absent","bool_schema","code_fingerprint","vtable","virtual_target","zero_byte","positive_branch","controller_level"};
        const auto& row=observation;char label[160]{};std::snprintf(label,sizeof(label),"route=%s reason=%s code=%u cold_code=%u found=%u type=%u offset=%d size=%d byte=%u mask=%u",
            row.complete?(row.hot?"hot":"legacy"):"refused",row.reason<std::size(reasons)?reasons[row.reason]:"refused",row.code,row.cold_code,row.found,row.type,row.schema.offset,row.schema.size,row.schema.bool_offset,row.schema.bool_mask);
        const auto elapsed=static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::microseconds>(std::chrono::steady_clock::now()-started).count());
        logger("gameplay_current",row.complete,elapsed,attempt,row.operations,row.stage,row.reason,label);
    }
};
struct GameplayPawn {
    Obj world{},controller{},pawn{},actor_class{},level{};
    Transform initial{};uint32_t own{},stage{};
    LookupEntry world_path,controller_path,pawn_path;
    HsmpProp level_world{},controller_pawn{},pawn_controller{};
    std::optional<GameplayCurrent> current;
    GameplayCurrentObservation current_binding;
    std::optional<GameplayApplied> applied;
};
std::map<uint64_t,GameplayPawn> gameplay_pawns;
std::mutex gameplay_mutex;
uint64_t gameplay_next_handle=1;
bool gameplay_layouts_verified{};
void gameplay_owner_code(){
    const auto* image=reinterpret_cast<const uint8_t*>(GetModuleHandleW(nullptr));require(image!=nullptr,"native gameplay shipping image unavailable");
    const auto* dos=reinterpret_cast<const IMAGE_DOS_HEADER*>(image);require(dos->e_magic==IMAGE_DOS_SIGNATURE&&dos->e_lfanew>0&&dos->e_lfanew<65536,"native gameplay shipping image invalid");
    const auto* pe=reinterpret_cast<const IMAGE_NT_HEADERS64*>(image+dos->e_lfanew);
    constexpr std::array<uint8_t,32> bytes{0x48,0x8b,0x42,0x20,0x45,0x33,0xc9,0x48,0x85,0xc0,0x41,0x0f,0x95,0xc1,0x4c,0x03,
        0xc8,0x4c,0x89,0x4a,0x20,0x48,0x8b,0x81,0x90,0,0,0,0x49,0x89,0,0xc3};
    require(pe->Signature==IMAGE_NT_SIGNATURE&&pe->FileHeader.Machine==IMAGE_FILE_MACHINE_AMD64&&pe->OptionalHeader.Magic==IMAGE_NT_OPTIONAL_HDR64_MAGIC&&
        pe->OptionalHeader.SizeOfImage>0x3b54190+bytes.size()&&std::memcmp(image+0x3b54190,bytes.data(),bytes.size())==0,"native gameplay original GetOwner code changed");
}
void gameplay_transform_code(){
    const auto* image=reinterpret_cast<const uint8_t*>(GetModuleHandleW(nullptr));require(image!=nullptr,"native gameplay shipping image unavailable");
    const auto* dos=reinterpret_cast<const IMAGE_DOS_HEADER*>(image);require(dos->e_magic==IMAGE_DOS_SIGNATURE&&dos->e_lfanew>0&&dos->e_lfanew<65536,"native gameplay shipping image invalid");
    const auto* pe=reinterpret_cast<const IMAGE_NT_HEADERS64*>(image+dos->e_lfanew);
    constexpr std::array<uint8_t,101> bytes{0x48,0x8b,0x42,0x20,0x45,0x33,0xc9,0x48,0x85,0xc0,0x41,0x0f,0x95,0xc1,0x4c,0x03,
        0xc8,0x4c,0x89,0x4a,0x20,0x48,0x8b,0x81,0xa0,0x01,0,0,0x48,0x85,0xc0,0x74,0x08,0x48,0x05,0xd0,0x01,0,0,0xeb,0x07,
        0x48,0x8d,0x05,0x20,0xe5,0x63,0x05,0x0f,0x28,0,0x41,0x0f,0x29,0,0x0f,0x28,0x48,0x10,0x41,0x0f,0x29,0x48,0x10,
        0x0f,0x28,0x40,0x20,0x41,0x0f,0x29,0x40,0x20,0x0f,0x28,0x48,0x30,0x41,0x0f,0x29,0x48,0x30,0x0f,0x28,0x40,0x40,
        0x41,0x0f,0x29,0x40,0x40,0x0f,0x28,0x48,0x50,0x41,0x0f,0x29,0x48,0x50,0xc3};
    require(pe->Signature==IMAGE_NT_SIGNATURE&&pe->FileHeader.Machine==IMAGE_FILE_MACHINE_AMD64&&pe->OptionalHeader.Magic==IMAGE_NT_OPTIONAL_HDR64_MAGIC&&
        pe->OptionalHeader.SizeOfImage>0x34a5990+bytes.size()&&std::memcmp(image+0x34a5990,bytes.data(),bytes.size())==0,"native gameplay original GetTransform code changed");
}
bool gameplay_current_code(uintptr_t& base,uint32_t* detail=nullptr){
    if(detail)*detail=1;
    const auto* image=reinterpret_cast<const uint8_t*>(GetModuleHandleW(nullptr));if(!image)return false;
    if(detail)*detail=2;
    const auto* dos=reinterpret_cast<const IMAGE_DOS_HEADER*>(image);if(dos->e_magic!=IMAGE_DOS_SIGNATURE||dos->e_lfanew<=0||dos->e_lfanew>=65536)return false;
    const auto* pe=reinterpret_cast<const IMAGE_NT_HEADERS64*>(image+dos->e_lfanew);
    if(pe->Signature!=IMAGE_NT_SIGNATURE||pe->FileHeader.Machine!=IMAGE_FILE_MACHINE_AMD64||pe->OptionalHeader.Magic!=IMAGE_NT_OPTIONAL_HDR64_MAGIC||pe->OptionalHeader.SizeOfImage<=0x37d50a9)return false;
    constexpr std::array<uint8_t,47> local_exec{0x40,0x53,0x48,0x83,0xec,0x20,0x48,0x8b,0x42,0x20,0x45,0x33,0xc9,0x48,0x85,0xc0,0x49,0x8b,0xd8,0x41,0x0f,0x95,0xc1,0x4c,0x03,0xc8,0x4c,0x89,0x4a,0x20,0x48,0x8b,0x01,0xff,0x90,0xa8,0x07,0x00,0x00,0x88,0x03,0x48,0x83,0xc4,0x20,0x5b,0xc3};
    constexpr std::array<uint8_t,73> local_leaf{0x40,0x53,0x48,0x83,0xec,0x20,0x80,0xb9,0xbc,0x06,0x00,0x00,0x00,0x48,0x8b,0xd9,0x75,0x2f,0xe8,0xa9,0x4e,0xcc,0xff,0x83,0xf8,0x01,0x75,0x08,0x32,0xc0,0x48,0x83,0xc4,0x20,0x5b,0xc3,0x83,0xf8,0x03,0x74,0x11,0x85,0xc0,0x74,0x0d,0x0f,0xb6,0x83,0xbc,0x06,0x00,0x00,0x48,0x83,0xc4,0x20,0x5b,0xc3,0xc6,0x83,0xbc,0x06,0x00,0x00,0x01,0xb0,0x01,0x48,0x83,0xc4,0x20,0x5b,0xc3};
    constexpr std::array<uint8_t,67> level_exec{0x48,0x89,0x5c,0x24,0x08,0x57,0x48,0x83,0xec,0x20,0x48,0x8b,0x42,0x20,0x45,0x33,0xc9,0x48,0x85,0xc0,0x49,0x8b,0xf8,0x48,0x8b,0xd9,0x41,0x0f,0x95,0xc1,0x4c,0x03,0xc8,0x4c,0x89,0x4a,0x20,0xe8,0xd6,0xa2,0x27,0x00,0x48,0x8b,0xd0,0x48,0x8b,0xcb,0xe8,0x5b,0x59,0x01,0xfe,0x48,0x8b,0x5c,0x24,0x30,0x48,0x89,0x07,0x48,0x83,0xc4,0x20,0x5f,0xc3};
    constexpr std::array<uint8_t,81> typed_outer{0x48,0x89,0x5c,0x24,0x08,0x57,0x48,0x83,0xec,0x20,0x48,0x8b,0xfa,0x48,0x8b,0xd9,0xe8,0x6b,0x75,0xf8,0xff,0x48,0x8b,0x43,0x20,0x0f,0x1f,0x80,0x00,0x00,0x00,0x00,0x48,0x85,0xc0,0x74,0x21,0x48,0x8b,0x48,0x10,0x4c,0x8d,0x47,0x30,0x49,0x63,0x50,0x08,0x3b,0x51,0x38,0x7f,0x0a,0x48,0x8b,0x49,0x30,0x4c,0x39,0x04,0xd1,0x74,0x06,0x48,0x8b,0x40,0x20,0xeb,0xda,0x48,0x8b,0x5c,0x24,0x30,0x48,0x83,0xc4,0x20,0x5f,0xc3};
    if(detail)*detail=3;if(std::memcmp(image+0x355aea0,local_exec.data(),local_exec.size()))return false;
    if(detail)*detail=4;if(std::memcmp(image+0x37d5060,local_leaf.data(),local_leaf.size()))return false;
    if(detail)*detail=5;if(std::memcmp(image+0x34a5200,level_exec.data(),level_exec.size()))return false;
    if(detail)*detail=6;if(std::memcmp(image+0x14bab90,typed_outer.data(),typed_outer.size()))return false;
    if(detail)*detail=0;
    base=reinterpret_cast<uintptr_t>(image);return true;
}
thread_local const GameplayPawn* gameplay_active{};
void gameplay_enum_disabled(Obj pawn,const wchar_t* field);
void gameplay_layouts(){
    layouts();if(gameplay_layouts_verified)return;
    layout(L"/Script/CoreUObject.Rotator",24,{{L"Pitch",L"DoubleProperty",0,8,nullptr},{L"Yaw",L"DoubleProperty",8,8,nullptr},{L"Roll",L"DoubleProperty",16,8,nullptr}});
    layout(L"/Script/Engine.TViewTarget",0x820,{{L"Target",L"ObjectProperty",0,8,nullptr}});gameplay_layouts_verified=true;
}

LookupEntry gameplay_path(Obj object){
    LookupEntry path;require(source_package_name!=nullptr,"native gameplay package metadata unavailable");path.package=*source_package_name;
    auto node=lookup_node(get(object),path,false,nullptr);
    for(;;){require(path.original.size()<64,"native gameplay original path bound");
        for(const auto& old:path.original)require(old.address!=node.address,"native gameplay original path cycle");
        path.original.push_back(node);const auto* pointer=lookup_node_get(node,path.zero_item);
        const auto* flags=retirement_flags(pointer);const auto* class_flags=retirement_flags(vt->resolve(node.class_weak));
        require(flags&&class_flags,"native gameplay original flags unavailable");path.flags.push_back(*flags);path.class_flags.push_back(*class_flags);
        const auto* outer=source_outer(pointer);require(outer,"native gameplay original Outer unavailable");
        if(!*outer)break;node=lookup_node(const_cast<void*>(*outer),path,true,nullptr);
    }
    path.pinned=path.original;lookup_pin(path);lookup_entry_final(path);return path;
}
void gameplay_current_paths(const GameplayPawn& entry){
    if(!entry.current)return;const auto& profile=*entry.current;
    lookup_entry_final(profile.level_path);lookup_entry_final(profile.get_level_path);lookup_entry_final(profile.local_path);
    const auto* pc=lookup_node_get(entry.controller_path.pinned.front(),entry.controller_path.zero_item);
    const auto* level=lookup_node_get(profile.level_path.pinned.front(),profile.level_path.zero_item);
    const auto* outer=source_outer(pc);require(outer&&reinterpret_cast<uint64_t>(*outer)==profile.controller_level.address,"native gameplay original controller level changed");
    void* world{};std::memcpy(&world,static_cast<const uint8_t*>(level)+profile.level_world.offset,8);
    require(reinterpret_cast<uint64_t>(world)==entry.world.address,"native gameplay original controller world changed");
}
// A current invocation owns copied expectations only. Every existing pure
// boundary still reads all original metadata and hard links afresh.
struct GameplayBoundaryRow {
    Obj world{},controller{},pawn{},level{},controller_level{};
    HsmpProp level_world{},controller_world{},controller_pawn{},pawn_controller{};
    uint32_t own{},stage{};bool current{};
};
struct GameplayBoundary {
    LookupState* operation{active_lookup};std::optional<uint64_t> package;
    std::vector<std::pair<uint64_t,LookupClassWitness>> classes;
    std::vector<std::pair<uint64_t,LookupObjectWitness>> objects;
    std::vector<GameplayBoundaryRow> rows;
    explicit GameplayBoundary(const GameplayPawn& active){
        LookupState collected;
        struct RecordScope {LookupState* previous{active_lookup};explicit RecordScope(LookupState& state){active_lookup=&state;}~RecordScope(){active_lookup=previous;}} record(collected);
        const auto add=[&](const GameplayPawn& entry){
            lookup_record(entry.world_path);lookup_record(entry.controller_path);lookup_record(entry.pawn_path);
            GameplayBoundaryRow row{entry.world,entry.controller,entry.pawn,entry.level,{},entry.level_world,{},entry.controller_pawn,entry.pawn_controller,entry.own,entry.stage,false};
            if(entry.current){const auto& current=*entry.current;
                lookup_record(current.level_path);lookup_record(current.get_level_path);lookup_record(current.local_path);
                row.controller_level=current.controller_level;row.controller_world=current.level_world;row.current=true;}
            rows.push_back(row);
        };
        bool retained{};
        for(const auto& [handle,entry]:gameplay_pawns){(void)handle;if(same(entry.world,active.world)){add(entry);retained=retained||&entry==&active;}}
        if(!retained)add(active);
        require(rows.size()<=32,"native gameplay original current roster bound");
        package=collected.witnesses.package;
        classes.assign(collected.witnesses.classes.begin(),collected.witnesses.classes.end());
        objects.assign(collected.witnesses.objects.begin(),collected.witnesses.objects.end());
    }
    void metadata()const{
        require(vt&&object_name&&retirement_flags&&source_outer&&source_package_name,"native gameplay shared metadata unavailable");
        require(!package||*source_package_name==*package,"native gameplay shared package changed");
        for(const auto& [address,saved]:classes){void* p=vt->resolve(saved.object.weak);
            require(saved.object.weak&&reinterpret_cast<uint64_t>(p)==address,"native gameplay shared class expired");
            const auto* flags=retirement_flags(p);require(flags&&*flags==saved.flags&&(*flags&0x40000000u)==0,"native gameplay shared class RF changed");
            const auto* n=object_name(p);require(n&&*n==saved.name&&(!saved.exact_serial||vt->weak(p)==saved.object.weak),"native gameplay shared class identity changed");}
        for(const auto& [address,saved]:objects){const auto& node=saved.node;
            void* p=node.weak?vt->resolve(node.weak):lookup_zero_object(saved.zero_item,address);
            require(reinterpret_cast<uint64_t>(p)==address,"native gameplay shared object expired");
            const auto* flags=retirement_flags(p);require(flags&&*flags==saved.flags&&(*flags&0x40000000u)==0,"native gameplay shared object RF changed");
            const auto* n=object_name(p);const auto* outer=source_outer(p);
            require(n&&*n==node.name&&vt->class_of(p)==reinterpret_cast<void*>(node.class_address)&&outer&&reinterpret_cast<uint64_t>(*outer)==saved.outer,"native gameplay shared original path changed");
            if(!node.weak)require(node.class_name==*source_package_name&&saved.outer==0&&lookup_zero_object(saved.zero_item,address)==p,"native gameplay shared zero Package changed");}
        require(!package||*source_package_name==*package,"native gameplay shared package changed during proof");
    }
    void final()const{
        metadata();
        for(const auto& row:rows){
            const auto* pawn=vt->resolve(row.pawn.weak);const auto* pc=vt->resolve(row.controller.weak);const auto* level=vt->resolve(row.level.weak);
            require(reinterpret_cast<uint64_t>(pawn)==row.pawn.address&&reinterpret_cast<uint64_t>(pc)==row.controller.address&&reinterpret_cast<uint64_t>(level)==row.level.address,"native gameplay original raw link receiver expired");
            const auto* outer=source_outer(pawn);require(outer&&reinterpret_cast<uint64_t>(*outer)==row.level.address,"native gameplay original level changed");
            void* world{};std::memcpy(&world,static_cast<const uint8_t*>(level)+row.level_world.offset,8);
            require(reinterpret_cast<uint64_t>(world)==row.world.address,"native gameplay original world changed");
            if(row.current){const auto* controller_level=vt->resolve(row.controller_level.weak);
                require(reinterpret_cast<uint64_t>(controller_level)==row.controller_level.address,"native gameplay original controller level expired");
                const auto* controller_outer=source_outer(pc);require(controller_outer&&reinterpret_cast<uint64_t>(*controller_outer)==row.controller_level.address,"native gameplay original controller level changed");
                std::memcpy(&world,static_cast<const uint8_t*>(controller_level)+row.controller_world.offset,8);
                require(reinterpret_cast<uint64_t>(world)==row.world.address,"native gameplay original controller world changed");}
            if(row.stage>=3&&row.own){void* owned{};void* controller{};
                std::memcpy(&owned,static_cast<const uint8_t*>(pc)+row.controller_pawn.offset,8);
                std::memcpy(&controller,static_cast<const uint8_t*>(pawn)+row.pawn_controller.offset,8);
                require(reinterpret_cast<uint64_t>(owned)==row.pawn.address&&reinterpret_cast<uint64_t>(controller)==row.controller.address,"native gameplay original possession changed");}
        }
        metadata();
    }
};
thread_local const GameplayBoundary* gameplay_boundary{};
bool gameplay_boundary_active(){return gameplay_boundary&&gameplay_boundary->operation==active_lookup;}
struct GameplayBoundaryScope {
    const GameplayBoundary* previous{gameplay_boundary};GameplayBoundary boundary;
    explicit GameplayBoundaryScope(const GameplayPawn& entry):boundary(entry){gameplay_boundary=&boundary;}
    ~GameplayBoundaryScope(){gameplay_boundary=previous;}
};
void gameplay_pure(const GameplayPawn& entry){
    if(gameplay_boundary_active()){gameplay_boundary->final();return;}
    lookup_entry_final(entry.world_path);lookup_entry_final(entry.controller_path);lookup_entry_final(entry.pawn_path);
    const auto* pawn=lookup_node_get(entry.pawn_path.pinned.front(),entry.pawn_path.zero_item);
    const auto* outer=source_outer(pawn);require(outer&&reinterpret_cast<uint64_t>(*outer)==entry.level.address,"native gameplay original level changed");
    const auto* level=vt->resolve(entry.level.weak);require(level&&reinterpret_cast<uint64_t>(level)==entry.level.address,"native gameplay original level expired");
    void* world{};std::memcpy(&world,static_cast<const uint8_t*>(level)+entry.level_world.offset,8);
    require(reinterpret_cast<uint64_t>(world)==entry.world.address,"native gameplay original world changed");
    gameplay_current_paths(entry);
    if(entry.stage>=3&&entry.own){
        const auto* pc=lookup_node_get(entry.controller_path.pinned.front(),entry.controller_path.zero_item);
        void* owned{};void* controller{};
        std::memcpy(&owned,static_cast<const uint8_t*>(pc)+entry.controller_pawn.offset,8);
        std::memcpy(&controller,static_cast<const uint8_t*>(pawn)+entry.pawn_controller.offset,8);
        require(reinterpret_cast<uint64_t>(owned)==entry.pawn.address&&reinterpret_cast<uint64_t>(controller)==entry.controller.address,
            "native gameplay original possession changed");
    }
}
bool gameplay_current_positive(const GameplayPawn& entry,bool code_supported,uintptr_t base,GameplayCurrentObservation* observation=nullptr){
    const auto reason=[observation](uint32_t value){if(observation)observation->reason=value;};
    reason(GP_CURRENT_WORLD);gameplay_pure(entry);
    reason(entry.current_binding.reason?entry.current_binding.reason:GP_CURRENT_ABSENT);if(!entry.current)return false;
    reason(GP_CURRENT_CODE);if(!code_supported)return false;const auto& profile=*entry.current;
    reason(GP_CURRENT_SCHEMA);
    // The adapter publishes FBoolProperty::GetByteMask, not GetFieldMask.
    // This exact native property's observed ByteMask is1; the pinned native
    // IsLocalController leaf itself tests the entire byte against zero.
    if(profile.local.offset!=0x6bc||profile.local.size!=1||profile.local.bool_offset!=0||profile.local.bool_mask!=1)return false;
    const auto* pc=lookup_node_get(entry.controller_path.pinned.front(),entry.controller_path.zero_item);
    reason(GP_CURRENT_VTABLE);uint64_t table{},target{};std::memcpy(&table,pc,8);if(!table||table!=profile.vtable)return false;
    reason(GP_CURRENT_TARGET);std::memcpy(&target,reinterpret_cast<const uint8_t*>(table)+0x7a8,8);if(target!=base+0x37d5060)return false;
    const bool positive=static_cast<const uint8_t*>(pc)[profile.local.offset]!=0;
    reason(GP_CURRENT_WORLD);gameplay_pure(entry);
    if(positive){const auto* final_pc=lookup_node_get(entry.controller_path.pinned.front(),entry.controller_path.zero_item);
        uint64_t final_table{},final_target{};std::memcpy(&final_table,final_pc,8);
        require(final_table==profile.vtable,"native gameplay original controller vtable changed");
        std::memcpy(&final_target,reinterpret_cast<const uint8_t*>(final_table)+0x7a8,8);
        require(final_target==base+0x37d5060&&static_cast<const uint8_t*>(final_pc)[profile.local.offset]!=0,"native gameplay local controller changed");}
    reason(positive?GP_CURRENT_HOT:GP_CURRENT_ZERO);return positive;
}
void gameplay_current_bind(GameplayPawn& entry,HsmpViewResult* result){
    entry.current_binding.reason=GP_CURRENT_LEVEL;
    GameplayCurrent profile;profile.controller_level=returned(entry.controller,L"/Script/Engine.Actor:GetLevel",result);
    require(profile.controller_level.weak&&is(profile.controller_level,L"/Script/Engine.Level"),"native gameplay original controller level unavailable");
    const auto* pc_before=get(entry.controller);const auto* outer=source_outer(pc_before);const auto original_outer=outer?*outer:nullptr;get(entry.controller);
    if(reinterpret_cast<uint64_t>(original_outer)!=profile.controller_level.address)return;
    profile.level_world=property(profile.controller_level,L"OwningWorld",L"ObjectProperty",8);
    require(profile.level_world.offset==0xc0,"native gameplay controller world schema changed");
    auto* pc=get(entry.controller);const auto found=vt->obj_prop(pc,u16(L"bIsLocalPlayerController"),&profile.local);get(entry.controller);
    const bool bool_type=found==1&&profile.local.cls==name(L"BoolProperty");
    entry.current_binding.found=found==1?1u:0u;entry.current_binding.schema=profile.local;
    entry.current_binding.type=bool_type?1u:0u;entry.current_binding.reason=GP_CURRENT_SCHEMA;
    if(!bool_type||profile.local.offset!=0x6bc||profile.local.size!=1||profile.local.bool_offset!=0||profile.local.bool_mask!=1)return;
    entry.current_binding.reason=GP_CURRENT_CODE;uintptr_t image{};if(!gameplay_current_code(image,&entry.current_binding.code))return;
    std::memcpy(&profile.vtable,pc,8);
    const auto* dos=reinterpret_cast<const IMAGE_DOS_HEADER*>(image);const auto* pe=reinterpret_cast<const IMAGE_NT_HEADERS64*>(image+dos->e_lfanew);
    entry.current_binding.reason=GP_CURRENT_VTABLE;if(profile.vtable<image||profile.vtable-image>pe->OptionalHeader.SizeOfImage-0x7b0)return;
    entry.current_binding.reason=GP_CURRENT_TARGET;uint64_t target{};std::memcpy(&target,reinterpret_cast<const uint8_t*>(profile.vtable)+0x7a8,8);if(target!=image+0x37d5060)return;
    Function level_api(L"/Script/Engine.Actor:GetLevel");level_api.field(L"ReturnValue",L"ObjectProperty",8);
    Function local_api(L"/Script/Engine.Controller:IsLocalController");local_api.field(L"ReturnValue",L"BoolProperty",1);
    profile.level_path=gameplay_path(profile.controller_level);profile.get_level_path=gameplay_path(level_api.function);profile.local_path=gameplay_path(local_api.function);
    entry.current=std::move(profile);gameplay_pure(entry);entry.current_binding.reason=GP_CURRENT_HOT;
}
void gameplay_call_guard(){
    if(!gameplay_active)return;
    if(gameplay_boundary_active()){gameplay_boundary->final();return;}
    for(const auto& [handle,entry]:gameplay_pawns){(void)handle;if(same(entry.world,gameplay_active->world))gameplay_pure(entry);}
    gameplay_pure(*gameplay_active);
    gameplay_native_guard();
}
struct GameplayWatch {
    const GameplayPawn* previous{gameplay_active};
    explicit GameplayWatch(const GameplayPawn& entry){gameplay_pure(entry);gameplay_active=&entry;}
    ~GameplayWatch(){gameplay_active=previous;}
};
GameplayPawn& gameplay_entry(uint64_t handle){const auto it=gameplay_pawns.find(handle);require(handle&&it!=gameplay_pawns.end(),"native gameplay original handle unavailable");return it->second;}
void gameplay_local(const GameplayPawn& entry,HsmpViewResult* result){
    require(same(actor_world(entry.controller,result),entry.world),"native gameplay original controller world changed");
    Function local(L"/Script/Engine.Controller:IsLocalController");local.call(entry.controller,result);
    require(local.value<uint8_t>(L"ReturnValue",L"BoolProperty")!=0,"native gameplay local controller required");
    require(same(actor_world(entry.pawn,result),entry.world),"native gameplay original pawn world changed");
    require(vt->class_of(get(entry.pawn))==get(entry.actor_class),"native gameplay original Willie class changed");
    gameplay_pure(entry);
}
void gameplay_quat_snapshot(GameplayQuatTrace& trace,const GameplayPawn& entry,Obj root,GameplayQuatSnapshot& out){
    if(!trace.active)return;out.reason=1;
    try{uintptr_t image{};uint32_t size{};if(!gameplay_quat_image(image,size))return;
        gameplay_pure(entry);const auto node=trace.root_bound?trace.original_root:source_path_node(get(root));auto* object=source_path_get(node);const auto* cls=vt->resolve(node.class_weak);
        const auto flags=*retirement_flags(object),class_flags=*retirement_flags(cls);const auto* outer=source_outer(object);
        if(trace.root_bound&&(flags!=trace.root_flags||class_flags!=trace.root_class_flags)){out.reason=6;return;}
        if(!trace.root_bound){trace.original_root=node;trace.root_flags=flags;trace.root_class_flags=class_flags;trace.root_bound=true;}
        void* bound{};void* owner{};const auto* pawn=lookup_node_get(entry.pawn_path.pinned.front(),entry.pawn_path.zero_item);
        std::memcpy(&bound,static_cast<const uint8_t*>(pawn)+0x1a0,8);std::memcpy(&owner,static_cast<const uint8_t*>(object)+0x90,8);
        if(bound!=object||owner!=pawn||!outer||*outer!=pawn){out.reason=6;return;}
        out.class_name=node.class_name;if(!gameplay_quat_slot(object,image,size,out.slot)){out.reason=7;return;}out.reason=0;
        if(!gameplay_quat_copy(object,out))return;
        gameplay_pure(entry);source_path_get(node);const auto* final_outer=source_outer(object);
        std::memcpy(&bound,static_cast<const uint8_t*>(pawn)+0x1a0,8);std::memcpy(&owner,static_cast<const uint8_t*>(object)+0x90,8);
        if(bound!=object||owner!=pawn||!final_outer||*final_outer!=pawn||*retirement_flags(object)!=flags||*retirement_flags(cls)!=class_flags){out.reason=6;return;}out.available=1;
    }catch(const std::exception&){out.available=0;out.reason=6;}
}
struct alignas(16) GameplayCachePair {double q[4],rotation[3];uint64_t padding{};};
struct GameplayOverlapView {const void* data{};int32_t count{},padding{};};
static_assert(sizeof(GameplayCachePair)==64&&offsetof(GameplayCachePair,rotation)==32);
static_assert(sizeof(GameplayOverlapView)==16&&offsetof(GameplayOverlapView,count)==8);
using GameplayImport=void(*)(void*,const void* const*);
using GameplayAbsolute=uint8_t(*)(void*,const double*,const double*,uint8_t,uint8_t);
using GameplayOverlaps=uint8_t(*)(void*,const GameplayOverlapView*,uint8_t,const GameplayOverlapView*);
struct GameplayNativeCalls {GameplayImport import{};GameplayAbsolute absolute{};GameplayOverlaps overlaps{};};
uintptr_t gameplay_native_image(){
    uintptr_t image{};uint32_t size{};require(gameplay_quat_image(image,size),"native gameplay absolute image/code unavailable");
    struct Window{uint32_t rva,bytes;uint64_t hash;};
    constexpr Window windows[]{{0x3bf5ad0,401,0x9f9ffdef88936f35ULL},{0x3bf1970,2355,0x8a824d5b8f611ae4ULL},
        {0x3bf7430,2757,0xda3cf6857a774fb7ULL},{0x3bf4370,577,0x1b93861411267304ULL},
        {0x3bf7f40,125,0x2184e79ded3197a9ULL},{0x3bd6ee0,153,0x239a2ba825e91f19ULL},
        {0x22179d0,478,0x563d60f52ee4b5deULL},{0x3bda090,4063,0x7954ddbdced44e04ULL}};
    for(const auto& w:windows)require(w.rva<=size&&w.bytes<=size-w.rva&&gameplay_quat_readable(reinterpret_cast<const void*>(image+w.rva),w.bytes)&&
        gameplay_quat_hash(reinterpret_cast<const uint8_t*>(image+w.rva),w.bytes)==w.hash,"native gameplay absolute code changed");
    return image;
}
GameplayCachePair gameplay_cache_pair(const void* root){
    const void* cache{};std::memcpy(&cache,static_cast<const uint8_t*>(root)+0x1c0,8);
    require(cache&&reinterpret_cast<uintptr_t>(cache)%alignof(double)==0&&gameplay_quat_readable(cache,64),"native gameplay original rotation cache unavailable");
    GameplayCachePair pair{};std::memcpy(&pair,cache,56);const void* final{};std::memcpy(&final,static_cast<const uint8_t*>(root)+0x1c0,8);
    require(final==cache,"native gameplay original rotation cache changed");return pair;
}
bool gameplay_cache_import_needed(const GameplayCachePair& current,const GameplayCachePair& desired){
    for(const auto value:current.q)require(std::isfinite(value),"native gameplay current cache quaternion nonfinite");
    for(const auto value:current.rotation)require(std::isfinite(value),"native gameplay current cache rotation nonfinite");
    for(const auto value:desired.q)require(std::isfinite(value),"native gameplay cache quaternion nonfinite");
    for(const auto value:desired.rotation)require(std::isfinite(value),"native gameplay cache rotation nonfinite");
    if(std::memcmp(&current,&desired,56)==0)return false;
    bool differs{};for(size_t i=0;i<3;++i)differs=differs||current.rotation[i]!=desired.rotation[i];
    require(differs,"native gameplay same-Euler different rotation cache unsupported");return true;
}
void gameplay_native_root_fields(const void* root,const void* pawn,const void* outer,const GameplayNativeProof& proof){
    require(gameplay_quat_readable(root,0x1d0+96),"native gameplay original root storage unavailable");
    void* bound{};void* owner{};void* parent{};uint64_t table{};int32_t scopes{};
    std::memcpy(&bound,static_cast<const uint8_t*>(pawn)+0x1a0,8);std::memcpy(&owner,static_cast<const uint8_t*>(root)+0x90,8);
    std::memcpy(&parent,static_cast<const uint8_t*>(root)+0xb0,8);std::memcpy(&scopes,static_cast<const uint8_t*>(root)+0x1b0,4);std::memcpy(&table,root,8);
    require(bound==root&&owner==pawn&&outer==pawn&&!parent,"native gameplay original absolute root binding changed");
    require(scopes==0,"native gameplay scoped root movement unsupported");
    require(table&&table==proof.table&&gameplay_quat_readable(reinterpret_cast<const void*>(table),0x540),"native gameplay original root vtable changed");
    for(const auto& [slot,target]:{std::pair<uint32_t,uint32_t>{0x528,0x3bd6ee0},{0x530,0x3bda090},{0x538,0x3bd53c0}}){uint64_t address{};std::memcpy(&address,reinterpret_cast<const uint8_t*>(table)+slot,8);
        require(address==proof.image+target,"native gameplay root movement dispatch unsupported");}
    const auto* bytes=static_cast<const uint8_t*>(root);
    require((bytes[0x88]&9)==proof.body_flags&&(proof.body_flags&1)&&(bytes[0x18a]&8)==proof.notification_flags&&proof.notification_flags==8,
        "native gameplay root body/notification flags changed");
    // The importer may publish existing native state before copying the pair.
    // Admit every interpreted original transform/cache value before dispatch.
    for(const auto offset:{0x128u,0x140u,0x158u}){double values[3]{};std::memcpy(values,bytes+offset,24);
        for(const auto value:values)require(std::isfinite(value),"native gameplay current relative transform nonfinite");}
    EngineTransform world{};std::memcpy(&world,bytes+0x1d0,sizeof(world));
    for(const auto value:world.q)require(std::isfinite(value),"native gameplay current world quaternion nonfinite");
    for(const auto value:world.p)require(std::isfinite(value),"native gameplay current world position nonfinite");
    for(const auto value:world.scale)require(std::isfinite(value),"native gameplay current world scale nonfinite");
    const auto cache=gameplay_cache_pair(root);
    for(const auto value:cache.q)require(std::isfinite(value),"native gameplay current cache quaternion nonfinite");
    for(const auto value:cache.rotation)require(std::isfinite(value),"native gameplay current cache rotation nonfinite");
}
void gameplay_native_pure(const GameplayPawn& entry,const GameplayNativeProof& proof){
    gameplay_pure(entry);lookup_entry_final(proof.root_path);require(proof.image&&gameplay_native_image()==proof.image,"native gameplay original absolute image changed");
    const auto* root=lookup_node_get(proof.root_path.pinned.front(),proof.root_path.zero_item);const auto* pawn=lookup_node_get(entry.pawn_path.pinned.front(),entry.pawn_path.zero_item);
    const auto* outer=source_outer(root);require(outer!=nullptr,"native gameplay original root Outer unavailable");gameplay_native_root_fields(root,pawn,*outer,proof);
    lookup_entry_final(proof.root_path);gameplay_pure(entry);
}
struct GameplayNativeWatch;
thread_local const GameplayNativeWatch* gameplay_native_active{};
struct GameplayNativeWatch {
    const GameplayNativeWatch* previous{gameplay_native_active};const GameplayPawn& entry;const GameplayNativeProof& proof;const void* operation{active_lookup};
    GameplayNativeWatch(const GameplayPawn& e,const GameplayNativeProof& p):entry(e),proof(p){gameplay_native_pure(entry,proof);gameplay_native_active=this;}
    ~GameplayNativeWatch(){gameplay_native_active=previous;}
};
void gameplay_native_guard(){if(!gameplay_native_active)return;require(active_lookup==gameplay_native_active->operation,"native gameplay absolute operation reentry");
    gameplay_native_pure(gameplay_native_active->entry,gameplay_native_active->proof);}
GameplayNativeProof gameplay_native_bind(const GameplayPawn& entry,Obj root){
    GameplayNativeProof proof;proof.root_path=gameplay_path(root);proof.image=gameplay_native_image();
    require(vt->class_of(get(root))==get(find(L"/Script/Engine.CapsuleComponent")),"native gameplay exact Capsule root required");
    const auto position=property(root,L"RelativeLocation",L"StructProperty",24),rotation=property(root,L"RelativeRotation",L"StructProperty",24),scale=property(root,L"RelativeScale3D",L"StructProperty",24);
    require(property(entry.pawn,L"RootComponent",L"ObjectProperty",8).offset==0x1a0&&property(root,L"AttachParent",L"ObjectProperty",8).offset==0xb0&&
        position.offset==0x128&&position.sub==name(L"Vector")&&rotation.offset==0x140&&rotation.sub==name(L"Rotator")&&
        scale.offset==0x158&&scale.sub==name(L"Vector"),"native gameplay absolute root schema changed");
    const auto* bytes=static_cast<const uint8_t*>(get(root));std::memcpy(&proof.table,bytes,8);proof.body_flags=bytes[0x88]&9;proof.notification_flags=bytes[0x18a]&8;
    gameplay_owner_code();gameplay_native_pure(entry,proof);return proof;
}
template<class Guard>void gameplay_absolute_run(const HsmpGameplayState& state,const GameplayNativeCalls& calls,Guard&& guard){
    GameplayCachePair desired{};std::copy_n(state.orientation,4,desired.q);std::copy_n(state.cache_rotation,3,desired.rotation);
    auto* root=guard();const auto before=gameplay_cache_pair(root);const bool needed=gameplay_cache_import_needed(before,desired);
    if(needed){const void* source=&desired;calls.import(root,&source);root=guard();}
    const auto exact=[&desired](const void* object){const auto current=gameplay_cache_pair(object);return std::memcmp(&current,&desired,56)==0;};
    require(exact(root),"native gameplay exact rotation cache import failed");
    root=guard();require(exact(root),"native gameplay rotation cache changed before absolute update");
    const auto changed=calls.absolute(root,state.position,state.orientation,0,1);root=guard();
    require(exact(root),"native gameplay native rotation cache changed during absolute update");
    if(changed){const GameplayOverlapView empty{};root=guard();require(exact(root),"native gameplay rotation cache changed before overlap publication");calls.overlaps(root,&empty,1,nullptr);root=guard();}
    require(exact(root),"native gameplay native rotation cache changed during overlap publication");
}
int32_t gameplay_begin(Obj world,Obj controller,HsmpViewText class_path,const Transform* initial,uint32_t own,
    const HsmpViewGuard* guard,uint64_t* handle,Obj* pawn,HsmpViewResult* result){
    const std::lock_guard lock(gameplay_mutex);Obj created{};
    try{initialize_result(result);thread();OperationScope scope(guard,world);layouts();
        require(initial&&handle&&pawn&&own<=1,"native gameplay begin arguments");*handle=0;*pawn={};
        require(gameplay_pawns.size()<32,"native gameplay pawn bound");
        const auto path=text(class_path);require(path==L"/Game/Character/Blueprints/Willie_BP.Willie_BP_C","native gameplay exact Willie class required");
        auto cls=find(path.c_str());require(is(controller,L"/Script/Engine.PlayerController")&&same(actor_world(controller,result),world),"native gameplay controller qualification");
        Function local(L"/Script/Engine.Controller:IsLocalController");local.call(controller,result);
        require(local.value<uint8_t>(L"ReturnValue",L"BoolProperty")!=0,"native gameplay local controller required");
        Function begin(L"/Script/Engine.GameplayStatics:BeginDeferredActorSpawnFromClass");
        begin.object(L"WorldContextObject",world);begin.object(L"ActorClass",cls,true);begin.put(L"SpawnTransform",L"StructProperty",engine(*initial),L"Transform");
        begin.enumeration(L"CollisionHandlingOverride",1);begin.object(L"Owner",{});begin.enumeration(L"TransformScaleMethod",0);
        begin.call(find(L"/Script/Engine.Default__GameplayStatics"),result);created=begin.returned();
        require(created.weak&&vt->class_of(get(created))==get(cls)&&same(actor_world(created,result),world),"native gameplay deferred Willie qualification");
        // These replicas have no native AI authority and do not steal a local
        // controller during construction; possession is a qualified later stage.
        gameplay_enum_disabled(created,L"AutoPossessAI");gameplay_enum_disabled(created,L"AutoPossessPlayer");
        GameplayPawn entry;entry.world=world;entry.controller=controller;entry.pawn=created;entry.actor_class=cls;entry.initial=*initial;entry.own=own;entry.stage=1;
        entry.level=returned(created,L"/Script/Engine.Actor:GetLevel",result);entry.level_world=property(entry.level,L"OwningWorld",L"ObjectProperty",8);
        entry.controller_pawn=property(controller,L"Pawn",L"ObjectProperty",8);entry.pawn_controller=property(created,L"Controller",L"ObjectProperty",8);
        entry.world_path=gameplay_path(world);entry.controller_path=gameplay_path(controller);entry.pawn_path=gameplay_path(created);
        gameplay_current_bind(entry,result);
        gameplay_pure(entry);lookup_finish();require(gameplay_next_handle!=0,"native gameplay handle exhausted");
        const auto key=gameplay_next_handle++;gameplay_pawns.emplace(key,std::move(entry));*handle=key;*pawn=created;result->complete=1;return 1;
    }catch(const std::exception& error){failure(result,error.what());
        // A failed begin exposes no handle. Destroy only its original returned
        // actor while the caller's admission still holds; never search by name.
        if(created.weak){try{OperationScope cleanup(guard,world);destroy_actor(world,created);}catch(...){}}
        return -1;}
}
int32_t gameplay_current(uint64_t handle,const HsmpViewGuard* guard,Obj* pawn,HsmpViewResult* result){
    GameplayCurrentTrace trace;
    const std::lock_guard lock(gameplay_mutex);
    try{initialize_result(result);thread();auto& entry=gameplay_entry(handle);OperationScope scope(guard,entry.world);GameplayBoundaryScope boundary(entry);GameplayWatch watch(entry);
        trace.observation=entry.current_binding;trace.observation.cold_code=entry.current_binding.code;trace.observation.stage=entry.stage;trace.observation.reason=GP_CURRENT_WORLD;
        require(pawn!=nullptr,"native gameplay pawn output missing");uintptr_t image{};
        uint32_t code{};const bool code_supported=gameplay_current_code(image,&code);trace.observation.code=code;
        const bool hot=gameplay_current_positive(entry,code_supported,image,&trace.observation);trace.observation.hot=hot;
        if(!hot)gameplay_local(entry,result);lookup_finish();gameplay_pure(entry);
        if(hot){uintptr_t final_image{};const bool final_code=gameplay_current_code(final_image,&trace.observation.code);require(gameplay_current_positive(entry,final_code,final_image,&trace.observation),"native gameplay local controller changed");}
        *pawn=entry.pawn;result->complete=1;trace.observation.complete=1;trace.observation.operations=result->operations;return 1;
    }catch(const std::exception& error){failure(result,error.what());if(result)trace.observation.operations=result->operations;return -1;}
}
int32_t gameplay_construct(uint64_t handle,const HsmpViewGuard* guard,Obj* pawn,HsmpViewResult* result){
    const std::lock_guard lock(gameplay_mutex);
    try{initialize_result(result);thread();auto& entry=gameplay_entry(handle);OperationScope scope(guard,entry.world);GameplayWatch watch(entry);
        require(pawn&&entry.stage==1,"native gameplay construction stage");gameplay_local(entry,result);
        Function finish(L"/Script/Engine.GameplayStatics:FinishSpawningActor");finish.object(L"Actor",entry.pawn);
        finish.put(L"SpawnTransform",L"StructProperty",engine(entry.initial),L"Transform");finish.enumeration(L"TransformScaleMethod",0);
        finish.call(find(L"/Script/Engine.Default__GameplayStatics"),result);require(same(finish.returned(),entry.pawn),"native gameplay construction returned another pawn");
        gameplay_local(entry,result);lookup_finish();gameplay_pure(entry);entry.stage=2;*pawn=entry.pawn;result->complete=1;return 1;
    }catch(const std::exception& error){failure(result,error.what());return -1;}
}
int32_t gameplay_finish(uint64_t handle,const HsmpViewGuard* guard,HsmpViewResult* result){
    const std::lock_guard lock(gameplay_mutex);
    try{initialize_result(result);thread();auto& entry=gameplay_entry(handle);OperationScope scope(guard,entry.world);GameplayWatch watch(entry);
        require(entry.stage==2,"native gameplay initialization stage");gameplay_local(entry,result);
        if(entry.own){Function possess(L"/Script/Engine.Controller:Possess");possess.object(L"InPawn",entry.pawn);possess.call(entry.controller,result);
            require(same(returned(entry.controller,L"/Script/Engine.Controller:K2_GetPawn",result),entry.pawn)&&same(object_property(entry.pawn,L"Controller"),entry.controller),"native gameplay possession readback failed");}
        else require(!object_property(entry.pawn,L"Controller").weak,"native gameplay remote pawn unexpectedly possessed");
        Function disable(L"/Script/Engine.Actor:DisableInput");disable.object(L"PlayerController",entry.controller);disable.call(entry.pawn,result);
        // Remove the actor's native input stack, so local keys cannot execute
        // unacknowledged damage. Accepted movement is replayed explicitly.
        if(entry.own){Function move(L"/Script/Engine.Controller:ResetIgnoreMoveInput");move.call(entry.controller,result);
            Function look(L"/Script/Engine.Controller:ResetIgnoreLookInput");look.call(entry.controller,result);}
        entry.stage=3;gameplay_local(entry,result);lookup_finish();gameplay_pure(entry);result->complete=1;return 1;
    }catch(const std::exception& error){failure(result,error.what());return -1;}
}
void gameplay_value(Obj pawn,const wchar_t* field,const HsmpGameplayValue& expected){
    require(expected.pad==0&&std::isfinite(expected.value)&&(expected.kind==1||expected.kind==2),"native gameplay exact vital type unavailable");
    const auto type=expected.kind==1?L"FloatProperty":L"DoubleProperty";
    const auto bytes=expected.kind==1?4:8;const auto schema=property(pawn,field,type,bytes);
    auto* output=static_cast<uint8_t*>(get(pawn))+schema.offset;
    if(expected.kind==1){const float value=static_cast<float>(expected.value);require(std::isfinite(value)&&static_cast<double>(value)==expected.value,"native gameplay f32 vital precision");std::memcpy(output,&value,4);}
    else std::memcpy(output,&expected.value,8);
    get(pawn);
}
void gameplay_value_readback(Obj pawn,const wchar_t* field,const HsmpGameplayValue& expected){
    if(expected.kind==1){const auto actual=read<float>(pawn,field,L"FloatProperty");const float value=static_cast<float>(expected.value);require(std::memcmp(&actual,&value,4)==0,"native gameplay f32 vital readback failed");}
    else{const auto actual=read<double>(pawn,field,L"DoubleProperty");require(std::memcmp(&actual,&expected.value,8)==0,"native gameplay f64 vital readback failed");}
}
using GameplayVector=std::array<double,3>;
using GameplayOrientation=std::array<double,4>;
void gameplay_finite(const double* values){for(uint32_t i=0;i<3;++i)require(std::isfinite(values[i]),"native gameplay state nonfinite");}
void gameplay_root_readback(const HsmpGameplayState& expected,const GameplayVector& position,const GameplayOrientation& orientation,const GameplayVector& velocity){
    const std::array<const double*,3> requested{expected.position,expected.orientation,expected.velocity};
    const std::array<const double*,3> actual{position.data(),orientation.data(),velocity.data()};
    constexpr std::array<const char*,3> fields{"position","orientation","velocity"};
    constexpr std::array<uint32_t,3> counts{3,4,3},starts{0,3,7};
    uint32_t mask{},first_field{},first_axis{};
    for(uint32_t field=0;field<3;++field)for(uint32_t axis=0;axis<counts[field];++axis)
        if(std::memcmp(requested[field]+axis,actual[field]+axis,8)!=0){if(!mask){first_field=field;first_axis=axis;}mask|=1u<<(starts[field]+axis);}
    if(!mask)return;
    const auto field=first_field,axis=first_axis;uint64_t requested_bits{},actual_bits{};
    std::memcpy(&requested_bits,requested[field]+axis,8);std::memcpy(&actual_bits,actual[field]+axis,8);
    char reason[192]{};std::snprintf(reason,sizeof(reason),
        "native gameplay exact root/velocity readback failed; field=%s axis=%u req=%.17g got=%.17g bits=%016llx/%016llx mask=%03x",
        fields[field],axis,requested[field][axis],actual[field][axis],static_cast<unsigned long long>(requested_bits),static_cast<unsigned long long>(actual_bits),mask);
    throw Error(reason);
}
void gameplay_enum_disabled(Obj pawn,const wchar_t* field){
    HsmpProp prop{};require(vt->obj_prop(get(pawn),u16(field),&prop)==1&&prop.size==1&&prop.offset>=0&&prop.offset<65536&&
        (prop.cls==name(L"ByteProperty")||prop.cls==name(L"EnumProperty")),"native gameplay auto possession schema");
    auto* byte=static_cast<uint8_t*>(get(pawn))+prop.offset;*byte=0;get(pawn);require(*byte==0,"native gameplay auto possession readback");
}
template<class T>T gameplay_raw(const void* object,const HsmpProp& prop){T value{};std::memcpy(&value,static_cast<const uint8_t*>(object)+prop.offset,sizeof(T));return value;}
bool gameplay_raw_bool(const void* object,const HsmpProp& prop){require(prop.bool_mask!=0,"native gameplay bool mask missing");return (static_cast<const uint8_t*>(object)[prop.offset+prop.bool_offset]&prop.bool_mask)!=0;}
void gameplay_state_pure(const GameplayPawn& entry,const GameplayApplied& frame){
    gameplay_native_pure(entry,frame.native);
    gameplay_pure(entry);gameplay_owner_code();gameplay_transform_code();lookup_entry_final(frame.owner_api_path);lookup_entry_final(frame.transform_api_path);lookup_entry_final(frame.root_path);lookup_entry_final(frame.movement_path);lookup_entry_final(frame.mesh_path);lookup_entry_final(frame.asset_path);
    const auto* pawn=lookup_node_get(entry.pawn_path.pinned.front(),entry.pawn_path.zero_item);
    const auto* root=lookup_node_get(frame.root_path.pinned.front(),frame.root_path.zero_item);
    const auto* movement=lookup_node_get(frame.movement_path.pinned.front(),frame.movement_path.zero_item);
    const auto* mesh=lookup_node_get(frame.mesh_path.pinned.front(),frame.mesh_path.zero_item);
    for(const auto* component:{root,movement,mesh}){void* owner{};std::memcpy(&owner,static_cast<const uint8_t*>(component)+0x90,8);
        require(reinterpret_cast<uint64_t>(owner)==entry.pawn.address,"native gameplay final original component owner changed");}
    require(frame.root_field.offset==0x1a0&&reinterpret_cast<uint64_t>(gameplay_raw<void*>(pawn,frame.root_field))==frame.root.address&&gameplay_raw<void*>(root,frame.parent)==nullptr,
        "native gameplay original root binding changed");
    EngineTransform world{};std::memcpy(&world,static_cast<const uint8_t*>(root)+0x1d0,sizeof(world));
    require(std::memcmp(world.p,frame.expected.position,24)==0&&std::memcmp(world.q,frame.expected.orientation,32)==0&&
        std::memcmp(world.scale,frame.scale.data(),24)==0,"native gameplay final exact native transform changed");
    const auto cache=gameplay_cache_pair(root);require(std::memcmp(cache.q,frame.expected.orientation,32)==0&&std::memcmp(cache.rotation,frame.expected.cache_rotation,24)==0,
        "native gameplay final original rotation cache changed");
    require(std::memcmp(static_cast<const uint8_t*>(root)+frame.position.offset,frame.expected.position,24)==0&&
        std::memcmp(static_cast<const uint8_t*>(root)+frame.rotation.offset,frame.relative_rotation.data(),24)==0&&
        std::memcmp(static_cast<const uint8_t*>(movement)+frame.velocity.offset,frame.expected.velocity,24)==0,"native gameplay final root/velocity changed");
    const auto vital=[pawn](const HsmpProp& prop,const HsmpGameplayValue& value){
        if(value.kind==1){const float f=static_cast<float>(value.value);require(std::memcmp(static_cast<const uint8_t*>(pawn)+prop.offset,&f,4)==0,"native gameplay final f32 vital changed");}
        else require(std::memcmp(static_cast<const uint8_t*>(pawn)+prop.offset,&value.value,8)==0,"native gameplay final f64 vital changed");};
    vital(frame.health,frame.expected.health);vital(frame.stamina,frame.expected.stamina);
    const auto skinned=gameplay_raw<void*>(mesh,frame.skinned),skeletal=gameplay_raw<void*>(mesh,frame.skeletal);
    require(reinterpret_cast<uint64_t>(gameplay_raw<void*>(pawn,frame.mesh_field))==frame.mesh.address&&
        reinterpret_cast<uint64_t>(skeletal?skeletal:skinned)==frame.asset.address&&gameplay_raw_bool(mesh,frame.visible)&&!gameplay_raw_bool(pawn,frame.hidden),
        "native gameplay final native body changed");
    if(entry.own){
        lookup_entry_final(frame.camera_path);lookup_entry_final(frame.hud_path);
        const auto* pc=lookup_node_get(entry.controller_path.pinned.front(),entry.controller_path.zero_item);
        const auto* camera=lookup_node_get(frame.camera_path.pinned.front(),frame.camera_path.zero_item);
        const auto* hud=lookup_node_get(frame.hud_path.pinned.front(),frame.hud_path.zero_item);
        const auto pending=gameplay_raw<void*>(camera,frame.pending_target);
        require(reinterpret_cast<uint64_t>(gameplay_raw<void*>(pc,frame.manager))==frame.proof.camera_manager.address&&
            reinterpret_cast<uint64_t>(gameplay_raw<void*>(camera,frame.pc_owner))==entry.controller.address&&
            reinterpret_cast<uint64_t>(gameplay_raw<void*>(camera,frame.view_target))==entry.pawn.address&&
            (!pending||reinterpret_cast<uint64_t>(pending)==entry.pawn.address),"native gameplay final owned view changed");
        require(reinterpret_cast<uint64_t>(gameplay_raw<void*>(pc,frame.hud_field))==frame.proof.hud.address&&gameplay_raw_bool(hud,frame.show_hud),"native gameplay final native HUD changed");
    }
    gameplay_pure(entry);
}
GameplayApplied gameplay_applied_snapshot(GameplayPawn& entry,const HsmpGameplayState& expected,const HsmpGameplayProof& proof,Obj movement,Obj original_root){
    GameplayApplied frame;frame.expected=expected;frame.proof=proof;frame.movement=movement;
    frame.root=original_root;require(frame.root.weak&&same(object_property(entry.pawn,L"RootComponent"),frame.root)&&is(frame.root,L"/Script/Engine.SceneComponent"),"native gameplay root component unavailable");
    frame.mesh=object_property(entry.pawn,L"Mesh");frame.asset=mesh_asset(frame.mesh,nullptr);
    gameplay_owner_code();Function owner(L"/Script/Engine.ActorComponent:GetOwner");owner.field(L"ReturnValue",L"ObjectProperty",8);
    frame.owner_api_path=gameplay_path(owner.function);
    for(const auto component:{frame.root,movement,frame.mesh}){
        require(is(component,L"/Script/Engine.ActorComponent"),"native gameplay owner receiver class");
        require(same(returned(component,L"/Script/Engine.ActorComponent:GetOwner"),entry.pawn),"native gameplay original component owner mismatch");}
    gameplay_transform_code();Function native_transform(L"/Script/Engine.Actor:GetTransform");native_transform.field(L"ReturnValue",L"StructProperty",96,L"Transform");frame.transform_api_path=gameplay_path(native_transform.function);
    frame.root_field=property(entry.pawn,L"RootComponent",L"ObjectProperty",8);require(frame.root_field.offset==0x1a0,"native gameplay native transform root layout");frame.parent=property(frame.root,L"AttachParent",L"ObjectProperty",8);
    frame.position=property(frame.root,L"RelativeLocation",L"StructProperty",24);frame.rotation=property(frame.root,L"RelativeRotation",L"StructProperty",24);
    require(frame.position.sub==name(L"Vector")&&frame.rotation.sub==name(L"Rotator"),"native gameplay root schema");
    frame.relative_rotation=read<GameplayVector>(frame.root,L"RelativeRotation",L"StructProperty");std::copy_n(entry.initial.scale,3,frame.scale.data());
    frame.velocity=property(movement,L"Velocity",L"StructProperty",24);
    frame.health=property(entry.pawn,L"Health",expected.health.kind==1?L"FloatProperty":L"DoubleProperty",expected.health.kind==1?4:8);
    frame.stamina=property(entry.pawn,L"Stamina",expected.stamina.kind==1?L"FloatProperty":L"DoubleProperty",expected.stamina.kind==1?4:8);
    frame.mesh_field=property(entry.pawn,L"Mesh",L"ObjectProperty",8);frame.visible=property(frame.mesh,L"bVisible",L"BoolProperty",1);frame.hidden=property(entry.pawn,L"bHidden",L"BoolProperty",1);
    frame.skinned=property(frame.mesh,L"SkinnedAsset",L"ObjectProperty",8);frame.skeletal=property(frame.mesh,L"SkeletalMesh",L"ObjectProperty",8);
    frame.root_path=gameplay_path(frame.root);frame.movement_path=gameplay_path(movement);frame.mesh_path=gameplay_path(frame.mesh);frame.asset_path=gameplay_path(frame.asset);
    if(entry.own){
        frame.manager=property(entry.controller,L"PlayerCameraManager",L"ObjectProperty",8);frame.pc_owner=property(proof.camera_manager,L"PCOwner",L"ObjectProperty",8);
        frame.view_target=property(proof.camera_manager,L"ViewTarget",L"StructProperty",0x820);frame.pending_target=property(proof.camera_manager,L"PendingViewTarget",L"StructProperty",0x820);
        require(frame.view_target.sub==name(L"TViewTarget")&&frame.pending_target.sub==name(L"TViewTarget"),"native gameplay view target schema");
        frame.hud_field=property(entry.controller,L"MyHUD",L"ObjectProperty",8);frame.show_hud=property(proof.hud,L"bShowHUD",L"BoolProperty",1);
        frame.camera_path=gameplay_path(proof.camera_manager);frame.hud_path=gameplay_path(proof.hud);
    }
    return frame;
}
uint32_t gameplay_visible_body(GameplayPawn& entry,HsmpViewResult* result){
    // Actual native mesh asset and visibility, not a marker or actor existence.
    const auto mesh=object_property(entry.pawn,L"Mesh");require(mesh.weak&&is(mesh,L"/Script/Engine.SkeletalMeshComponent"),"native gameplay body mesh unavailable");
    require(same(returned(mesh,L"/Script/Engine.ActorComponent:GetOwner",result),entry.pawn),"native gameplay body owner changed");
    Function visible(L"/Script/Engine.SceneComponent:IsVisible");visible.call(mesh,result);
    require(visible.value<uint8_t>(L"ReturnValue",L"BoolProperty")!=0&&!bool_property(entry.pawn,L"bHidden"),"native gameplay body hidden");
    const auto asset=mesh_asset(mesh,result);require(asset.weak&&is(asset,L"/Script/Engine.SkeletalMesh"),"native gameplay real body asset unavailable");return 1;
}
void gameplay_view(GameplayPawn& entry,HsmpGameplayProof& proof,HsmpViewResult* result){
    require(same(returned(entry.controller,L"/Script/Engine.Controller:K2_GetPawn",result),entry.pawn)&&same(object_property(entry.pawn,L"Controller"),entry.controller),"native gameplay possession readback failed");proof.flags|=HSMP_GAMEPLAY_POSSESSION;
    proof.view_target=returned(entry.controller,L"/Script/Engine.Controller:GetViewTarget",result);
    require(same(proof.view_target,entry.pawn),"native gameplay own native view target pending");proof.flags|=HSMP_GAMEPLAY_VIEW_TARGET;
    proof.camera_manager=object_property(entry.controller,L"PlayerCameraManager");
    require(proof.camera_manager.weak&&is(proof.camera_manager,L"/Script/Engine.PlayerCameraManager")&&same(object_property(proof.camera_manager,L"PCOwner"),entry.controller),"native gameplay own camera manager pending");
    Function location(L"/Script/Engine.PlayerCameraManager:GetCameraLocation");location.call(proof.camera_manager,result);const auto p=location.value<GameplayVector>(L"ReturnValue",L"StructProperty",L"Vector");gameplay_finite(p.data());
    Function rotation(L"/Script/Engine.PlayerCameraManager:GetCameraRotation");rotation.call(proof.camera_manager,result);const auto q=rotation.value<GameplayVector>(L"ReturnValue",L"StructProperty",L"Rotator");gameplay_finite(q.data());
    Function fov(L"/Script/Engine.PlayerCameraManager:GetFOVAngle");fov.call(proof.camera_manager,result);const auto angle=fov.value<float>(L"ReturnValue",L"FloatProperty");require(std::isfinite(angle)&&angle>0,"native gameplay own camera POV unavailable");proof.flags|=HSMP_GAMEPLAY_CAMERA;
    proof.hud=returned(entry.controller,L"/Script/Engine.PlayerController:GetHUD",result);
    require(proof.hud.weak&&is(proof.hud,L"/Script/Engine.HUD")&&bool_property(proof.hud,L"bShowHUD"),"native gameplay native HUD unavailable");proof.flags|=HSMP_GAMEPLAY_HUD;
}
int32_t gameplay_apply(uint64_t handle,const HsmpGameplayState* state,const HsmpViewGuard* guard,HsmpGameplayProof* proof,HsmpViewResult* result){
    GameplayQuatTrace diagnostic;
    const std::lock_guard lock(gameplay_mutex);
    try{initialize_result(result);thread();auto& entry=gameplay_entry(handle);OperationScope scope(guard,entry.world);GameplayWatch watch(entry);
        require(state&&proof&&entry.stage==3,"native gameplay apply stage");*proof={};gameplay_finite(state->position);for(const auto value:state->orientation)require(std::isfinite(value),"native gameplay orientation nonfinite");gameplay_finite(state->velocity);gameplay_local(entry,result);
        gameplay_layouts();
        gameplay_transform_code();const auto root=object_property(entry.pawn,L"RootComponent");require(root.weak&&is(root,L"/Script/Engine.SceneComponent"),"native gameplay native transform root unavailable");
        require(property(entry.pawn,L"RootComponent",L"ObjectProperty",8).offset==0x1a0&&!object_property(root,L"AttachParent").weak,"native gameplay native transform root layout/attachment");
        if(diagnostic.active)std::copy_n(state->orientation,4,diagnostic.requested.data());
        auto native=gameplay_native_bind(entry,root);GameplayNativeWatch native_watch(entry,native);
        const GameplayNativeCalls calls{reinterpret_cast<GameplayImport>(native.image+0x3bf5ad0),reinterpret_cast<GameplayAbsolute>(native.image+0x3bf1970),reinterpret_cast<GameplayOverlaps>(native.image+0x3bf7f40)};
        const auto original=[&](){check_guard();lookup_finish();gameplay_native_pure(entry,native);return const_cast<void*>(lookup_node_get(native.root_path.pinned.front(),native.root_path.zero_item));};
        gameplay_quat_snapshot(diagnostic,entry,root,diagnostic.before);gameplay_absolute_run(*state,calls,original);
        Function scale(L"/Script/Engine.SceneComponent:SetRelativeScale3D");scale.put(L"NewScale3D",L"StructProperty",GameplayVector{entry.initial.scale[0],entry.initial.scale[1],entry.initial.scale[2]},L"Vector");scale.call(root,result);
        original();gameplay_quat_snapshot(diagnostic,entry,root,diagnostic.after);
        const auto movement=returned(entry.pawn,L"/Script/Engine.Pawn:GetMovementComponent",result);require(movement.weak&&is(movement,L"/Script/Engine.MovementComponent"),"native gameplay movement component unavailable");
        require(same(returned(movement,L"/Script/Engine.ActorComponent:GetOwner",result),entry.pawn),"native gameplay movement owner changed");
        const auto velocity=property(movement,L"Velocity",L"StructProperty",24);require(velocity.sub==name(L"Vector"),"native gameplay velocity layout");std::memcpy(static_cast<uint8_t*>(get(movement))+velocity.offset,state->velocity,24);get(movement);
        gameplay_value(entry.pawn,L"Health",state->health);gameplay_value(entry.pawn,L"Stamina",state->stamina);
        proof->world=entry.world;proof->pawn=entry.pawn;proof->controller=entry.controller;proof->own=entry.own;
        proof->visible_meshes=gameplay_visible_body(entry,result);proof->flags|=HSMP_GAMEPLAY_VISIBLE_BODY;
        if(entry.own)gameplay_view(entry,*proof,result);
        require(same(object_property(entry.pawn,L"RootComponent"),root),"native gameplay original transform root changed");
        Function native_transform(L"/Script/Engine.Actor:GetTransform");native_transform.call(entry.pawn,result);const auto actual=native_transform.value<EngineTransform>(L"ReturnValue",L"StructProperty",L"Transform");
        require(same(object_property(entry.pawn,L"RootComponent"),root),"native gameplay original transform root changed");
        GameplayVector actual_position{};GameplayOrientation actual_orientation{};std::copy_n(actual.p,3,actual_position.data());std::copy_n(actual.q,4,actual_orientation.data());
        Function v(L"/Script/Engine.Actor:GetVelocity");v.call(entry.pawn,result);const auto actual_velocity=v.value<GameplayVector>(L"ReturnValue",L"StructProperty",L"Vector");
        gameplay_root_readback(*state,actual_position,actual_orientation,actual_velocity);require(std::memcmp(actual.scale,entry.initial.scale,24)==0,"native gameplay exact actor scale readback failed");
        gameplay_value_readback(entry.pawn,L"Health",state->health);gameplay_value_readback(entry.pawn,L"Stamina",state->stamina);
        gameplay_local(entry,result);proof->flags|=HSMP_GAMEPLAY_STATE;
        auto applied=gameplay_applied_snapshot(entry,*state,*proof,movement,root);
        applied.native=native;
        lookup_finish();gameplay_call_guard();gameplay_state_pure(entry,applied);entry.applied=std::move(applied);result->complete=1;diagnostic.complete=1;return 1;
    }catch(const std::exception& error){if(proof)*proof={};failure(result,error.what());return -1;}
}
int32_t gameplay_clear(uint64_t handle,const HsmpViewGuard* guard,HsmpViewResult* result){
    const std::lock_guard lock(gameplay_mutex);
    try{initialize_result(result);thread();auto& entry=gameplay_entry(handle);OperationScope scope(guard,entry.world);
        // Teardown starts only after the active construction/apply call unwinds.
        gameplay_local(entry,result);
        if(entry.own&&same(returned(entry.controller,L"/Script/Engine.Controller:K2_GetPawn",result),entry.pawn)){
            Function unpossess(L"/Script/Engine.Controller:UnPossess");unpossess.call(entry.controller,result);
            require(!returned(entry.controller,L"/Script/Engine.Controller:K2_GetPawn",result).weak,"native gameplay cleanup unpossess failed");}
        destroy_actor(entry.world,entry.pawn);gameplay_pawns.erase(handle);result->complete=1;return 1;
    }catch(const std::exception& error){failure(result,error.what());return -1;}
}
void gameplay_discard(uint64_t handle){const std::lock_guard lock(gameplay_mutex);gameplay_pawns.erase(handle);}
int32_t gameplay_complete(const uint64_t* handles,uint32_t count,const HsmpViewGuard* guard,HsmpGameplayProof* proofs,HsmpViewResult* result){
    const std::lock_guard lock(gameplay_mutex);
    try{initialize_result(result);thread();require(handles&&proofs&&count>0&&count<=32&&count==gameplay_pawns.size(),"native gameplay complete roster bound");
        auto& first=gameplay_entry(handles[0]);OperationScope scope(guard,first.world);GameplayWatch watch(first);uint32_t own{};
        for(uint32_t index=0;index<count;++index){require(std::find(handles,handles+index,handles[index])==handles+index,"native gameplay duplicate complete handle");
            auto& entry=gameplay_entry(handles[index]);require(entry.applied&&entry.stage==3&&same(entry.world,first.world)&&same(entry.controller,first.controller),"native gameplay complete original roster changed");own+=entry.own;}
        require(own==1,"native gameplay complete owned human ambiguous");check_guard();lookup_finish();
        // No callback, getter, wrapper factory or logger after this full-set pass.
        for(uint32_t index=0;index<count;++index){const auto& entry=gameplay_entry(handles[index]);gameplay_state_pure(entry,*entry.applied);proofs[index]=entry.applied->proof;}
        result->complete=1;return 1;
    }catch(const std::exception& error){failure(result,error.what());return -1;}
}
const HsmpGameplay gameplay_provider{3,0,gameplay_begin,gameplay_current,gameplay_construct,gameplay_finish,gameplay_apply,gameplay_clear,gameplay_discard,gameplay_complete};
