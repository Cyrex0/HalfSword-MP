# HSMPNative native-transport spike: builds
#   out/HSMPNative/dlls/main.dll              option F: UE4SS C++ mod (+ Rust core, + private Lua 5.4.7)
#   out/HSMPNativeProbe/Scripts/hsmp_lua.dll  option E: Lua C module written in Rust (+ private Lua 5.4.7)
#   out/HSMPNativeProbe/Scripts/main.lua      in-game probe that exercises both
#   harness/harness.exe                       offline test against a mock UE4SS.dll (ctest)
#
# configure:  cmake -S . -B build -G "Visual Studio 17 2022" -A x64 -DUE4SS_SRC=<path to an RE-UE4SS checkout>
# build:      cmake --build build --config RelWithDebInfo
# test:       ctest --test-dir build -C RelWithDebInfo --output-on-failure
cmake_minimum_required(VERSION 3.22)
project(HSMPNative_spike C CXX)

if(NOT MSVC)
    message(FATAL_ERROR "UE4SS C++ mods must be built with MSVC (same C++ ABI/CRT family as UE4SS.dll)")
endif()

set(CMAKE_CONFIGURATION_TYPES "RelWithDebInfo;Release" CACHE STRING "" FORCE)
# UE4SS.dll uses the dynamic release CRT (/MD); so must we (and so does Rust's msvc target).
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreadedDLL")
set(CMAKE_C_STANDARD 11)   # _Static_assert in hsmp_luauser.c
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# ----------------------------------------------------------------- inputs
set(UE4SS_SRC "${CMAKE_CURRENT_SOURCE_DIR}/../../third_party/RE-UE4SS" CACHE PATH "RE-UE4SS checkout at the deployed commit")
set(UE4SS_PINNED_SHA "e3ba1016562d6c0868c410d0a71e88bfcdbf691b")
set(LUARAW "${UE4SS_SRC}/deps/first/LuaRaw")
if(NOT EXISTS "${LUARAW}/src/lapi.c")
    message(FATAL_ERROR "LuaRaw not found under UE4SS_SRC=${UE4SS_SRC}. Pass -DUE4SS_SRC=<RE-UE4SS checkout>.")
endif()
if(EXISTS "${UE4SS_SRC}/.git/HEAD")
    file(READ "${UE4SS_SRC}/.git/HEAD" _head)
    string(STRIP "${_head}" _head)
    if(_head MATCHES "^ref: (.*)$")
        set(_ref "${CMAKE_MATCH_1}")
        if(EXISTS "${UE4SS_SRC}/.git/${_ref}")
            file(READ "${UE4SS_SRC}/.git/${_ref}" _head)
            string(STRIP "${_head}" _head)
        endif()
    endif()
    if(NOT _head STREQUAL UE4SS_PINNED_SHA)
        message(WARNING "RE-UE4SS is at ${_head}, HSMPNative is pinned to ${UE4SS_PINNED_SHA}. Lua layout may not match the deployed UE4SS.dll.")
    else()
        message(STATUS "RE-UE4SS at pinned commit ${_head}")
    endif()
endif()

find_program(CARGO cargo HINTS "$ENV{USERPROFILE}/.cargo/bin" REQUIRED)

# ------------------------------------------------------------- Rust core
set(RUST_TARGET_DIR "${CMAKE_BINARY_DIR}/cargo")
set(RUST_CORE_LIB "${RUST_TARGET_DIR}/release/hsmp_core_stub.lib")
set(RUST_LUA_E_LIB "${RUST_TARGET_DIR}/release/hsmp_lua_e.lib")
add_custom_target(rust_libs ALL
    COMMAND "${CARGO}" build --release --manifest-path "${CMAKE_CURRENT_SOURCE_DIR}/Cargo.toml"
            --target-dir "${RUST_TARGET_DIR}" -p hsmp_core_stub -p hsmp_lua_e
    BYPRODUCTS "${RUST_CORE_LIB}" "${RUST_LUA_E_LIB}"
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    COMMENT "cargo build --release (hsmp_core_stub, hsmp_lua_e)"
    USES_TERMINAL)
# from `cargo rustc -- --print native-static-libs`
set(RUST_SYSLIBS kernel32 ntdll userenv ws2_32 dbghelp bcrypt advapi32)

# ------------------------------------------- UE4SS import lib (from .def)
set(UE4SS_IMPLIB "${CMAKE_BINARY_DIR}/implib/UE4SS.lib")
add_custom_command(OUTPUT "${UE4SS_IMPLIB}"
    COMMAND "${CMAKE_COMMAND}" -E make_directory "${CMAKE_BINARY_DIR}/implib"
    COMMAND "${CMAKE_AR}" /nologo "/def:${CMAKE_CURRENT_SOURCE_DIR}/cpp/abi/UE4SS.def" /machine:x64 "/out:${UE4SS_IMPLIB}"
    DEPENDS "${CMAKE_CURRENT_SOURCE_DIR}/cpp/abi/UE4SS.def"
    COMMENT "UE4SS.lib import library from abi/UE4SS.def")
add_custom_target(ue4ss_implib DEPENDS "${UE4SS_IMPLIB}")

