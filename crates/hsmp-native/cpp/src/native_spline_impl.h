// Included inside the provider namespace: native POD curve layouts are admitted
// through reflected metadata before reading or replacing engine-owned arrays.
using SplineMalloc=void*(*)(uint64_t,uint32_t);
using SplineFree=void(*)(void*);
using SplineChildren=void**(*)(void*);
using SplineNext=void*(*)(void*);
using SplineInt=int32_t*(*)(void*);
using SplineInner=void**(*)(void*);
using SplineStruct=void**(*)(void*);
struct SplineName {uint32_t index,number;};
struct SplineVariant {void* pointer;bool object;uint8_t pad[7];};
using SplineFieldName=SplineName*(*)(const void*,SplineName*);
using SplineFieldClass=SplineVariant*(*)(void*,SplineVariant*);
using SplineVariantName=SplineName*(*)(const SplineVariant*,SplineName*);
struct SplineApi {
    SplineMalloc allocate{};SplineFree release{};SplineChildren children{};SplineNext next{};
    SplineInner inner{};SplineStruct structure{};SplineInt size{},offset{};
    SplineFieldName field_name{};SplineFieldClass field_class{};SplineVariantName variant_name{};
};
SplineApi spline_api{};
using SplineFlags=const uint32_t*(*)(const void*);
SplineFlags spline_flags{};
void spline_live(Obj o){require(spline_flags!=nullptr,"native spline flags unavailable");const auto* flags=spline_flags(get(o));require(flags&&(*flags&0x40000000u)==0,"native spline original object garbage");get(o);}
struct NativeVectorPoint {float key;uint32_t pad;double out[3],arrive[3],leave[3];uint8_t interp,padding[7];};
struct alignas(16) NativeQuatPoint {float key;uint8_t pad[12];double out[4],arrive[4],leave[4];uint8_t interp,padding[15];};
struct NativeFloatPoint {float key,out,arrive,leave;uint8_t interp,padding[3];};
static_assert(sizeof(NativeVectorPoint)==88&&sizeof(NativeQuatPoint)==128&&sizeof(NativeFloatPoint)==20);
uint64_t spline_name(SplineName value){return static_cast<uint64_t>(value.index)|(static_cast<uint64_t>(value.number)<<32);}
void spline_api_ok() {
    require(spline_api.allocate&&spline_api.release&&spline_api.children&&spline_api.next&&spline_api.inner&&
        spline_api.structure&&spline_api.size&&spline_api.offset&&spline_api.field_name&&spline_api.field_class&&spline_api.variant_name,
        "native spline metadata/allocator unavailable");
}
void spline_inner(Obj curve,const wchar_t* point_path,int bytes) {
    spline_api_ok();spline_live(curve);void* field=*spline_api.children(get(curve));bool found{};
    for(uint32_t n=0;field;++n) {
        require(n<4096,"native spline property-chain bound");spline_live(curve);
        SplineName field_name{};spline_api.field_name(field,&field_name);get(curve);
        if(spline_name(field_name)==name(L"Points")) {
            SplineVariant variant{};SplineName cls{};spline_api.field_class(field,&variant);spline_api.variant_name(&variant,&cls);
            require(spline_name(cls)==name(L"ArrayProperty")&&*spline_api.offset(field)==0&&*spline_api.size(field)==16,"native spline Points array ABI");
            void* inner=*spline_api.inner(field);require(inner!=nullptr,"native spline array inner missing");
            spline_api.field_class(inner,&variant);spline_api.variant_name(&variant,&cls);
            require(spline_name(cls)==name(L"StructProperty")&&*spline_api.size(inner)==bytes,"native spline array inner ABI");
            const auto point=find(point_path);require(*spline_api.structure(inner)==get(point),"native spline array point identity");
            spline_live(curve);spline_live(point);found=true;break;
        }
        get(curve);field=spline_api.next(field);
    }
    require(found,"native spline Points metadata missing");
}
bool spline_layout_verified{};std::vector<Obj> spline_layout_objects;
void spline_layouts() {
    spline_api_ok();if(spline_layout_verified){for(auto o:spline_layout_objects)spline_live(o);return;}
    profile_phase("spline_layout",0);
    const size_t begin=layout_objects.size();
    layout(L"/Script/CoreUObject.InterpCurvePointVector",88,{{L"InVal",L"FloatProperty",0,4,nullptr},{L"OutVal",L"StructProperty",8,24,L"Vector"},{L"ArriveTangent",L"StructProperty",32,24,L"Vector"},{L"LeaveTangent",L"StructProperty",56,24,L"Vector"},{L"InterpMode",L"ByteProperty",80,1,nullptr}});
    layout(L"/Script/CoreUObject.InterpCurvePointQuat",128,{{L"InVal",L"FloatProperty",0,4,nullptr},{L"OutVal",L"StructProperty",16,32,L"Quat"},{L"ArriveTangent",L"StructProperty",48,32,L"Quat"},{L"LeaveTangent",L"StructProperty",80,32,L"Quat"},{L"InterpMode",L"ByteProperty",112,1,nullptr}});
    layout(L"/Script/CoreUObject.InterpCurvePointFloat",20,{{L"InVal",L"FloatProperty",0,4,nullptr},{L"OutVal",L"FloatProperty",4,4,nullptr},{L"ArriveTangent",L"FloatProperty",8,4,nullptr},{L"LeaveTangent",L"FloatProperty",12,4,nullptr},{L"InterpMode",L"ByteProperty",16,1,nullptr}});
    for(auto path:{L"/Script/CoreUObject.InterpCurveVector",L"/Script/CoreUObject.InterpCurveQuat",L"/Script/CoreUObject.InterpCurveFloat"})
        layout(path,24,{{L"Points",L"ArrayProperty",0,16,nullptr},{L"bIsLooped",L"BoolProperty",16,1,nullptr},{L"LoopKeyOffset",L"FloatProperty",20,4,nullptr}});
    layout(L"/Script/Engine.SplineCurves",112,{{L"Position",L"StructProperty",0,24,L"InterpCurveVector"},{L"Rotation",L"StructProperty",24,24,L"InterpCurveQuat"},{L"Scale",L"StructProperty",48,24,L"InterpCurveVector"},{L"ReparamTable",L"StructProperty",72,24,L"InterpCurveFloat"},{L"MetaData",L"ObjectProperty",96,8,nullptr},{L"Version",L"UInt32Property",104,4,nullptr}});
    spline_inner(find(L"/Script/CoreUObject.InterpCurveVector"),L"/Script/CoreUObject.InterpCurvePointVector",88);
    spline_inner(find(L"/Script/CoreUObject.InterpCurveQuat"),L"/Script/CoreUObject.InterpCurvePointQuat",128);
    spline_inner(find(L"/Script/CoreUObject.InterpCurveFloat"),L"/Script/CoreUObject.InterpCurvePointFloat",20);
    spline_layout_objects.assign(layout_objects.begin()+static_cast<ptrdiff_t>(begin),layout_objects.end());spline_layout_verified=true;
    profile_phase("spline_layout",1);
}
void spline_class(Obj component) {spline_live(component);const auto cls=keep(vt->class_of(get(component)));spline_live(cls);require(same(cls,find(L"/Script/Engine.SplineComponent")),"native spline exact class changed");}
thread_local Obj spline_owner{},spline_component{};
struct SplineOperation {
    Obj previous_owner{},previous_component{};
    SplineOperation(Obj owner,Obj component):previous_owner(spline_owner),previous_component(spline_component){
        if(owner.weak){spline_live(owner);spline_class(component);spline_owner=owner;spline_component=component;}
    }
    ~SplineOperation(){spline_owner=previous_owner;spline_component=previous_component;}
};
void spline_call_guard(Obj object,Obj function,Obj cls){
    if(!spline_owner.weak)return;
    spline_live(spline_owner);spline_class(spline_component);
    spline_live(object);spline_live(function);spline_live(cls);
}
struct SplineSnapshot {
    HsmpViewSplineFrame value{};
    std::vector<HsmpViewSplineVectorPoint> position,scale;std::vector<HsmpViewSplineQuatPoint> rotation;std::vector<HsmpViewSplineFloatPoint> reparam;
};
template<class T> bool spline_finite(const T* p,size_t count){for(size_t i=0;i<count;++i)if(!std::isfinite(p[i]))return false;return true;}
void spline_profile_valid(const HsmpViewSplineProfile& p) {require(p.metadata_null==1&&p.position_count<=64&&p.rotation_count<=64&&p.scale_count<=64&&p.reparam_count<=1024,"native spline profile bounds/metadata");}
template<class Native,class Wire,class Curve> void spline_read_curve(const uint8_t* base,Curve& curve,std::vector<Wire>& values,uint32_t cap) {
    Array array{};std::memcpy(&array,base,16);uint8_t looped{};std::memcpy(&looped,base+16,1);std::memcpy(&curve.loop_key_offset,base+20,4);
    require(array.count>=0&&array.capacity>=array.count&&static_cast<uint32_t>(array.count)<=cap&&(!array.count||array.data)&&looped<=1&&std::isfinite(curve.loop_key_offset),"native spline raw array bounds");
    require(!array.count||reinterpret_cast<uintptr_t>(array.data)%alignof(Native)==0,"native spline point alignment");
    values.resize(static_cast<size_t>(array.count));for(size_t i=0;i<values.size();++i){Native native{};std::memcpy(&native,static_cast<const uint8_t*>(array.data)+i*sizeof(Native),sizeof(Native));auto& out=values[i];out.key=native.key;out.interp=native.interp;
        if constexpr(std::is_same_v<Wire,HsmpViewSplineFloatPoint>){out.out=native.out;out.arrive=native.arrive;out.leave=native.leave;require(std::isfinite(out.key)&&std::isfinite(out.out)&&std::isfinite(out.arrive)&&std::isfinite(out.leave),"native spline float point nonfinite");}
        else {std::copy(std::begin(native.out),std::end(native.out),out.out);std::copy(std::begin(native.arrive),std::end(native.arrive),out.arrive);std::copy(std::begin(native.leave),std::end(native.leave),out.leave);require(std::isfinite(out.key)&&spline_finite(out.out,std::size(out.out))&&spline_finite(out.arrive,std::size(out.arrive))&&spline_finite(out.leave,std::size(out.leave)),"native spline vector point nonfinite");}
        require(out.interp<=5,"native spline interpolation mode");}
    curve.count=static_cast<uint32_t>(values.size());curve.looped=looped;curve.points=values.data();
}
SplineSnapshot spline_read(Obj owner,Obj component) {
    spline_layouts();spline_class(component);SplineSnapshot out{};
    auto p=property(component,L"SplineCurves",L"StructProperty",112);require(p.sub==name(L"SplineCurves"),"native spline component curves ABI");
    const auto* base=static_cast<const uint8_t*>(get(component))+p.offset;void* metadata{};std::memcpy(&metadata,base+96,8);require(metadata==nullptr,"native spline metadata requires replication");
    spline_read_curve<NativeVectorPoint>(base,out.value.position,out.position,64);
    spline_read_curve<NativeQuatPoint>(base+24,out.value.rotation,out.rotation,64);
    spline_read_curve<NativeVectorPoint>(base+48,out.value.scale,out.scale,64);
    spline_read_curve<NativeFloatPoint>(base+72,out.value.reparam,out.reparam,1024);std::memcpy(&out.value.version,base+104,4);get(component);
    out.value.visible=bool_property(component,L"bVisible");out.value.hidden=bool_property(component,L"bHiddenInGame");out.value.owner_hidden=bool_property(owner,L"bHidden");auto& s=out.value.settings;
    s.allow_spline_editing_per_instance=bool_property(component,L"bAllowSplineEditingPerInstance");s.reparam_steps_per_segment=read<int32_t>(component,L"ReparamStepsPerSegment",L"IntProperty");s.duration=read<float>(component,L"Duration",L"FloatProperty");
    s.stationary_endpoints=bool_property(component,L"bStationaryEndpoints");s.spline_has_been_edited=bool_property(component,L"bSplineHasBeenEdited");s.modified_by_construction_script=bool_property(component,L"bModifiedByConstructionScript");
    s.input_spline_points_to_construction_script=bool_property(component,L"bInputSplinePointsToConstructionScript");s.draw_debug=bool_property(component,L"bDrawDebug");s.closed_loop=bool_property(component,L"bClosedLoop");s.loop_position_override=bool_property(component,L"bLoopPositionOverride");s.loop_position=read<float>(component,L"LoopPosition",L"FloatProperty");
    const auto up=property(component,L"DefaultUpVector",L"StructProperty",24);require(up.sub==name(L"Vector"),"native spline up vector ABI");std::memcpy(s.default_up_vector,static_cast<const uint8_t*>(get(component))+up.offset,24);get(component);
    require(std::isfinite(s.duration)&&std::isfinite(s.loop_position)&&spline_finite(s.default_up_vector,3),"native spline settings nonfinite");return out;
}
HsmpViewSplineProfile spline_profile(const SplineSnapshot& s){return {s.value.position.count,s.value.rotation.count,s.value.scale.count,s.value.reparam.count,1};}
template<class T> bool spline_equal_points(const std::vector<T>& a,const std::vector<T>& b){return a.size()==b.size()&&(!a.size()||std::memcmp(a.data(),b.data(),a.size()*sizeof(T))==0);}
bool spline_equal(const SplineSnapshot& a,const SplineSnapshot& b) {
    auto x=a.value,y=b.value;x.position.points=y.position.points=nullptr;x.rotation.points=y.rotation.points=nullptr;x.scale.points=y.scale.points=nullptr;x.reparam.points=y.reparam.points=nullptr;
    return std::memcmp(&x,&y,sizeof(x))==0&&spline_equal_points(a.position,b.position)&&spline_equal_points(a.rotation,b.rotation)&&spline_equal_points(a.scale,b.scale)&&spline_equal_points(a.reparam,b.reparam);
}
SplineSnapshot spline_coherent(Obj world,Obj owner,Obj component,HsmpViewResult* r) {
    SplineOperation spline_scope(owner,component);
    profile_phase("qualify_first",0);spline_live(owner);spline_class(component);qualify(world,owner,component,r);profile_phase("qualify_first",1);
    profile_phase("raw_first",0);auto first=spline_read(owner,component);profile_phase("raw_first",1);
    profile_phase("qualify_second",0);spline_live(owner);spline_class(component);qualify(world,owner,component,r);profile_phase("qualify_second",1);
    profile_phase("raw_second",0);auto second=spline_read(owner,component);profile_phase("raw_second",1);
    profile_phase("qualify_final",0);spline_live(owner);spline_class(component);qualify(world,owner,component,r);profile_phase("qualify_final",1);
    require(spline_equal(first,second),"native spline changed during capture");return second;
}
template<class Curve,class Points> void spline_copy(Curve& dst,const Curve& src,const Points& points) {
    require(dst.count==src.count&&(!dst.count||dst.points),"native spline caller array profile changed");auto* pointer=dst.points;dst=src;dst.points=pointer;if(dst.count)std::copy(points.begin(),points.end(),pointer);
}
void spline_capture(Obj world,Obj owner,Obj component,const HsmpViewSplineProfile& profile,HsmpViewSplineFrame& frame,HsmpViewResult* r) {
    spline_profile_valid(profile);auto s=spline_coherent(world,owner,component,r);const auto actual=spline_profile(s);require(std::memcmp(&actual,&profile,sizeof(profile))==0,"native spline recipe counts changed");
    auto position=frame.position;auto rotation=frame.rotation;auto scale=frame.scale;auto reparam=frame.reparam;
    frame=s.value;frame.position=position;frame.rotation=rotation;frame.scale=scale;frame.reparam=reparam;
    spline_copy(frame.position,s.value.position,s.position);spline_copy(frame.rotation,s.value.rotation,s.rotation);spline_copy(frame.scale,s.value.scale,s.scale);spline_copy(frame.reparam,s.value.reparam,s.reparam);
}
void spline_frame_valid(const HsmpViewSplineProfile& profile,const HsmpViewSplineFrame& f) {
    spline_profile_valid(profile);require(f.visible<=1&&f.hidden<=1&&f.owner_hidden<=1,"native spline visibility flags");
    const auto& s=f.settings;
    for(auto value:{s.allow_spline_editing_per_instance,s.stationary_endpoints,s.spline_has_been_edited,s.modified_by_construction_script,s.input_spline_points_to_construction_script,s.draw_debug,s.closed_loop,s.loop_position_override})require(value<=1,"native spline settings boolean");
    require(std::isfinite(s.duration)&&std::isfinite(s.loop_position)&&spline_finite(s.default_up_vector,3),"native spline settings nonfinite");
    const auto validate=[](const auto& curve,uint32_t count){require(curve.count==count&&curve.looped<=1&&std::isfinite(curve.loop_key_offset)&&(!count||curve.points),"native spline frame curve profile");
        for(uint32_t i=0;i<count;++i){const auto& p=curve.points[i];require(std::isfinite(p.key)&&p.interp<=5,"native spline frame point");
            if constexpr(std::is_same_v<std::decay_t<decltype(p)>,HsmpViewSplineFloatPoint>)require(std::isfinite(p.out)&&std::isfinite(p.arrive)&&std::isfinite(p.leave),"native spline frame float nonfinite");
            else require(spline_finite(p.out,std::size(p.out))&&spline_finite(p.arrive,std::size(p.arrive))&&spline_finite(p.leave,std::size(p.leave)),"native spline frame vector nonfinite");}};
    validate(f.position,profile.position_count);validate(f.rotation,profile.rotation_count);validate(f.scale,profile.scale_count);validate(f.reparam,profile.reparam_count);
}
struct SplineOwnedArray {
    void* data{};uint32_t count{},bytes{};SplineOwnedArray()=default;SplineOwnedArray(const SplineOwnedArray&)=delete;
    ~SplineOwnedArray(){if(data)spline_api.release(data);}
};
template<class Native,class Curve> void spline_allocate(SplineOwnedArray& dst,const Curve& source) {
    dst.count=source.count;dst.bytes=source.count*static_cast<uint32_t>(sizeof(Native));if(!dst.count)return;
    dst.data=spline_api.allocate(dst.bytes,alignof(Native));require(dst.data!=nullptr,"native spline allocation failed");
    std::memset(dst.data,0,dst.bytes);for(uint32_t i=0;i<dst.count;++i){Native point{};const auto& p=source.points[i];point.key=p.key;point.interp=static_cast<uint8_t>(p.interp);
        if constexpr(std::is_same_v<Native,NativeFloatPoint>){point.out=p.out;point.arrive=p.arrive;point.leave=p.leave;}
        else {std::copy(std::begin(p.out),std::end(p.out),point.out);std::copy(std::begin(p.arrive),std::end(p.arrive),point.arrive);std::copy(std::begin(p.leave),std::end(p.leave),point.leave);}
        std::memcpy(static_cast<uint8_t*>(dst.data)+i*sizeof(Native),&point,sizeof(point));}
}
template<class T> void spline_write(Obj component,const wchar_t* key,const wchar_t* type,const T& value) {
    const auto p=property(component,key,type,static_cast<int>(sizeof(T)));std::memcpy(static_cast<uint8_t*>(get(component))+p.offset,&value,sizeof(T));get(component);
}
void spline_write_bool(Obj component,const wchar_t* key,uint32_t value) {
    require(value<=1,"native spline boolean write");auto p=property(component,key,L"BoolProperty",1);require(p.bool_mask!=0,"native spline boolean mask");
    auto* address=static_cast<uint8_t*>(get(component))+p.offset+p.bool_offset;*address=static_cast<uint8_t>((*address&~p.bool_mask)|(value?p.bool_mask:0));get(component);
}
void spline_apply(Obj world,Obj owner,Obj component,const HsmpViewSplineProfile& profile,const HsmpViewSplineFrame& f,HsmpViewResult* r) {
    spline_frame_valid(profile,f);spline_layouts();SplineOperation spline_scope(owner,component);qualify(world,owner,component,r);spline_class(component);
    spline_read(owner,component); // admits destination headers/metadata before ownership transfer
    SplineOwnedArray position,rotation,scale,reparam;
    spline_allocate<NativeVectorPoint>(position,f.position);spline_allocate<NativeQuatPoint>(rotation,f.rotation);spline_allocate<NativeVectorPoint>(scale,f.scale);spline_allocate<NativeFloatPoint>(reparam,f.reparam);
    const auto curves=property(component,L"SplineCurves",L"StructProperty",112);require(curves.sub==name(L"SplineCurves"),"native mirror spline curve ABI");
    std::array<Array,4> old{};const auto* base=static_cast<const uint8_t*>(get(component))+curves.offset;
    for(size_t i=0;i<4;++i)std::memcpy(&old[i],base+i*24,16);
    for(size_t i=0;i<old.size();++i)for(size_t j=0;j<i;++j)require(!old[i].data||old[i].data!=old[j].data,"native mirror spline array alias");
    // Validate all old/deprecated aliases before freeing any engine allocation.
    const wchar_t* deprecated_names[]={L"SplineInfo",L"SplineRotInfo",L"SplineScaleInfo",L"SplineReparamTable"};
    for(auto key:deprecated_names){auto prop=property(component,key,L"StructProperty",24);Array a{};std::memcpy(&a,static_cast<const uint8_t*>(get(component))+prop.offset,16);require(a.count>=0&&a.capacity>=a.count&&(!a.count||a.data),"native mirror deprecated spline array bounds");for(const auto& b:old)require(!a.data||a.data!=b.data,"native mirror deprecated spline array alias");}
    qualify(world,owner,component,r);spline_class(component);
    // No engine callback occurs while all four POD array headers are replaced.
    auto* destination=static_cast<uint8_t*>(get(component))+curves.offset;
    for(size_t i=0;i<old.size();++i){Array fresh{};std::memcpy(&fresh,destination+i*24,16);require(std::memcmp(&fresh,&old[i],16)==0,"native mirror spline headers changed before commit");}
    const auto commit=[&](size_t offset,SplineOwnedArray& owned,const auto& curve){Array array{owned.data,static_cast<int32_t>(owned.count),static_cast<int32_t>(owned.count)};std::memcpy(destination+offset,&array,16);const uint8_t loop=static_cast<uint8_t>(curve.looped);std::memcpy(destination+offset+16,&loop,1);std::memcpy(destination+offset+20,&curve.loop_key_offset,4);owned.data=nullptr;};
    commit(0,position,f.position);commit(24,rotation,f.rotation);commit(48,scale,f.scale);commit(72,reparam,f.reparam);std::memcpy(destination+104,&f.version,4);
    for(auto a:old)if(a.data)spline_api.release(a.data);get(component);
    const auto& s=f.settings;
    spline_write_bool(component,L"bAllowSplineEditingPerInstance",s.allow_spline_editing_per_instance);spline_write(component,L"ReparamStepsPerSegment",L"IntProperty",s.reparam_steps_per_segment);spline_write(component,L"Duration",L"FloatProperty",s.duration);
    spline_write_bool(component,L"bStationaryEndpoints",s.stationary_endpoints);spline_write_bool(component,L"bSplineHasBeenEdited",s.spline_has_been_edited);spline_write_bool(component,L"bModifiedByConstructionScript",s.modified_by_construction_script);spline_write_bool(component,L"bInputSplinePointsToConstructionScript",s.input_spline_points_to_construction_script);
    spline_write_bool(component,L"bClosedLoop",s.closed_loop);spline_write_bool(component,L"bLoopPositionOverride",s.loop_position_override);spline_write(component,L"LoopPosition",L"FloatProperty",s.loop_position);
    const auto up=property(component,L"DefaultUpVector",L"StructProperty",24);require(up.sub==name(L"Vector"),"native mirror spline up vector ABI");std::memcpy(static_cast<uint8_t*>(get(component))+up.offset,s.default_up_vector,24);get(component);
    // Matched shipping SetDrawDebug writes bDrawDebug then unconditionally calls
    // the native component notification path (VA 0x143b51810). The semantic
    // callee name/visible proxy are not inferred from the memory readback.
    // UpdateSpline would recompute original source tangents/reparam data.
    for(bool draw:{s.draw_debug==0,s.draw_debug!=0}){spline_class(component);Function fn(L"/Script/Engine.SplineComponent:SetDrawDebug");fn.boolean(L"bShow",draw);fn.call(component,r);spline_class(component);require(bool_property(component,L"bDrawDebug")==draw,"native mirror spline debug setter readback");}
    visibility(component,f.visible!=0,r);Function hidden(L"/Script/Engine.SceneComponent:SetHiddenInGame");hidden.boolean(L"NewHidden",f.hidden!=0||f.owner_hidden!=0);hidden.boolean(L"bPropagateToChildren",false);hidden.call(component,r);
    collision_off(component,r);Function tick(L"/Script/Engine.ActorComponent:SetComponentTickEnabled");tick.boolean(L"bEnabled",false);tick.call(component,r);
    Function check_tick(L"/Script/Engine.ActorComponent:IsComponentTickEnabled");check_tick.call(component,r);auto tp=check_tick.field(L"ReturnValue",L"BoolProperty",1);require((check_tick.buf[static_cast<size_t>(tp.offset+tp.bool_offset)]&tp.bool_mask)==0,"native mirror spline tick active");
    auto actual=spline_coherent(world,owner,component,r);SplineSnapshot expected{};expected.value=f;expected.value.hidden=f.hidden||f.owner_hidden;expected.value.owner_hidden=0;
    if(f.position.count)expected.position.assign(f.position.points,f.position.points+f.position.count);if(f.rotation.count)expected.rotation.assign(f.rotation.points,f.rotation.points+f.rotation.count);if(f.scale.count)expected.scale.assign(f.scale.points,f.scale.points+f.scale.count);if(f.reparam.count)expected.reparam.assign(f.reparam.points,f.reparam.points+f.reparam.count);
    require(spline_equal(actual,expected),"native mirror spline full readback mismatch");
}
