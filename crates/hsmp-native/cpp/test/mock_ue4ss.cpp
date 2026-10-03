// Mock UE4SS.dll for the offline harness. Exports the symbols of abi/UE4SS.def (the harness
// fails to load main.dll if one is missing) and, for native sampling / servo, the RC::Unreal reflection
// exports cpp/src/ue4ss_reflect.cpp resolves, backed by a tiny fake engine:
//   * an FName table, a fake GUObjectArray (index + serial; serial 0 unless MOCK_SERIALS=1);
//   * classes SceneComponent > PrimitiveComponent (> SkeletalMeshComponent), Actor
//     (> Willie_BP_C, > Weapon_BP_C), the math structs Vector / Rotator / Quat / Transform;
//   * the ten UFunctions native sampling calls, with UE 5.4 param chains, and ProcessEvent
//     answering them deterministically (see `fake_*` below; the harness recomputes them);
//   * a pawn (BP variables at real offsets inside the object), a mesh, a weapon with root /
//     base / tip components: mock_world() hands their addresses to the harness.
// MOCK_SOFT=1 adds a SoftObjectProperty param to GetSocketTransform (verification must refuse).
#define WIN32_LEAN_AND_MEAN
#include <windows.h>

#include <cmath>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <cwchar>
#include <string>
#include <vector>

#include "ue4ss_abi.hpp"

namespace RC
{
    CppUserModBase::CppUserModBase() = default;
    CppUserModBase::~CppUserModBase() = default;

    // Real class: first member is `lua_State* m_lua_state`.
    auto LuaMadeSimple::Lua::get_lua_state() const -> lua_State*
    {
        return *reinterpret_cast<lua_State* const*>(this);
    }
} // namespace RC

using namespace RC::Unreal;

namespace
{
    // ---- names ----
    std::vector<std::wstring>& names()
    {
        static std::vector<std::wstring> n{L"None"};
        return n;
    }
    uint32_t name_id(const wchar_t* s, bool add)
    {
        auto& n = names();
        for (size_t i = 0; i < n.size(); ++i)
            if (_wcsicmp(n[i].c_str(), s) == 0) return static_cast<uint32_t>(i);
        if (!add) return 0;
        n.emplace_back(s);
        return static_cast<uint32_t>(n.size() - 1);
    }
    FName fn(const wchar_t* s)
    {
        return FName(s, FNAME_Add, nullptr);
    }

    // ---- fake reflection objects ----
    struct FakeStruct;
    struct FakeProp
    {
        FName name{L"None", FNAME_Add, nullptr};
        FName cls{L"None", FNAME_Add, nullptr};
        int offset = 0;
        int size = 0;
        FakeStruct* sub = nullptr; // StructProperty
        uint8_t byte_offset = 0, byte_mask = 0;
        FakeProp* next = nullptr;
    };

    // Every fake UObject starts with this header (the reflection exports cast `this` to it).
    struct FakeObj
    {
        int32_t index = -1;
        int32_t serial = 0;
        FakeObj* cls = nullptr; // the object's class (a FakeObj of kind Class)
        FName name{L"None", FNAME_Add, nullptr};
        enum Kind
        {
            Class,
            Function,
            Struct,
            Instance
        } kind = Instance;
        std::wstring path;
        bool dead = false;           // pending kill: weak pointers stop resolving
        FakeObj* super = nullptr;    // classes
        FakeProp* props = nullptr;   // classes / functions / structs
        int props_size = 0;
        alignas(16) uint8_t data[1024]{}; // instance BP variables live here
    };
    struct FakeStruct : FakeObj
    {
    };

    std::vector<FakeObj*>& objects()
    {
        static std::vector<FakeObj*> o;
        return o;
    }
    int32_t g_serial = 100;
    // No object has a serial number, as in-game (pawns, meshes, weapons and even
    // the /Script UFunctions / classes had none; UE4SS's FWeakObjectPtr allocating one crashed).
    // MOCK_SERIALS=1 gives classes / functions / structs one (the serial-checked path).
    void reg(FakeObj* o, bool serial)
    {
        static const bool serials = std::getenv("MOCK_SERIALS") != nullptr;
        o->index = static_cast<int32_t>(objects().size());
        o->serial = (serial && serials) ? ++g_serial : 0;
        objects().push_back(o);
    }

