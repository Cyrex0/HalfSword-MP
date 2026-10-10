// Display-only client fighter pose publication. Pointers are borrowed for one game-thread call.
#pragma once
#include <cstdint>
extern "C" {
// Copies the hidden Poseable calculator's component-space pose into the visible
// SkeletalMeshComponent and publishes it. 1 = published, 0 = refused (reason written).
using HsmpPuppetPublish = int32_t (*)(void* calculator, void* render, uint32_t count, char* reason, uint32_t capacity);
void hsmp_native_set_puppet_publish(HsmpPuppetPublish publish);
}
