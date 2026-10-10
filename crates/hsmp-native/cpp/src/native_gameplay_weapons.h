// Private ABI4 output. Caller owns every byte; no native row/string is borrowed.
#pragma once
#include <cstdint>
#include <cstddef>
extern "C" {
struct HsmpGameplayWeaponPath { uint32_t length,pad; uint16_t data[513]; };
struct HsmpGameplayWeaponName { uint32_t length,pad; uint16_t data[129]; };
struct HsmpGameplayWeaponPassport {
    uint32_t pawn_index,pad;
    HsmpGameplayWeaponPath actor_class;
    // WeaponClass, HeadSubModule1/2, Head, Guard, Pommel, Grip.
    HsmpGameplayWeaponPath classes[7];
    HsmpGameplayWeaponName name;
    int32_t id;
    uint8_t materials[4],tier,padding[3];
    double sizes[4][3],mass[4],price;
    float colors[2][4];
};
}
static_assert(sizeof(HsmpGameplayWeaponPath)==1036);
static_assert(sizeof(HsmpGameplayWeaponName)==268);
static_assert(sizeof(HsmpGameplayWeaponPassport)==8744);
static_assert(offsetof(HsmpGameplayWeaponPassport,classes)==1044);
static_assert(offsetof(HsmpGameplayWeaponPassport,name)==8296);
static_assert(offsetof(HsmpGameplayWeaponPassport,id)==8564);
static_assert(offsetof(HsmpGameplayWeaponPassport,sizes)==8576);
static_assert(offsetof(HsmpGameplayWeaponPassport,mass)==8672);
static_assert(offsetof(HsmpGameplayWeaponPassport,price)==8704);
static_assert(offsetof(HsmpGameplayWeaponPassport,colors)==8712);