    FakeProp* prop(const wchar_t* name, const wchar_t* cls, int offset, int size, FakeStruct* sub = nullptr)
    {
        auto* p = new FakeProp;
        p->name = fn(name);
        p->cls = fn(cls);
        p->offset = offset;
        p->size = size;
        p->sub = sub;
        return p;
    }
    void chain(FakeObj* o, std::initializer_list<FakeProp*> ps, int size)
    {
        FakeProp* prev = nullptr;
        for (FakeProp* p : ps)
        {
            if (prev) prev->next = p;
            else o->props = p;
            prev = p;
        }
        o->props_size = size;
    }
    FakeObj* make(FakeObj::Kind k, const wchar_t* path, const wchar_t* short_name, FakeObj* super = nullptr)
    {
        auto* o = k == FakeObj::Struct ? new FakeStruct : new FakeObj;
        o->kind = k;
        o->path = path;
        o->name = fn(short_name);
        o->super = super;
        reg(o, k != FakeObj::Instance);
        return o;
    }

    // the world
    struct World
    {
        FakeObj *vector, *rotator, *quat, *transform;
        FakeObj *c_scene, *c_prim, *c_mesh, *c_actor, *c_willie, *c_weapon;
        FakeObj *f_socket_xf, *f_linvel, *f_angvel, *f_issim, *f_socket_loc, *f_comp_loc, *f_actor_xf, *f_actor_loc, *f_actor_rot, *f_velocity;
        FakeObj *f_set_lin = nullptr, *f_set_ang = nullptr;
        FakeObj* f_motors = nullptr;
        uint64_t motors_off = 0;
        FakeObj *mesh, *pawn, *weapon, *w_root, *w_base, *w_tip;
        FakeObj *math_cdo, *sys_cdo, *add, *frames;
        uint64_t pe_calls = 0;
    };
    World* g_world = nullptr;
    bool soft_mode()
    {
        static const bool soft = std::getenv("MOCK_SOFT") != nullptr;
        return soft;
    }

    // BP variable offsets inside FakeObj::data (+ offsetof(FakeObj, data) when reported)
    constexpr int kOffData = static_cast<int>(offsetof(FakeObj, data));

