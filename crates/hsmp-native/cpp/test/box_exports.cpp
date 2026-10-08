// Maps the pinned DLL for export inspection only; does not execute DllMain or a probe.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <cstdio>
#include "box_snapshot_exports.h"
int wmain(int argc,wchar_t** argv)
{
    if (argc!=2) return 2;
    HMODULE m=LoadLibraryExW(argv[1],nullptr,DONT_RESOLVE_DLL_REFERENCES);
    if (!m) return 2;
    unsigned missing=0;
    for (const auto name : hsmp_box::kExports) if (!GetProcAddress(m,name))
    { std::fprintf(stderr,"MISSING %s\n",name); ++missing; }
    FreeLibrary(m);
    std::printf("box_exports: 8 verified signatures, %u missing\n",missing);
    return missing ? 1 : 0;
}
