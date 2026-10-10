#pragma once
#include <atomic>
#include <cstdint>

namespace hsmp_reflect {
using SlowFind = void* (*)(void*,void*,const wchar_t*,bool);
using PathFind = void* (*)(void*,const wchar_t*,bool,bool);
using HashAvailable = bool (*)();
using FindRouteLog = void (*)(uint32_t exports,uint32_t available,uint32_t canonical,uint32_t hash_route);
struct FindRoute {
    SlowFind slow{};PathFind path{};HashAvailable available{};
    std::atomic<FindRouteLog> logger{};std::atomic<bool> reported{};
    static bool canonical_path(const wchar_t* name){
        if(!name||name[0]!=L'/')return false;
        bool object{};for(const auto* p=name;*p;++p){if(*p==L'.'||*p==L':')object=true;else if(object&&*p==L'/')return false;}
        return true;
    }
    void* find(const wchar_t* name){
        const bool exports=path&&available;
        // A slash within a member FName (the actual "Move Forward / Backward"
        // axis event) is not an admitted package-path parser grammar.
        const bool canonical=canonical_path(name);
        const bool ready=exports&&available();
        const bool hash=canonical&&ready;
        // This is the pinned StaticFindObjectByPath contract. A qualified null
        // result stays null: another search must not substitute a different key.
        void* result=hash?path(nullptr,name,false,true):slow(nullptr,nullptr,name,false);
        if(const auto sink=logger.load();sink&&!reported.exchange(true)){
            // The sink receives four copied scalars and cannot access any native
            // object, string, Rust state, caller guard or lookup cache.
            try{sink(exports?1u:0u,ready?1u:0u,canonical?1u:0u,hash?1u:0u);}catch(...){}
        }
        return result;
    }
};
}