    World& world()
    {
        if (g_world) return *g_world;
        auto* w = new World;
        g_world = w;
        auto S = [](const wchar_t* path, const wchar_t* n) { return static_cast<FakeStruct*>(make(FakeObj::Struct, path, n)); };
        auto* vec = S(L"/Script/CoreUObject.Vector", L"Vector");
        chain(vec, {prop(L"X", L"DoubleProperty", 0, 8), prop(L"Y", L"DoubleProperty", 8, 8), prop(L"Z", L"DoubleProperty", 16, 8)}, 24);
        auto* rot = S(L"/Script/CoreUObject.Rotator", L"Rotator");
        chain(rot, {prop(L"Pitch", L"DoubleProperty", 0, 8), prop(L"Yaw", L"DoubleProperty", 8, 8), prop(L"Roll", L"DoubleProperty", 16, 8)}, 24);
        auto* quat = S(L"/Script/CoreUObject.Quat", L"Quat");
        chain(quat, {prop(L"X", L"DoubleProperty", 0, 8), prop(L"Y", L"DoubleProperty", 8, 8), prop(L"Z", L"DoubleProperty", 16, 8), prop(L"W", L"DoubleProperty", 24, 8)}, 32);
        auto* xf = S(L"/Script/CoreUObject.Transform", L"Transform");
        chain(xf, {prop(L"Rotation", L"StructProperty", 0, 32, quat), prop(L"Translation", L"StructProperty", 32, 24, vec), prop(L"Scale3D", L"StructProperty", 64, 24, vec)}, 96);
        w->vector = vec;
        w->rotator = rot;
        w->quat = quat;
        w->transform = xf;

        w->c_scene = make(FakeObj::Class, L"/Script/Engine.SceneComponent", L"SceneComponent");
        w->c_prim = make(FakeObj::Class, L"/Script/Engine.PrimitiveComponent", L"PrimitiveComponent", w->c_scene);
        w->c_mesh = make(FakeObj::Class, L"/Script/Engine.SkeletalMeshComponent", L"SkeletalMeshComponent", w->c_prim);
        w->c_actor = make(FakeObj::Class, L"/Script/Engine.Actor", L"Actor");
        w->c_willie = make(FakeObj::Class, L"/Game/Character/Willie_BP.Willie_BP_C", L"Willie_BP_C", w->c_actor);
        w->c_weapon = make(FakeObj::Class, L"/Game/Weapons/Weapon_BP.Weapon_BP_C", L"Weapon_BP_C", w->c_actor);

        auto F = [](const wchar_t* path, const wchar_t* n) { return make(FakeObj::Function, path, n); };
        auto V = [&](const wchar_t* n, int off) { return prop(n, L"StructProperty", off, 24, vec); };
        auto N = [](const wchar_t* n, int off) { return prop(n, L"NameProperty", off, 8); };
        w->f_socket_xf = F(L"/Script/Engine.SceneComponent:GetSocketTransform", L"GetSocketTransform");
        if (soft_mode())
            chain(w->f_socket_xf, {N(L"InSocketName", 0), prop(L"TransformSpace", L"ByteProperty", 8, 1), prop(L"Sneaky", L"SoftObjectProperty", 9, 40),
                                   prop(L"ReturnValue", L"StructProperty", 64, 96, xf)}, 160);
        else
            chain(w->f_socket_xf, {N(L"InSocketName", 0), prop(L"TransformSpace", L"ByteProperty", 8, 1), prop(L"ReturnValue", L"StructProperty", 16, 96, xf)}, 112);
        w->f_linvel = F(L"/Script/Engine.PrimitiveComponent:GetPhysicsLinearVelocityAtPoint", L"GetPhysicsLinearVelocityAtPoint");
        chain(w->f_linvel, {V(L"Point", 0), N(L"BoneName", 24), V(L"ReturnValue", 32)}, 56);
        w->f_angvel = F(L"/Script/Engine.PrimitiveComponent:GetPhysicsAngularVelocityInDegrees", L"GetPhysicsAngularVelocityInDegrees");
        chain(w->f_angvel, {N(L"BoneName", 0), V(L"ReturnValue", 8)}, 32);
        w->f_issim = F(L"/Script/Engine.PrimitiveComponent:IsSimulatingPhysics", L"IsSimulatingPhysics");
        chain(w->f_issim, {N(L"BoneName", 0), prop(L"ReturnValue", L"BoolProperty", 8, 1)}, 16);
        w->f_socket_loc = F(L"/Script/Engine.SceneComponent:GetSocketLocation", L"GetSocketLocation");
        chain(w->f_socket_loc, {N(L"InSocketName", 0), V(L"ReturnValue", 8)}, 32);
        w->f_comp_loc = F(L"/Script/Engine.SceneComponent:K2_GetComponentLocation", L"K2_GetComponentLocation");
        chain(w->f_comp_loc, {V(L"ReturnValue", 0)}, 24);
        w->f_actor_xf = F(L"/Script/Engine.Actor:GetTransform", L"GetTransform");
        chain(w->f_actor_xf, {prop(L"ReturnValue", L"StructProperty", 0, 96, xf)}, 96);
        w->f_actor_loc = F(L"/Script/Engine.Actor:K2_GetActorLocation", L"K2_GetActorLocation");
        chain(w->f_actor_loc, {V(L"ReturnValue", 0)}, 24);
        w->f_actor_rot = F(L"/Script/Engine.Actor:K2_GetActorRotation", L"K2_GetActorRotation");
        chain(w->f_actor_rot, {prop(L"ReturnValue", L"StructProperty", 0, 24, rot)}, 24);
        w->f_velocity = F(L"/Script/Engine.Actor:GetVelocity", L"GetVelocity");
        chain(w->f_velocity, {V(L"ReturnValue", 0)}, 24);
        w->f_set_lin = F(L"/Script/Engine.PrimitiveComponent:SetPhysicsLinearVelocity", L"SetPhysicsLinearVelocity");
        chain(w->f_set_lin, {V(L"NewVel", 0), prop(L"bAddToCurrent", L"BoolProperty", 24, 1), N(L"BoneName", 32)}, 40);
        w->f_set_ang = F(L"/Script/Engine.PrimitiveComponent:SetPhysicsAngularVelocityInDegrees", L"SetPhysicsAngularVelocityInDegrees");
        chain(w->f_set_ang, {V(L"NewAngVel", 0), prop(L"bAddToCurrent", L"BoolProperty", 24, 1), N(L"BoneName", 32)}, 40);
        w->f_motors = F(L"/Script/Engine.SkeletalMeshComponent:SetAllMotorsAngularDriveParams", L"SetAllMotorsAngularDriveParams");
        chain(w->f_motors, {prop(L"InSpring", L"FloatProperty", 0, 4), prop(L"InDamping", L"FloatProperty", 4, 4), prop(L"InForceLimit", L"FloatProperty", 8, 4),
                            prop(L"bSkipCustomPhysicsType", L"BoolProperty", 12, 1)}, 16);

        // the pawn class's BP variables: Willie_BP_C (offsets into FakeObj::data)
        {
            std::vector<FakeProp*> ps;
            const wchar_t* flags[] = {L"R_Guarding", L"L_Guarding", L"Any_Guarding"};
            for (int i = 0; i < 3; ++i)
            {
                FakeProp* p = prop(flags[i], L"BoolProperty", kOffData + 0, 1);
                p->byte_offset = 0;
                p->byte_mask = static_cast<uint8_t>(1u << i); // bitfield bools in one byte
                ps.push_back(p);
            }
            ps.push_back(prop(L"R_GripType_Current", L"ByteProperty", kOffData + 8, 1));
            ps.push_back(prop(L"L_GripType_Current", L"IntProperty", kOffData + 12, 4));
            ps.push_back(prop(L"All Body Tonus", L"FloatProperty", kOffData + 16, 4));
            ps.push_back(prop(L"Stamina", L"DoubleProperty", kOffData + 24, 8));
            ps.push_back(prop(L"Aim Vector", L"StructProperty", kOffData + 32, 24, vec));
            ps.push_back(prop(L"Current Control Rotation", L"StructProperty", kOffData + 56, 24, rot));
            ps.push_back(prop(L"R Out End Pos", L"StructProperty", kOffData + 80, 24, vec));
            ps.push_back(prop(L"Weapon R", L"ObjectProperty", kOffData + 104, 8));
            ps.push_back(prop(L"Soft Thing", L"SoftObjectProperty", kOffData + 112, 40));
            for (int i = 0; i < 26; ++i)   // neutralise: BP floats like "Pain Head", "Arm R Tonus"
            {
                wchar_t nm[16];
                swprintf(nm, 16, L"Neut%02d", i);
                ps.push_back(prop(nm, L"FloatProperty", kOffData + 200 + 4 * i, 4));
            }
            for (size_t i = 1; i < ps.size(); ++i) ps[i - 1]->next = ps[i];
            w->c_willie->props = ps[0];
        }
        // the weapon class: RootComponent (Actor), "Root Scene", TippyTipScene
        {
            FakeProp* r = prop(L"RootComponent", L"ObjectProperty", kOffData + 0, 8);
            FakeProp* b = prop(L"Root Scene", L"ObjectProperty", kOffData + 8, 8);
            FakeProp* t = prop(L"TippyTipScene", L"ObjectProperty", kOffData + 16, 8);
            r->next = b;
            b->next = t;
            w->c_weapon->props = r;
        }

        auto I = [](FakeObj* cls, const wchar_t* n) {
            FakeObj* o = make(FakeObj::Instance, L"", n);
            o->cls = cls;
            return o;
        };
        w->mesh = I(w->c_mesh, L"CharacterMesh0");
        w->pawn = I(w->c_willie, L"Willie_BP_C_0");
        w->weapon = I(w->c_weapon, L"Weapon_BP_C_0");
        w->w_root = I(w->c_prim, L"WeaponMesh");
        w->w_base = I(w->c_scene, L"Root Scene");
        w->w_tip = I(w->c_scene, L"TippyTipScene");
        // pawn BP values
        uint8_t* d = w->pawn->data;
        d[0] = 0b101; // R_Guarding, Any_Guarding
        d[8] = 3;     // R grip
        int32_t lg = 2;
        std::memcpy(d + 12, &lg, 4);
        float tonus = 0.75f;
        std::memcpy(d + 16, &tonus, 4);
        double stamina = 42.5;
        std::memcpy(d + 24, &stamina, 8);
        double aim[3] = {1, 0, 0};
        std::memcpy(d + 32, aim, 24);
        double ctl[3] = {10, 20, 30};
        std::memcpy(d + 56, ctl, 24);
        double ik[3] = {5, 6, 7};
        std::memcpy(d + 80, ik, 24);
        FakeObj* wp = w->weapon;
        std::memcpy(d + 104, &wp, 8);
        // weapon components
        FakeObj* comps[3] = {w->w_root, w->w_base, w->w_tip};
        std::memcpy(w->weapon->data, comps, 24);

        w->math_cdo = make(FakeObj::Instance, L"/Script/Engine.Default__KismetMathLibrary", L"Default__KismetMathLibrary");
        w->sys_cdo = make(FakeObj::Instance, L"/Script/Engine.Default__KismetSystemLibrary", L"Default__KismetSystemLibrary");
        w->add = make(FakeObj::Function, L"/Script/Engine.KismetMathLibrary:Add_IntInt", L"Add_IntInt");
        w->frames = make(FakeObj::Function, L"/Script/Engine.KismetSystemLibrary:GetFrameCount", L"GetFrameCount");
        return *w;
    }

