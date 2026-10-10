// Included in the private provider namespace. No initializer query drives time,
// executes latent actions, refreshes an original identity, or stores completion.
constexpr std::array<GameplayCodePin,5> gameplay_initialization_pins{{
    {0x39e6370,28,0xb05c21d8bd2ad52eULL},{0x3717fd0,91,0x9ceeb57e10d0c85dULL},
    {0x2acdc00,157,0x2ca2b9f230544454ULL},{0x13a91c0,60,0x32aa1b029c0bf899ULL},
    {0x14dea40,85,0x19949f7da597ec87ULL}}};
using GameplayInitializationCount=int32_t(*)(const void*,uint64_t);
struct GameplayInitializationStorage {
    std::array<uint8_t,0x50> outer{},inner{};
    struct Row {int32_t index{};std::array<uint8_t,32> bytes{};};
    std::vector<Row> chain;int32_t bucket{-1},pending{};const void* list{};
};
template<class T>T gameplay_initialization_scalar(const std::array<uint8_t,0x50>& bytes,size_t at){T value{};std::memcpy(&value,bytes.data()+at,sizeof(value));return value;}
template<class Readable>void gameplay_initialization_header(const void* pointer,std::array<uint8_t,0x50>& bytes,size_t stride,Readable readable){
    require(pointer&&reinterpret_cast<uintptr_t>(pointer)%8==0&&readable(pointer,bytes.size()),"native gameplay latent header unavailable");std::memcpy(bytes.data(),pointer,bytes.size());
    const auto data=gameplay_initialization_scalar<uintptr_t>(bytes,0);
    const auto num=gameplay_initialization_scalar<int32_t>(bytes,8),maximum=gameplay_initialization_scalar<int32_t>(bytes,12),free=gameplay_initialization_scalar<int32_t>(bytes,0x34);
    require(num>=0&&maximum>=num&&maximum<=4096&&free>=0&&free<=num&&((maximum>0)==(data!=0)),"native gameplay latent header bounds");
    require(!maximum||(data%8==0&&readable(reinterpret_cast<const void*>(data),static_cast<size_t>(maximum)*stride)),"native gameplay latent storage unavailable");
}
template<class Readable>GameplayInitializationStorage gameplay_initialization_storage(const void* manager,uint64_t weak,Readable readable){
    require(weak&&static_cast<int32_t>(weak)>=0&&static_cast<int32_t>(weak>>32)>0,"native gameplay latent positive callback identity required");
    GameplayInitializationStorage snapshot;gameplay_initialization_header(manager,snapshot.outer,32,readable);
    const auto num=gameplay_initialization_scalar<int32_t>(snapshot.outer,8),free=gameplay_initialization_scalar<int32_t>(snapshot.outer,0x34);
    if(num==free)return snapshot;
    const auto hash_size=gameplay_initialization_scalar<int32_t>(snapshot.outer,0x48);
    const auto allocated=gameplay_initialization_scalar<uintptr_t>(snapshot.outer,0x40);
    require(hash_size>0&&hash_size<=8192&&(static_cast<uint32_t>(hash_size)&(static_cast<uint32_t>(hash_size)-1))==0&&(allocated||hash_size<=2),"native gameplay latent hash geometry");
    const auto buckets=allocated?allocated:reinterpret_cast<uintptr_t>(manager)+0x38;
    require(buckets%4==0&&readable(reinterpret_cast<const void*>(buckets),static_cast<size_t>(hash_size)*4),"native gameplay latent hash unavailable");
    const uint32_t hash=static_cast<uint32_t>(weak)^static_cast<uint32_t>(weak>>32);
    std::memcpy(&snapshot.bucket,reinterpret_cast<const uint8_t*>(buckets)+(hash&(static_cast<uint32_t>(hash_size)-1))*4,4);
    auto index=snapshot.bucket;const auto data=gameplay_initialization_scalar<uintptr_t>(snapshot.outer,0);
    while(index!=-1){require(index>=0&&index<num&&snapshot.chain.size()<static_cast<size_t>(num),"native gameplay latent hash chain bounds");
        for(const auto& old:snapshot.chain)require(old.index!=index,"native gameplay latent hash cycle");
        GameplayInitializationStorage::Row row;row.index=index;std::memcpy(row.bytes.data(),reinterpret_cast<const uint8_t*>(data)+static_cast<size_t>(index)*32,32);snapshot.chain.push_back(row);
        uint64_t key{};std::memcpy(&key,row.bytes.data(),8);
        if(key==weak){std::memcpy(&snapshot.list,row.bytes.data()+8,8);if(snapshot.list){gameplay_initialization_header(snapshot.list,snapshot.inner,24,readable);
                snapshot.pending=gameplay_initialization_scalar<int32_t>(snapshot.inner,8)-gameplay_initialization_scalar<int32_t>(snapshot.inner,0x34);}return snapshot;}
        std::memcpy(&index,row.bytes.data()+0x18,4);
    }
    return snapshot;
}
bool gameplay_initialization_storage_same(const GameplayInitializationStorage& a,const GameplayInitializationStorage& b){
    if(a.outer!=b.outer||a.inner!=b.inner||a.bucket!=b.bucket||a.pending!=b.pending||a.list!=b.list||a.chain.size()!=b.chain.size())return false;
    for(size_t i=0;i<a.chain.size();++i)if(a.chain[i].index!=b.chain[i].index||a.chain[i].bytes!=b.chain[i].bytes)return false;return true;
}
template<class Readable>int32_t gameplay_initialization_query(const void* manager,uint64_t weak,GameplayInitializationCount count,Readable readable,bool initial=false){
    require(count,"native gameplay latent count unavailable");const auto before=gameplay_initialization_storage(manager,weak,readable);
    const auto pending=count(manager,weak);const auto after=gameplay_initialization_storage(manager,weak,readable);
    require(pending>=0&&pending==before.pending&&gameplay_initialization_storage_same(before,after),"native gameplay latent storage changed during query");
    require(!initial||pending>0,"native gameplay initial pending actions not observed");return pending;
}
void gameplay_initialization_original(const GameplayPawn& entry,const GameplayInitialization& original){
    gameplay_pure(entry);lookup_entry_final(original.pawn_path);lookup_entry_final(original.instance_path);
    require(active_lookup&&original.instance.address==active_lookup->gi.address&&
        (same(original.instance,active_lookup->gi)||mesh_serial_assignment(original.instance,active_lookup->gi))&&
        original.world_instance.offset==0x1d8&&original.world_instance.size==8,"native gameplay latent original instance changed");
    auto* pawn=lookup_node_get(original.pawn_path.pinned.front(),original.pawn_path.zero_item);
    require(reinterpret_cast<uint64_t>(pawn)==entry.pawn.address&&vt->weak(pawn)==original.callback_weak&&reinterpret_cast<uint64_t>(vt->resolve(original.callback_weak))==entry.pawn.address,"native gameplay latent callback identity changed");
    const auto* instance=lookup_node_get(original.instance_path.pinned.front(),original.instance_path.zero_item);const auto* world=lookup_node_get(entry.world_path.pinned.front(),entry.world_path.zero_item);
    void* current_instance{};std::memcpy(&current_instance,static_cast<const uint8_t*>(world)+0x1d8,8);
    require(reinterpret_cast<uint64_t>(current_instance)==original.instance.address&&gameplay_quat_readable(instance,0x100),"native gameplay latent instance link changed");
    const void* manager{};std::memcpy(&manager,static_cast<const uint8_t*>(instance)+0xf8,8);require(manager&&manager==original.manager,"native gameplay latent manager changed");
    gameplay_code_validate_at(original.code,gameplay_code_module());
}
int32_t gameplay_initialization_count_original(const GameplayPawn& entry,const GameplayInitialization& original,bool initial=false){
    gameplay_initialization_original(entry,original);const auto image=original.code.image;
    using Manager=void*(*)(const void*);using Valid=uint8_t(*)(const uint64_t*);
    require(reinterpret_cast<Manager>(image+0x39e6370)(reinterpret_cast<const void*>(entry.world.address))==original.manager&&reinterpret_cast<Valid>(image+0x14dea40)(&original.callback_weak)!=0,"native gameplay latent native identity changed");
    const auto pending=gameplay_initialization_query(original.manager,original.callback_weak,reinterpret_cast<GameplayInitializationCount>(image+0x3717fd0),gameplay_quat_readable,initial);
    gameplay_initialization_original(entry,original);return pending;
}
void gameplay_initialization_capture(GameplayPawn& entry){
    require(active_lookup&&!entry.initialization,"native gameplay latent capture order");GameplayInitialization original;original.instance=active_lookup->gi;
    original.world_instance=property(entry.world,L"OwningGameInstance",L"ObjectProperty",8);require(original.world_instance.offset==0x1d8,"native gameplay latent world schema");
    original.instance_path=gameplay_path(original.instance);original.pawn_path=entry.pawn_path;lookup_pin(original.pawn_path);
    original.callback_weak=original.pawn_path.pinned.front().weak;
    require(static_cast<int32_t>(original.callback_weak>>32)>0,"native gameplay latent positive callback identity required");
    const auto* instance=lookup_node_get(original.instance_path.pinned.front(),original.instance_path.zero_item);require(gameplay_quat_readable(instance,0x100),"native gameplay latent instance storage unavailable");
    std::memcpy(&original.manager,static_cast<const uint8_t*>(instance)+0xf8,8);
    const auto image=gameplay_code_module();original.code=gameplay_code_copy(image,gameplay_initialization_pins);
    gameplay_initialization_count_original(entry,original,true);entry.initialization=std::move(original);
}
uint32_t gameplay_initialization_count(const GameplayPawn& entry){
    require(entry.initialization.has_value(),"native gameplay original initializer missing");return static_cast<uint32_t>(gameplay_initialization_count_original(entry,*entry.initialization));
}
int32_t gameplay_initialized(uint64_t handle,const HsmpViewGuard* guard,uint32_t* pending,HsmpViewResult* result){
    try{initialize_result(result);thread();require(!active_lookup&&!gameplay_active,"native gameplay initializer callback reentry");
        const std::lock_guard lock(gameplay_mutex);auto& entry=gameplay_entry(handle);OperationScope scope(guard,entry.world);GameplayWatch watch(entry);
        require(pending&&entry.stage==2,"native gameplay initializer stage");gameplay_tick_disabled(entry.pawn,result);
        const auto count=gameplay_initialization_count(entry);lookup_finish();gameplay_pure(entry);
        require(gameplay_initialization_count(entry)==count,"native gameplay latent count changed before return");*pending=count;result->complete=1;return 1;
    }catch(const std::exception& error){failure(result,error.what());return -1;}
}
