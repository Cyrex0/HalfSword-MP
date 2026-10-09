// Game-thread native scene provider. No engine pointer enters a network DTO.
#pragma once
#include <cstdint>
#include "ue4ss_reflect.h"
extern "C" {
struct HsmpViewText { const uint16_t* data; uint32_t len; uint32_t pad; };
struct HsmpViewObject { uint64_t weak; uint64_t address; };
// Borrowed for this operation only. check must not acquire the native API mutex.
// A zero result stops the operation before any further UObject access.
struct HsmpViewGuard { void* context; int32_t (*check)(void*); };
struct HsmpViewTransform { double p[3]; double q[4]; double scale[3]; };
struct HsmpViewParameter { HsmpViewText name; uint32_t association; int32_t index; };
struct HsmpViewMaterial {
    uint32_t slot; uint32_t pad; HsmpViewText base;
    const HsmpViewParameter* scalars; uint32_t scalar_count; uint32_t pad_s;
    const HsmpViewParameter* vectors; uint32_t vector_count; uint32_t pad_v;
    const HsmpViewParameter* textures; uint32_t texture_count; uint32_t pad_t;
};
struct HsmpViewVertexLod { uint32_t lod; uint32_t count; const uint8_t* rgba; uint32_t bytes; uint32_t pad; };
// ABI7: counts are actual source counts, not capacities/default curve recipes.
struct HsmpViewSplineProfile {
    uint32_t position_count,rotation_count,scale_count,reparam_count,metadata_null;
};
// ABI8: complete original static-component census, not an asset color guess.
// no_override0 means validated overrides exist; 1 means every actual slot null.
// ABI11: asset_present is the admitted original hard-link fact, never a Lua
// wrapper guess. component_kind0 skeletal/1 static; complete material count<=32,
// null-mask bits name actual null override slots. Skeletal is absence-only.
struct HsmpViewVertexState { uint32_t lod_info_count,no_override,asset_present,material_count,material_null_mask,component_kind; };
struct HsmpViewSpringArmFrame {double translation[3],rotation[4];};
struct HsmpViewFinishTarget {
    HsmpViewObject owner,component,asset;uint32_t scene_kind,owned_mirror;
    HsmpViewText socket;HsmpViewSpringArmFrame arm;
};
struct HsmpViewSplineVectorPoint { float key; uint32_t interp; double out[3],arrive[3],leave[3]; };
struct HsmpViewSplineQuatPoint { float key; uint32_t interp; double out[4],arrive[4],leave[4]; };
struct HsmpViewSplineFloatPoint { float key,out,arrive,leave; uint32_t interp; };
struct HsmpViewSplineSettings {
    uint32_t allow_spline_editing_per_instance; int32_t reparam_steps_per_segment; float duration;
    uint32_t stationary_endpoints,spline_has_been_edited,modified_by_construction_script;
    uint32_t input_spline_points_to_construction_script,draw_debug,closed_loop,loop_position_override;
    float loop_position; uint32_t pad; double default_up_vector[3];
};
struct HsmpViewSplineVectorCurve { HsmpViewSplineVectorPoint* points; uint32_t count,looped; float loop_key_offset; uint32_t pad; };
struct HsmpViewSplineQuatCurve { HsmpViewSplineQuatPoint* points; uint32_t count,looped; float loop_key_offset; uint32_t pad; };
struct HsmpViewSplineFloatCurve { HsmpViewSplineFloatPoint* points; uint32_t count,looped; float loop_key_offset; uint32_t pad; };
struct HsmpViewSplineFrame {
    uint32_t visible,hidden,owner_hidden,version; HsmpViewSplineSettings settings;
    HsmpViewSplineVectorCurve position; HsmpViewSplineQuatCurve rotation;
    HsmpViewSplineVectorCurve scale; HsmpViewSplineFloatCurve reparam;
};
struct HsmpViewComponent {
    uint32_t id; uint32_t parent; uint32_t kind; uint32_t visible;
    HsmpViewText asset; HsmpViewText skeleton; HsmpViewText socket;
    HsmpViewTransform relative;
    const HsmpViewText* bones; uint32_t bone_count; uint32_t pad_b;
    const HsmpViewText* morphs; uint32_t morph_count; uint32_t pad_m;
    const HsmpViewText* hidden_bones; uint32_t hidden_count; uint32_t pad_h;
    const HsmpViewMaterial* materials; uint32_t material_count; uint32_t pad_mat;
    uint32_t vertex_state; uint32_t pad_vertex; // native_asset0/captured1/runtime2/unavailable3/scene_not_applicable4
    const HsmpViewVertexLod* vertex_lods; uint32_t vertex_count; uint32_t pad_lod;
    HsmpViewSplineProfile spline; uint32_t pad_spline;
    HsmpViewText spring_arm_socket;
};
struct HsmpViewFrame {
    HsmpViewTransform world;
    HsmpViewTransform* bones; uint32_t bone_count; uint32_t pad_b;
    float* morphs; uint32_t morph_count; uint32_t pad_m;
    float* scalars; uint32_t scalar_count; uint32_t pad_s;
    float* vectors; uint32_t vector_count; uint32_t pad_v; // four floats per vector
    HsmpViewObject* textures; uint32_t texture_count; uint32_t pad_t;
    HsmpViewSplineFrame* spline;
    HsmpViewSpringArmFrame* spring_arm;
};
struct HsmpViewResult { uint32_t complete; uint32_t operations; char reason[192]; };
// ABI5 engine retirement requires positive original world membership, then
// world absence AND mirrored garbage/global disappearance. UE4SS weak validity
// is only a memory/identity qualifier, not this engine's actor liveness rule.
struct HsmpViewLifecycle {
    uint32_t known; uint32_t flags; uint32_t destroying; uint32_t listed;
    uint32_t authority; uint32_t local_role; uint32_t remote_role; uint32_t root_live;
    uint64_t name; uint64_t class_weak; uint64_t class_address;
    uint64_t level_weak; uint64_t level_address; uint64_t root_weak; uint64_t root_address;
}; // known bits: flags1,destroying2,membership4,roles8,root16,level32,identity64
struct HsmpViewRetirement {
    uint32_t qualified; uint32_t dispatched; uint32_t alive_after; uint32_t weak_present;
    uint64_t weak; uint64_t address; HsmpViewLifecycle before; HsmpViewLifecycle after;
    char reason[192]; // alive_after:0 engine-retired,1 unresolved/present,2 unknown; weak_present:0 absent,1 present,2 unknown
};
// ABI6 diagnostic only. qualified means original memory/identity and native
// census scope were verified; listed=false is not a retirement/inactive proof.
struct HsmpViewActorScope {
    uint32_t qualified; uint32_t pad; uint64_t weak; uint64_t address;
    HsmpViewLifecycle state; char reason[192];
};
struct HsmpPresentation {
    uint32_t abi; uint32_t pad;
    // Inspect kind/cloth/deformer state. Vertex colors still require the exact
    // captured recipe and native create/apply readback; absence is never inferred.
    // 1 inspected; 0 unavailable; -1 unsupported. Not a mirror-ready answer.
    int32_t (*inspect)(HsmpViewObject world,HsmpViewObject owner,HsmpViewObject component,const HsmpViewGuard*,HsmpViewResult*);
    // Recipe dictionaries and caller-owned output arrays are bounded and complete.
    int32_t (*capture)(HsmpViewObject world,HsmpViewObject owner,HsmpViewObject component,const HsmpViewComponent*,HsmpViewFrame*,const HsmpViewGuard*,HsmpViewResult*);
    // An inert Engine.Actor plus render components. Return opaque mirror handle;
    // all collision/physics/combat/animation-event execution must be disabled.
    uint64_t (*create)(HsmpViewObject world,const HsmpViewComponent*,uint32_t count,const HsmpViewGuard*,HsmpViewResult*);
    int32_t (*apply)(HsmpViewObject world,uint64_t mirror,const HsmpViewComponent*,const HsmpViewFrame*,uint32_t count,const HsmpViewGuard*,HsmpViewResult*);
    // ABI3: destroy borrows the same per-call world guard. A stale scope only
    // discards the handle; discard itself touches no UObject.
    void (*destroy)(HsmpViewObject world,uint64_t mirror,const HsmpViewGuard*);
    void (*discard)(uint64_t mirror);
    // kind0 only: exact map spawn drivers, target must be zero. Other kinds
    // refuse. Never destroys an unqualified/foreign/protected/reused actor.
    int32_t (*retire)(HsmpViewObject world,HsmpViewObject actor,uint32_t kind,HsmpViewObject target,const HsmpViewGuard*,HsmpViewRetirement*);
    // Rechecks only a previously qualified successful retirement. Never touches
    // the expired actor; native-live or reused identities fail closed.
    int32_t (*probe_retirement)(HsmpViewObject world,HsmpViewObject original,const HsmpViewGuard*,HsmpViewRetirement*);
    void (*forget_retirements)(); // scalar-only world-drop cleanup, no UObject access
    // Read-only static world/class enumeration followed by fresh original-slot
    // identity and flag reads. Never invokes an actor ProcessEvent.
    int32_t (*actor_scope)(HsmpViewObject world,HsmpViewObject actor,const HsmpViewGuard*,HsmpViewActorScope*);
    // Original source scope/handle must be qualified by Rust before/after this
    // operation. No arbitrary-address admission or rendering-ready assertion.
    int32_t (*describe_spline)(HsmpViewObject world,HsmpViewObject owner,HsmpViewObject component,const HsmpViewGuard*,HsmpViewSplineProfile*,HsmpViewResult*);
    int32_t (*describe_vertex_state)(HsmpViewObject world,HsmpViewObject owner,HsmpViewObject component,const HsmpViewGuard*,HsmpViewVertexState*,HsmpViewResult*);
    // After all callbacks, the final whole-scene census uses original identities
    // and hard fields only, without getters or ProcessEvent.
    int32_t (*finish_scene_sets)(HsmpViewObject world,const HsmpViewFinishTarget*,uint32_t target_count,const uint64_t* mirrors,uint32_t mirror_count,const HsmpViewGuard*,HsmpViewResult*);
};
void hsmp_native_set_presentation(const HsmpPresentation*);
}
static_assert(sizeof(HsmpViewText)==16);
static_assert(sizeof(HsmpViewObject)==16);
static_assert(sizeof(HsmpViewGuard)==16);
static_assert(sizeof(HsmpViewLifecycle)==88);
static_assert(sizeof(HsmpViewRetirement)==400);
static_assert(sizeof(HsmpViewActorScope)==304);
static_assert(sizeof(HsmpViewTransform)==80);
static_assert(sizeof(HsmpViewParameter)==24);
static_assert(sizeof(HsmpViewMaterial)==72);
static_assert(sizeof(HsmpViewVertexLod)==24);
static_assert(sizeof(HsmpViewSplineProfile)==20);
static_assert(sizeof(HsmpViewVertexState)==24);
static_assert(sizeof(HsmpViewSpringArmFrame)==56);
static_assert(sizeof(HsmpViewFinishTarget)==128);
static_assert(sizeof(HsmpPresentation)==112);
static_assert(sizeof(HsmpViewSplineVectorPoint)==80);
static_assert(sizeof(HsmpViewSplineQuatPoint)==104);
static_assert(sizeof(HsmpViewSplineFloatPoint)==20);
static_assert(sizeof(HsmpViewSplineSettings)==72);
static_assert(sizeof(HsmpViewSplineVectorCurve)==24);
static_assert(sizeof(HsmpViewSplineQuatCurve)==24);
static_assert(sizeof(HsmpViewSplineFloatCurve)==24);
static_assert(sizeof(HsmpViewSplineFrame)==184);
static_assert(sizeof(HsmpViewComponent)==272);
static_assert(sizeof(HsmpViewFrame)==176);
static_assert(sizeof(HsmpViewResult)==200);
// All ops borrow recipe strings/arrays only for the duration of the call.
// Caps: components64, bones512/component, morphs128/component, materials32,
// scalar/vector/texture parameter dictionaries128/material; total wire frame64KiB.
void hsmp_presentation_register(const HsmpReflect*);
// Private startup diagnostics only; does not change the provider/wire ABI.
// The callback must not call Unreal or throw. All names come from fixed code
// paths; each create operation emits at most 128 markers, never frame updates.
using HsmpPresentationCreateLog = void(*)(const char* stage,uint32_t edge,uint64_t operation,
    uint32_t marker,uint32_t component,uint32_t kind,uint32_t function_id,const char* function_name);
void hsmp_presentation_set_create_log(HsmpPresentationCreateLog);