    FakeObj* O(const void* p)
    {
        return const_cast<FakeObj*>(static_cast<const FakeObj*>(p));
    }

    void put3(uint8_t* p, double x, double y, double z)
    {
        double v[3] = {x, y, z};
        std::memcpy(p, v, 24);
    }
    void get3(const uint8_t* p, double* v)
    {
        std::memcpy(v, p, 24);
    }
    int bone_of(const uint8_t* fname)
    {
        uint32_t id;
        std::memcpy(&id, fname, 4);
        return static_cast<int>(id);
    }
} // namespace

// ---- exports: the engine-ish surface -----------------------------------------------------------
namespace RC::Unreal
{
    FName::FName(const wchar_t* name, EFindName find_type, void*)
    {
        ComparisonIndex = name_id(name ? name : L"None", find_type == FNAME_Add);
        Number = 0;
    }

    auto FFieldClassVariant::GetFName() const -> FName
    {
        auto* cls = static_cast<FName*>(Container);
        return *cls;
    }

    namespace UObjectGlobals
    {
        auto StaticFindObject_InternalSlow(UClass*, UObject*, const wchar_t* name, bool) -> UObject*
        {
            world();
            for (FakeObj* o : objects())
            {
                if (name && !o->path.empty() && wcscmp(name, o->path.c_str()) == 0) return reinterpret_cast<UObject*>(o);
            }
            return nullptr;
        }
    } // namespace UObjectGlobals

