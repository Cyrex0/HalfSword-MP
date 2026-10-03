/* Compiles the generated hsmp_ipc.h as C11 with /W4 /WX: every static_assert runs. */
#include "gen/hsmp_ipc.h"
int hsmp_ipc_header_check_c(void) { return (int)sizeof(hsmp_PeerPlay); }
