# HSMPNative (HSMP-SHM game side). Builds
#   out/HSMPNative/dlls/main.dll           option F: UE4SS C++ mod + Rust hsmp_native.lib + private Lua 5.4.7
#   out/HSMPNative/dlls/hsmp_lua.dll       option E: Lua C module (same Rust lib), loaded with package.loadlib
#   out/HSMPNativeProbe/Scripts/main.lua   in-game probe (dev only): prints the IPC VERDICT line
#   harness/harness.exe                    offline test against a mock UE4SS.dll + hsmp-fake-sidecar.exe (ctest)
#
# configure:  cmake -S crates/hsmp-native -B <build> -G "Visual Studio 17 2022" -A x64 -DUE4SS_SRC=<RE-UE4SS checkout>
# build:      cmake --build <build> --config Release
# test:       ctest --test-dir <build> -C Release --output-on-failure
cmake_minimum_required(VERSION 3.22)
project(HSMPNative C CXX)

if(NOT MSVC)
    message(FATAL_ERROR "UE4SS C++ mods must be built with MSVC (same C++ ABI/CRT family as UE4SS.dll)")
endif()

set(CMAKE_CONFIGURATION_TYPES "Release;RelWithDebInfo" CACHE STRING "" FORCE)
# UE4SS.dll uses the dynamic release CRT (/MD); so must we (and so does Rust's msvc target).
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreadedDLL")
set(CMAKE_C_STANDARD 11)
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