    auto UObjectBase::IsA(UClass* cls) const -> bool
    {
        for (FakeObj* c = O(this)->cls; c; c = c->super)
            if (c == O(cls)) return true;
        return false;
    }
    auto UObjectBase::GetClassPrivate() const -> const UClass*&
    {
        return *static_cast<const UClass**>(static_cast<void*>(&O(this)->cls));
    }
    auto UObjectBase::GetNamePrivate() const -> const FName&
    {
        return O(this)->name;
    }

    auto UStruct::GetChildProperties() -> FField*&
    {
        return reinterpret_cast<FField*&>(O(this)->props);
    }
    auto UStruct::GetPropertiesSize() -> int&
    {
        return O(this)->props_size;
    }

    auto FField::GetNextFieldAsProperty() -> FProperty*
    {
        return reinterpret_cast<FProperty*>(reinterpret_cast<FakeProp*>(this)->next);
    }
    auto FField::GetFName() const -> FName
    {
        return reinterpret_cast<const FakeProp*>(this)->name;
    }
    auto FField::GetClass() -> FFieldClassVariant
    {
        FFieldClassVariant v{};
        v.Container = &reinterpret_cast<FakeProp*>(this)->cls;
        v.bIsUObject = false;
        return v;
    }
    auto FProperty::GetOffset_Internal() -> int&
    {
        return reinterpret_cast<FakeProp*>(this)->offset;
    }
    auto FProperty::GetElementSize() -> int&
    {
        return reinterpret_cast<FakeProp*>(this)->size;
    }
    auto FStructProperty::GetStruct() -> TObjectPtr<UScriptStruct>&
    {
        return reinterpret_cast<TObjectPtr<UScriptStruct>&>(reinterpret_cast<FakeProp*>(this)->sub);
    }
    auto FBoolProperty::GetByteOffset() -> uint8_t&
    {
        return reinterpret_cast<FakeProp*>(this)->byte_offset;
    }
    auto FBoolProperty::GetByteMask() -> uint8_t&
    {
        return reinterpret_cast<FakeProp*>(this)->byte_mask;
    }

