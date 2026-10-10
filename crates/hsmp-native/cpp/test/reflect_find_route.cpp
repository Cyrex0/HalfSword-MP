#include "ue4ss_find_route.hpp"
#include <iostream>
#include <stdexcept>
#include <string>
namespace {
int checks{},slow_calls{},path_calls{},availability_calls{},logs{};bool ready{},missing{};void* expected=reinterpret_cast<void*>(0x1234);
uint32_t last_exports{},last_ready{},last_canonical{},last_hash{};
void check(bool value,const char* reason){++checks;if(!value)throw std::runtime_error(reason);}
bool available(){++availability_calls;return ready;}
void* slow(void* cls,void* outer,const wchar_t* path,bool exact){++slow_calls;check(!cls&&!outer&&!exact,"fallback retains exact old API arguments");check(path!=nullptr,"fixture input preserved");return expected;}
void* fast(void* cls,const wchar_t* path,bool exact,bool hash){++path_calls;check(!cls&&!exact&&hash,"pinned static path API receives class=null/exact=false/useHash=true");check(path!=nullptr&&path[0]==L'/',"canonical path is passed unchanged");return missing?nullptr:expected;}
void log(uint32_t exports,uint32_t availability,uint32_t canonical,uint32_t hash){++logs;last_exports=exports;last_ready=availability;last_canonical=canonical;last_hash=hash;}
}
int main(){try{
    hsmp_reflect::FindRoute route;route.slow=slow;route.logger.store(log);ready=false;
    check(route.find(L"/Game/Asset.Asset")==expected&&slow_calls==1&&!path_calls&&!availability_calls,"missing optional API retains slow result without unavailable function calls");
    check(logs==1&&!last_exports&&!last_ready&&last_canonical&&!last_hash,"first-use marker copies actual fallback readiness");
    route.path=fast;route.available=available;
    check(route.find(L"/Script/Engine.Actor")==expected&&slow_calls==2&&availability_calls==1,"not-yet-qualified hash API falls back on each current availability check");
    ready=true;
    for(const auto* path:{L"/Script/Engine",L"/Script/Engine.Actor:GetLevel",L"/Game/Asset.Asset:ExactSubobject",L"/Game/Asset.Asset:Name With Spaces"})check(route.find(path)==expected,"package/class:function/asset:subobject canonical grammar uses qualified path API");
    check(path_calls==4&&slow_calls==2&&logs==1,"optional readiness can become available without caching false or flooding logs");
    const int old_slow=slow_calls;missing=true;check(route.find(L"/Game/Missing.Missing")==nullptr&&slow_calls==old_slow,"qualified missing object remains null without a second replacement search");missing=false;
    check(route.find(L"Class /Game/Asset.Asset")==expected&&slow_calls==old_slow+1,"unproved unquoted type-prefix grammar keeps original route");
    const int old_path=path_calls;
    check(route.find(L"/Game/Character/Blueprints/Willie_BP.Willie_BP_C:InpAxisEvt_Move Forward / Backward_K2Node_InputAxisEvent_14")==expected&&slow_calls==old_slow+2&&path_calls==old_path,"actual slash-containing axis FName retains exact original slow lookup without attempting hash grammar");
    ready=false;check(route.find(L"/Game/Asset.Asset")==expected&&slow_calls==old_slow+3,"lost current availability is never treated as a retained hash admission");
    hsmp_reflect::FindRoute first_fast;first_fast.slow=slow;first_fast.path=fast;first_fast.available=available;first_fast.logger.store(log);ready=true;first_fast.find(L"/Game/Asset.Asset");
    check(logs==2&&last_exports&&last_ready&&last_canonical&&last_hash,"first actual qualified use reports hash selection");
    std::cout<<checks<<" exact find route checks passed\n";return 0;
}catch(const std::exception& error){std::cerr<<error.what()<<'\n';return 1;}}
