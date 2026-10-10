// Private game-thread native pawn provider. All pointers are borrowed per call.
#pragma once
#include <cstddef>
#include "native_presentation.h"
extern "C" {
struct HsmpGameplayValue { uint32_t kind,pad; double value; }; // 1=f32, 2=f64
struct HsmpGameplayState {
    double position[3],orientation[4],velocity[3];
    HsmpGameplayValue health,stamina;
    double cache_rotation[3]; // original native conversion-cache Euler, not Actor rotation
};
enum HsmpGameplayProofFlags : uint32_t {
    HSMP_GAMEPLAY_STATE=1, HSMP_GAMEPLAY_VISIBLE_BODY=2,
    HSMP_GAMEPLAY_POSSESSION=4, HSMP_GAMEPLAY_VIEW_TARGET=8,
    HSMP_GAMEPLAY_CAMERA=16, HSMP_GAMEPLAY_HUD=32
};
struct HsmpGameplayProof {
    HsmpViewObject world,pawn,controller,view_target,camera_manager,hud;
    uint32_t flags,own,visible_meshes,pad;
};
struct HsmpGameplay {
    uint32_t abi,pad;
    int32_t (*begin)(HsmpViewObject world,HsmpViewObject controller,HsmpViewText actor_class,
        const HsmpViewTransform*,uint32_t own,const HsmpViewGuard*,uint64_t* handle,HsmpViewObject* pawn,HsmpViewResult*);
    int32_t (*current)(uint64_t handle,const HsmpViewGuard*,HsmpViewObject* pawn,HsmpViewResult*);
    int32_t (*construct)(uint64_t handle,const HsmpViewGuard*,HsmpViewObject* pawn,HsmpViewResult*);
    int32_t (*finish)(uint64_t handle,const HsmpViewGuard*,HsmpViewResult*);
    int32_t (*apply)(uint64_t handle,const HsmpGameplayState*,const HsmpViewGuard*,HsmpGameplayProof*,HsmpViewResult*);
    int32_t (*clear)(uint64_t handle,const HsmpViewGuard*,HsmpViewResult*);
    void (*discard)(uint64_t handle); // world-drop scalar cleanup; no engine access
    int32_t (*complete)(const uint64_t* handles,uint32_t count,const HsmpViewGuard*,HsmpGameplayProof* proofs,HsmpViewResult*);
};
void hsmp_native_set_gameplay(const HsmpGameplay*);
}
static_assert(sizeof(HsmpGameplayValue)==16);
static_assert(sizeof(HsmpGameplayState)==136);
static_assert(offsetof(HsmpGameplayState,orientation)==24);
static_assert(offsetof(HsmpGameplayState,velocity)==56);
static_assert(offsetof(HsmpGameplayState,cache_rotation)==112);
static_assert(sizeof(HsmpGameplayProof)==112);
static_assert(sizeof(HsmpGameplay)==72);
static_assert(offsetof(HsmpGameplay,clear)==48);
static_assert(offsetof(HsmpGameplay,discard)==56);
static_assert(offsetof(HsmpGameplay,complete)==64);