    // The object array: an "item" is the address of the object's slot in objects().
    auto UObjectBase::GetInternalIndex_Private() const -> const int&
    {
        return O(this)->index;
    }
    auto FUObjectArray::IndexToObject(int index) -> FUObjectItem*
    {
        if (index < 0 || index >= static_cast<int>(objects().size())) return nullptr;
        return reinterpret_cast<FUObjectItem*>(&objects()[static_cast<size_t>(index)]);
    }
    auto FUObjectItem::GetObject() -> UObjectBase*&
    {
        return *reinterpret_cast<UObjectBase**>(this);
    }
    auto FUObjectItem::GetSerialNumber() -> int&
    {
        return (*reinterpret_cast<FakeObj**>(this))->serial;
    }
    auto FUObjectItem::IsValid(bool even_if_pending_kill) const -> bool
    {
        const FakeObj* o = *reinterpret_cast<FakeObj* const*>(this);
        return o && (even_if_pending_kill || !o->dead);
    }

    auto UObject::GetPropertyByNameInChain(const wchar_t* name) -> FProperty*
    {
        uint32_t id = name_id(name, false);
        if (id == 0) return nullptr;
        for (FakeObj* c = O(this)->cls; c; c = c->super)
            for (FakeProp* p = c->props; p; p = p->next)
                if (p->name.ComparisonIndex == id) return reinterpret_cast<FProperty*>(p);
        return nullptr;
    }

