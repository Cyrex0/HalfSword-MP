// reflect_check <UE4SS.dll>: every export cpp/src/ue4ss_reflect.cpp resolves exists in that
// DLL (mapped without running its DllMain). Run against the mock (ctest) and, when
// -DUE4SS_DLL=<the game's UE4SS.dll> is given to CMake, against the real pinned build: that
// proves the mangled names (and so the signatures the mock was compiled from) match.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>

#include <cstdio>

#include "ue4ss_reflect_names.h"

int wmain(int argc, wchar_t** argv)
{
    if (argc < 2)
    {
        std::fprintf(stderr, "usage: reflect_check <UE4SS.dll>\n");
        return 2;
    }
    HMODULE m = LoadLibraryExW(argv[1], nullptr, DONT_RESOLVE_DLL_REFERENCES);
    if (!m)
    {
        std::fprintf(stderr, "cannot map %ls (error %lu)\n", argv[1], GetLastError());
        return 2;
    }
    int missing = 0;
    for (unsigned i = 0; i < hsmp_reflect::kCount; ++i)
    {
        if (!GetProcAddress(m, hsmp_reflect::kNames[i]))
        {
            std::fprintf(stderr, "MISSING %s\n", hsmp_reflect::kNames[i]);
            ++missing;
        }
    }
    std::printf("reflect_check %ls: %u exports, %d missing\n", argv[1], hsmp_reflect::kCount, missing);
    unsigned optional{};for(const auto* name:hsmp_reflect::kFindNames)if(GetProcAddress(m,name))++optional;
    std::printf("optional exact-path/hash exports: %u/2 (absence retains original lookup)\n",optional);
    return missing == 0 ? 0 : 1;
}
