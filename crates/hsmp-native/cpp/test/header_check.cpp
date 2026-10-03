// Compiles the generated hsmp_ipc.h as C++20 with /W4 /WX: every static_assert runs.
#include "gen/hsmp_ipc.h"
int hsmp_ipc_header_check_cpp() { return static_cast<int>(sizeof(hsmp_Root)); }
