// The UE4SS.dll exports ue4ss_reflect.cpp resolves (in its Api member order). Shared with
// test/reflect_check.cpp, which checks them against a real (or the mock) UE4SS.dll.
#pragma once

namespace hsmp_reflect
{
    inline const char* const kNames[] = {
        "??0FName@Unreal@RC@@QEAA@PEB_WW4EFindName@12@PEAX@Z",
        "?StaticFindObject_InternalSlow@UObjectGlobals@Unreal@RC@@YAPEAVUObject@23@PEAVUClass@23@PEAV423@PEB_W_N@Z",
        "?ProcessEvent@UObject@Unreal@RC@@QEAAXPEAVUFunction@23@PEAX@Z",
        "?IsA@UObjectBase@Unreal@RC@@QEBA_NPEAVUClass@23@@Z",
        "?GetClassPrivate@UObjectBase@Unreal@RC@@QEBAAEAPEBVUClass@23@XZ",
        "?GetChildProperties@UStruct@Unreal@RC@@QEAAAEAPEAVFField@23@XZ",
        "?GetPropertiesSize@UStruct@Unreal@RC@@QEAAAEAHXZ",
        "?GetNextFieldAsProperty@FField@Unreal@RC@@QEAAPEAVFProperty@23@XZ",
        "?GetFName@FField@Unreal@RC@@QEBA?AVFName@23@XZ",
        "?GetClass@FField@Unreal@RC@@QEAA?AVFFieldClassVariant@23@XZ",
        "?GetFName@FFieldClassVariant@Unreal@RC@@QEBA?AVFName@23@XZ",
        "?GetOffset_Internal@FProperty@Unreal@RC@@QEAAAEAHXZ",
        "?GetElementSize@FProperty@Unreal@RC@@QEAAAEAHXZ",
        "?GetStruct@FStructProperty@Unreal@RC@@QEAAAEAV?$TObjectPtr@VUScriptStruct@Unreal@RC@@@23@XZ",
        "?GetByteOffset@FBoolProperty@Unreal@RC@@QEAAAEAEXZ",
        "?GetByteMask@FBoolProperty@Unreal@RC@@QEAAAEAEXZ",
        "?GetPropertyByNameInChain@UObject@Unreal@RC@@QEAAPEAVFProperty@23@PEB_W@Z",
        "?GetNamePrivate@UObjectBase@Unreal@RC@@QEBAAEBVFName@23@XZ",
        "?GetInternalIndex_Private@UObjectBase@Unreal@RC@@QEBAAEBHXZ",
        "?IndexToObject@FUObjectArray@Unreal@RC@@SAPEAUFUObjectItem@23@H@Z",
        "?GetObject@FUObjectItem@Unreal@RC@@AEAAAEAPEAVUObjectBase@23@XZ",
        "?GetSerialNumber@FUObjectItem@Unreal@RC@@QEAAAEAHXZ",
        "?IsValid@FUObjectItem@Unreal@RC@@QEBA_N_N@Z",
    };
    inline constexpr unsigned kCount = sizeof(kNames) / sizeof(kNames[0]);
} // namespace hsmp_reflect
