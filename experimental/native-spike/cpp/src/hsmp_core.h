// C ABI of the Rust staticlib crates/hsmp_core_stub (see src/lib.rs there).
#pragma once
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define HSMP_OK 0
#define HSMP_E_NOT_RUNNING (-1)
#define HSMP_E_ALREADY_RUNNING (-2)
#define HSMP_E_BAD_ARG (-3)
#define HSMP_E_IO (-4)
#define HSMP_E_FULL (-5)
#define HSMP_E_TOO_BIG (-6)
#define HSMP_E_BUSY (-7)
#define HSMP_E_EMPTY (-8)
#define HSMP_E_TOO_SMALL (-9)
#define HSMP_E_PANIC (-99)

typedef struct HsmpStats
{
    uint64_t sent, recv, send_err, recv_err, drop_in_full, drop_out_full, loops;
    uint32_t inq_len, outq_len, running, local_port;
} HsmpStats;

const char* hsmp_core_version(void);
uint64_t hsmp_core_now_us(void);
int32_t hsmp_core_start(const char* bind, const char* peer); /* port > 0 or error */
int32_t hsmp_core_stop(void);
int32_t hsmp_core_send(const uint8_t* data, size_t len);
int32_t hsmp_core_poll(uint8_t* out, size_t cap); /* length >= 0 or error */
int32_t hsmp_core_stats(HsmpStats* out);
uint32_t hsmp_core_max_payload(void);

/* hsmp_luauser.c */
const char* hsmp_lua_lock_mode(void);

#ifdef __cplusplus
}
#endif