# ------------------------------------------------- Lua 5.4.7 (UE4SS's own)
file(GLOB _lua_c "${LUARAW}/src/*.c")
list(FILTER _lua_c EXCLUDE REGEX "/(lua|luac|luauser)\\.c$")

# private copy linked into our DLLs: luauser.c replaced by hsmp_luauser.c (lock forwarding)
add_library(lua_vm_mod STATIC ${_lua_c} cpp/src/hsmp_luauser.c)
target_include_directories(lua_vm_mod PUBLIC "${LUARAW}/include" cpp/abi PRIVATE "${LUARAW}/src")
target_compile_options(lua_vm_mod PRIVATE /W0)
set_target_properties(lua_vm_mod PROPERTIES LINKER_LANGUAGE C)

# host copy for the offline harness (stands in for the copy inside UE4SS.dll)
add_library(lua_vm_host STATIC ${_lua_c} "${LUARAW}/src/luauser.c")
target_include_directories(lua_vm_host PUBLIC "${LUARAW}/include")
target_compile_options(lua_vm_host PRIVATE /W0)

set(OUT "${CMAKE_BINARY_DIR}/out")

# --------------------------------------------- option F: HSMPNative main.dll
add_library(HSMPNative SHARED cpp/src/dllmain.cpp)
add_dependencies(HSMPNative rust_libs ue4ss_implib)
target_include_directories(HSMPNative PRIVATE cpp/abi cpp/src)
target_compile_options(HSMPNative PRIVATE /W4 /permissive- /Zc:__cplusplus /EHsc /utf-8)
target_link_libraries(HSMPNative PRIVATE lua_vm_mod "${RUST_CORE_LIB}" "${UE4SS_IMPLIB}" ${RUST_SYSLIBS})
target_link_options(HSMPNative PRIVATE /DEBUG /INCREMENTAL:NO /OPT:REF /OPT:ICF)
set_target_properties(HSMPNative PROPERTIES OUTPUT_NAME main
    RUNTIME_OUTPUT_DIRECTORY_RELWITHDEBINFO "${OUT}/HSMPNative/dlls"
    RUNTIME_OUTPUT_DIRECTORY_RELEASE "${OUT}/HSMPNative/dlls")

# ---------------------------------------- option E: hsmp_lua.dll (Rust module)
add_library(hsmp_lua SHARED cpp/src/hsmp_lua_module.c cpp/src/hsmp_lua.def)
add_dependencies(hsmp_lua rust_libs)
target_link_libraries(hsmp_lua PRIVATE "${RUST_LUA_E_LIB}" lua_vm_mod ${RUST_SYSLIBS})
target_link_options(hsmp_lua PRIVATE /DEBUG /INCREMENTAL:NO /OPT:REF /OPT:ICF)
set_target_properties(hsmp_lua PROPERTIES LINKER_LANGUAGE C
    RUNTIME_OUTPUT_DIRECTORY_RELWITHDEBINFO "${OUT}/HSMPNativeProbe/Scripts"
    RUNTIME_OUTPUT_DIRECTORY_RELEASE "${OUT}/HSMPNativeProbe/Scripts")

add_custom_command(TARGET hsmp_lua POST_BUILD
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different "${CMAKE_CURRENT_SOURCE_DIR}/lua/HSMPNativeProbe/Scripts/main.lua"
            "${OUT}/HSMPNativeProbe/Scripts/main.lua")

# ------------------------------------------------------- offline harness
set(HARNESS "${CMAKE_BINARY_DIR}/harness")
add_library(mock_ue4ss SHARED cpp/test/mock_ue4ss.cpp)
target_include_directories(mock_ue4ss PRIVATE cpp/abi "${LUARAW}/include")
target_compile_definitions(mock_ue4ss PRIVATE "RC_UE4SS_API=__declspec(dllexport)")
target_compile_options(mock_ue4ss PRIVATE /EHsc /utf-8)
set_target_properties(mock_ue4ss PROPERTIES OUTPUT_NAME UE4SS
    RUNTIME_OUTPUT_DIRECTORY_RELWITHDEBINFO "${HARNESS}" RUNTIME_OUTPUT_DIRECTORY_RELEASE "${HARNESS}"
    ARCHIVE_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/mock_implib")

add_executable(harness cpp/test/harness.cpp)
target_include_directories(harness PRIVATE cpp/abi)
target_compile_options(harness PRIVATE /EHsc /utf-8)
target_link_libraries(harness PRIVATE lua_vm_host)
add_dependencies(harness HSMPNative hsmp_lua mock_ue4ss)
set_target_properties(harness PROPERTIES
    RUNTIME_OUTPUT_DIRECTORY_RELWITHDEBINFO "${HARNESS}" RUNTIME_OUTPUT_DIRECTORY_RELEASE "${HARNESS}")

enable_testing()
add_test(NAME harness_F_and_E
    COMMAND harness "${OUT}/HSMPNative/dlls/main.dll" "${OUT}/HSMPNativeProbe/Scripts"
            "${CMAKE_CURRENT_SOURCE_DIR}/cpp/test/harness_test.lua"
    WORKING_DIRECTORY "${HARNESS}")