    // Deterministic fake physics (the harness recomputes the same values):
    //   bone socket of FName id n: pos (n*10, n+1, -n), rot quat (0, 0, 0, 1)
    //   linear velocity at a point of bone n: (point.x + 1, n, 2); angular: (n, 0.5, -1)
    //   the weapon actor: at the hand_r socket + (10, 0, 0); root simulates
    //   components: base (1, 2, 3), tip (4, 5, 6); actor loc (100, 200, 300), rot (1, 2, 3), vel (7, 8, 9)
    auto UObject::ProcessEvent(UFunction* function, void* params) -> void
    {
        World& w = world();
        FakeObj* fo = O(function);
        auto* self = O(this);
        auto* p = static_cast<uint8_t*>(params);
        ++w.pe_calls;
        if (fo == w.add && self == w.math_cdo)
        {
            auto* q = static_cast<int32_t*>(params);
            q[2] = q[0] + q[1];
        }
        else if (fo == w.frames && self == w.sys_cdo)
        {
            static int64_t frame = 123456; // advances like GFrameCounter
            *static_cast<int64_t*>(params) = frame++;
        }
        else if (fo == w.f_socket_xf)
        {
            int n = bone_of(p);
            int ret = soft_mode() ? 64 : 16;
            double q[4] = {0, 0, 0, 1};
            std::memcpy(p + ret, q, 32);
            put3(p + ret + 32, n * 10.0, n + 1.0, -n * 1.0);
            put3(p + ret + 64, 1, 1, 1);
        }
        else if (fo == w.f_linvel)
        {
            double pt[3];
            get3(p, pt);
            put3(p + 32, pt[0] + 1, bone_of(p + 24), 2);
        }
        else if (fo == w.f_angvel)
        {
            put3(p + 8, bone_of(p), 0.5, -1);
        }
        else if (fo == w.f_issim)
        {
            p[8] = 1;
        }
        else if (fo == w.f_socket_loc)
        {
            int n = bone_of(p);
            put3(p + 8, n * 10.0, n + 1.0, -n * 1.0);
        }
        else if (fo == w.f_comp_loc)
        {
            if (self == w.w_base) put3(p, 1, 2, 3);
            else put3(p, 4, 5, 6);
        }
        else if (fo == w.f_actor_xf)
        {
            int n = static_cast<int>(name_id(L"hand_r", true));
            double q[4] = {0, 0, 0.6, 0.8};
            std::memcpy(p, q, 32);
            put3(p + 32, n * 10.0 + 10, n + 1.0, -n * 1.0);
            put3(p + 64, 1, 1, 1);
        }
        else if (fo == w.f_actor_loc)
            put3(p, 100, 200, 300);
        else if (fo == w.f_actor_rot)
            put3(p, 1, 2, 3);
        else if (fo == w.f_velocity)
            put3(p, 7, 8, 9);
        else if (fo == w.f_motors)
        {
            for (int k = 0; k < 13; ++k)
                if (p[k] != 0) RaiseException(0xE0000003, 0, 0, nullptr); // motors off = 0, 0, 0, false
            ++w.motors_off;
        }
        else if (fo == w.f_set_lin || fo == w.f_set_ang)
        {
            double v[3];
            get3(p, v);
            if (!(std::isfinite(v[0]) && std::isfinite(v[1]) && std::isfinite(v[2])) || p[24] != 0)
                RaiseException(0xE0000002, 0, 0, nullptr); // a bad velocity set: crash loudly
        }
        else
            RaiseException(0xE0000001, 0, 0, nullptr); // an unknown function: crash loudly
    }
} // namespace RC::Unreal

// ---- harness helpers (not UE4SS exports) -------------------------------------------------------
extern "C" __declspec(dllexport) void mock_world(uint64_t out[6])
{
    World& w = world();
    out[0] = reinterpret_cast<uint64_t>(w.mesh);
    out[1] = reinterpret_cast<uint64_t>(w.pawn);
    out[2] = reinterpret_cast<uint64_t>(w.weapon);
    out[3] = static_cast<uint64_t>(name_id(L"hand_r", true));
    out[4] = w.pe_calls;
    out[5] = reinterpret_cast<uint64_t>(w.w_root);
}

// The FName id of a name (adds it), for the harness's expected values.
extern "C" __declspec(dllexport) uint32_t mock_fname(const wchar_t* s)
{
    return name_id(s, true);
}

// Mark an object pending kill (FUObjectItem::IsValid(false) is false, as in the engine).
extern "C" __declspec(dllexport) void mock_kill(uint64_t obj, int dead)
{
    reinterpret_cast<FakeObj*>(obj)->dead = dead != 0;
}
