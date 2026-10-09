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
struct HsmpViewComponent {
    uint32_t id; uint32_t parent; uint32_t kind; uint32_t visible;
    HsmpViewText asset; HsmpViewText skeleton; HsmpViewText socket;
    HsmpViewTransform relative;
    const HsmpViewText* bones; uint32_t bone_count; uint32_t pad_b;
    const HsmpViewText* morphs; uint32_t morph_count; uint32_t pad_m;
    const HsmpViewText* hidden_bones; uint32_t hidden_count; uint32_t pad_h;
    const HsmpViewMaterial* materials; uint32_t material_count; uint32_t pad_mat;
    uint32_t vertex_state; uint32_t pad_vertex; // native_asset0/captured1/runtime2/unavailable3
    const HsmpViewVertexLod* vertex_lods; uint32_t vertex_count; uint32_t pad_lod;
};
struct HsmpViewFrame {
    HsmpViewTransform world;
    HsmpViewTransform* bones; uint32_t bone_count; uint32_t pad_b;
    float* morphs; uint32_t morph_count; uint32_t pad_m;
    float* scalars; uint32_t scalar_count; uint32_t pad_s;
    float* vectors; uint32_t vector_count; uint32_t pad_v; // four floats per vector
    HsmpViewObject* textures; uint32_t texture_count; uint32_t pad_t;
};
struct HsmpViewResult { uint32_t complete; uint32_t operations; char reason[192]; };
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
};
void hsmp_native_set_presentation(const HsmpPresentation*);
}
static_assert(sizeof(HsmpViewText)==16);
static_assert(sizeof(HsmpViewObject)==16);
static_assert(sizeof(HsmpViewGuard)==16);
static_assert(sizeof(HsmpViewTransform)==80);
static_assert(sizeof(HsmpViewParameter)==24);
static_assert(sizeof(HsmpViewMaterial)==72);
static_assert(sizeof(HsmpViewVertexLod)==24);
static_assert(sizeof(HsmpViewComponent)==232);
static_assert(sizeof(HsmpViewFrame)==160);
static_assert(sizeof(HsmpViewResult)==200);
// All ops borrow recipe strings/arrays only for the duration of the call.
// Caps: components64, bones512/component, morphs128/component, materials32,
// scalar/vector/texture parameter dictionaries128/material; total wire frame64KiB.
void hsmp_presentation_register(const HsmpReflect*);