get_filename_component(REPO_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/../.." ABSOLUTE)

# ----------------------------------------------------------------- inputs
set(UE4SS_SRC "${REPO_ROOT}/third_party/RE-UE4SS" CACHE PATH "RE-UE4SS checkout at the deployed commit")
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

# ------------------------------------------------------------- Rust parts
set(RUST_TARGET_DIR "${CMAKE_BINARY_DIR}/cargo")
set(RUST_NATIVE_LIB "${RUST_TARGET_DIR}/release/hsmp_native.lib")
set(FAKE_SIDECAR "${RUST_TARGET_DIR}/release/hsmp-fake-sidecar.exe")
add_custom_target(rust_libs ALL
    COMMAND "${CARGO}" build --release --manifest-path "${REPO_ROOT}/Cargo.toml"
            --target-dir "${RUST_TARGET_DIR}" -p hsmp-native -p hsmp-fake-sidecar
    BYPRODUCTS "${RUST_NATIVE_LIB}" "${FAKE_SIDECAR}"
    WORKING_DIRECTORY "${REPO_ROOT}"
    COMMENT "cargo build --release -p hsmp-native -p hsmp-fake-sidecar"
    USES_TERMINAL)
# from `cargo rustc -p hsmp-native --crate-type staticlib -- --print native-static-libs`
# (windows.0.52.0.lib = windows-sys import umbrella -> the system import libs below)
set(RUST_SYSLIBS kernel32 advapi32 bcrypt ntdll userenv ws2_32 dbghelp)

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
set(OUT_DLLS "${OUT}/HSMPNative/dlls")

# ------------------------------------------------------- generated header check
add_library(hsmp_ipc_header_check OBJECT cpp/test/header_check.c cpp/test/header_check.cpp)
target_include_directories(hsmp_ipc_header_check PRIVATE cpp)
target_compile_options(hsmp_ipc_header_check PRIVATE /W4 /WX)

# --------------------------------------------- option F: HSMPNative main.dll
add_library(HSMPNative SHARED cpp/src/dllmain.cpp cpp/src/ue4ss_reflect.cpp
    cpp/src/box_snapshot_probe.cpp cpp/src/ue4ss_reflect_box.cpp)
add_dependencies(HSMPNative rust_libs ue4ss_implib hsmp_ipc_header_check)
target_include_directories(HSMPNative PRIVATE cpp cpp/abi cpp/src)
target_compile_options(HSMPNative PRIVATE /W4 /WX /permissive- /Zc:__cplusplus /EHsc /utf-8)
target_link_libraries(HSMPNative PRIVATE lua_vm_mod "${RUST_NATIVE_LIB}" "${UE4SS_IMPLIB}" ${RUST_SYSLIBS})
target_link_options(HSMPNative PRIVATE /DEBUG /INCREMENTAL:NO /OPT:REF /OPT:ICF)
set_target_properties(HSMPNative PROPERTIES OUTPUT_NAME main
    RUNTIME_OUTPUT_DIRECTORY_RELWITHDEBINFO "${OUT_DLLS}"
    RUNTIME_OUTPUT_DIRECTORY_RELEASE "${OUT_DLLS}")

# ---------------------------------------- option E: hsmp_lua.dll (same Rust lib)
add_library(hsmp_lua SHARED cpp/src/hsmp_lua_module.c cpp/src/hsmp_lua.def)
add_dependencies(hsmp_lua rust_libs)
target_link_libraries(hsmp_lua PRIVATE "${RUST_NATIVE_LIB}" lua_vm_mod ${RUST_SYSLIBS})
target_link_options(hsmp_lua PRIVATE /DEBUG /INCREMENTAL:NO /OPT:REF /OPT:ICF)
set_target_properties(hsmp_lua PROPERTIES LINKER_LANGUAGE C
    RUNTIME_OUTPUT_DIRECTORY_RELWITHDEBINFO "${OUT_DLLS}"
    RUNTIME_OUTPUT_DIRECTORY_RELEASE "${OUT_DLLS}")

add_custom_command(TARGET hsmp_lua POST_BUILD
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different "${CMAKE_CURRENT_SOURCE_DIR}/probe/HSMPNativeProbe/Scripts/main.lua"
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
add_executable(box_snapshot_probe cpp/test/box_snapshot_probe.cpp cpp/src/box_snapshot_probe.cpp)
target_include_directories(box_snapshot_probe PRIVATE cpp/src)
target_link_libraries(box_snapshot_probe PRIVATE lua_vm_host)
target_compile_options(box_snapshot_probe PRIVATE /W4 /WX /permissive- /EHsc /utf-8)
add_test(NAME box_snapshot_probe COMMAND box_snapshot_probe)
# Compile the actual provider without building/linking the game DLL or the Rust ALL target.
add_library(box_provider_compile OBJECT cpp/src/ue4ss_reflect_box.cpp cpp/src/ue4ss_reflect.cpp cpp/src/dllmain.cpp)
target_include_directories(box_provider_compile PRIVATE cpp cpp/src cpp/abi)
target_link_libraries(box_provider_compile PRIVATE lua_vm_host)
target_compile_options(box_provider_compile PRIVATE /W4 /WX /permissive- /EHsc /utf-8)
add_executable(box_exports cpp/test/box_exports.cpp)
target_include_directories(box_exports PRIVATE cpp/src)
if(UE4SS_DLL)
    add_test(NAME box_exports_real COMMAND box_exports "${UE4SS_DLL}")
endif()
add_executable(caller_frame_walk cpp/test/caller_frame_walk.cpp)
target_include_directories(caller_frame_walk PRIVATE cpp/src)
target_compile_options(caller_frame_walk PRIVATE /W4 /WX /permissive- /EHsc)
add_test(NAME caller_frame_walk COMMAND caller_frame_walk)
add_test(NAME harness_F
    COMMAND harness F "${OUT_DLLS}/main.dll" "${OUT_DLLS}/hsmp_lua.dll" "${FAKE_SIDECAR}"
            "${CMAKE_CURRENT_SOURCE_DIR}/cpp/test/harness_ipc.lua"
    WORKING_DIRECTORY "${HARNESS}")
add_test(NAME harness_E
    COMMAND harness E "${OUT_DLLS}/hsmp_lua.dll" "${FAKE_SIDECAR}"
            "${CMAKE_CURRENT_SOURCE_DIR}/cpp/test/harness_ipc.lua"
    WORKING_DIRECTORY "${HARNESS}")

# Native sampling and servo against the mock engine (and the Soft* refusal)
add_test(NAME harness_sample
    COMMAND harness F "${OUT_DLLS}/main.dll" "${OUT_DLLS}/hsmp_lua.dll" "${FAKE_SIDECAR}"
            "${CMAKE_CURRENT_SOURCE_DIR}/cpp/test/harness_sample.lua"
    WORKING_DIRECTORY "${HARNESS}")
add_test(NAME harness_sample_soft
    COMMAND harness F "${OUT_DLLS}/main.dll" "${OUT_DLLS}/hsmp_lua.dll" "${FAKE_SIDECAR}"
            "${CMAKE_CURRENT_SOURCE_DIR}/cpp/test/harness_sample.lua"
    WORKING_DIRECTORY "${HARNESS}")
set_tests_properties(harness_sample_soft PROPERTIES ENVIRONMENT "MOCK_SOFT=1")
add_test(NAME harness_sample_serials
    COMMAND harness F "${OUT_DLLS}/main.dll" "${OUT_DLLS}/hsmp_lua.dll" "${FAKE_SIDECAR}"
            "${CMAKE_CURRENT_SOURCE_DIR}/cpp/test/harness_sample.lua"
    WORKING_DIRECTORY "${HARNESS}")
set_tests_properties(harness_sample_serials PROPERTIES ENVIRONMENT "MOCK_SERIALS=1")

# The exports ue4ss_reflect.cpp resolves exist in the mock and (with -DUE4SS_DLL) in the real build.
add_executable(reflect_check cpp/test/reflect_check.cpp)
target_include_directories(reflect_check PRIVATE cpp/src)
add_dependencies(reflect_check mock_ue4ss)
set_target_properties(reflect_check PROPERTIES
    RUNTIME_OUTPUT_DIRECTORY_RELWITHDEBINFO "${HARNESS}" RUNTIME_OUTPUT_DIRECTORY_RELEASE "${HARNESS}")
add_test(NAME reflect_exports_mock COMMAND reflect_check "${HARNESS}/UE4SS.dll")
set(UE4SS_DLL "" CACHE FILEPATH "the deployed (pinned) UE4SS.dll, to check the reflection exports against")
if(UE4SS_DLL)
    add_test(NAME reflect_exports_real COMMAND reflect_check "${UE4SS_DLL}")
endif()
