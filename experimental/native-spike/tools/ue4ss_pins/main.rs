//! Pin HSMPNative to one exact UE4SS binary.
//!
//! Reads the deployed UE4SS.dll (PE header + export table) and UE4SS.pdb
//! (through dbghelp) and writes:
//!
//!   cpp/abi/UE4SS.def     exports HSMPNative links against; CMake turns it
//!                         into an import library with lib.exe /def
//!   cpp/abi/ue4ss_pins.h  identity of that DLL (TimeDateStamp, SizeOfImage),
//!                         the RVA of the Lua lock (`Gl` in luauser.c) and the
//!                         sizes of Lua's internal structs as compiled INTO
//!                         UE4SS.dll; hsmp_luauser.c static_asserts our private
//!                         Lua copy against them
//!
//! Re-run after ANY UE4SS update; a missing export or a size mismatch is the
//! ABI break this pin exists to catch, and this tool or the build fails loudly.
//!
//! usage: cargo run -p ue4ss_pins -- <path\to\ue4ss\UE4SS.dll> [git_sha]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Exports HSMPNative needs: (prefix match, description). Each must match >= 1.
const REQUIRED: &[(&str, bool)] = &[
    ("??0CppUserModBase@RC@@QEAA@XZ", true),
    ("??1CppUserModBase@RC@@UEAA@XZ", true),
    ("?on_", false), // filtered to @CppUserModBase@RC@@ below
    ("?render_tab@CppUserModBase@RC@@", false),
    ("?get_lua_state@Lua@LuaMadeSimple@RC@@QEBAPEAUlua_State@@XZ", true),
    (
        "?StaticFindObject_InternalSlow@UObjectGlobals@Unreal@RC@@YAPEAVUObject@23@PEAVUClass@23@PEAV423@PEB_W_N@Z",
        true,
    ),
    ("?ProcessEvent@UObject@Unreal@RC@@QEAAXPEAVUFunction@23@PEAX@Z", true),
];

const PDB_SYMBOLS: &[&str] = &["Gl", "LuaLock", "LuaUnlock", "lua_pushcclosure"];
/// Lua internals whose layout must be identical in both copies.
const LUA_TYPES: &[&str] = &[
    "lua_State",
    "global_State",
    "CallInfo",
    "Table",
    "TString",
    "Udata",
    "CClosure",
    "LClosure",
    "Proto",
    "UpVal",
    "lua_Debug",
    "stringtable",
];

struct Pe {
    timestamp: u32,
    size_of_image: u32,
    exports: Vec<String>,
}

fn rd16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(d[o..o + 2].try_into().unwrap())
}
fn rd32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

fn read_pe(path: &Path) -> Result<Pe, String> {
    let d = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let pe = rd32(&d, 0x3C) as usize;
    if &d[pe..pe + 4] != b"PE\0\0" {
        return Err("not a PE file".into());
    }
    let nsec = rd16(&d, pe + 6) as usize;
    let timestamp = rd32(&d, pe + 8);
    let opt = pe + 24;
    if rd16(&d, opt) != 0x20B {
        return Err("expected PE32+".into());
    }
    let size_of_image = rd32(&d, opt + 56);
    let export_rva = rd32(&d, opt + 112) as usize;
    let sec = opt + rd16(&d, pe + 20) as usize;
    let sections: Vec<(usize, usize, usize)> = (0..nsec)
        .map(|i| {
            let s = sec + i * 40;
            let vsize = rd32(&d, s + 8) as usize;
            let va = rd32(&d, s + 12) as usize;
            let rawsize = rd32(&d, s + 16) as usize;
            let rawptr = rd32(&d, s + 20) as usize;
            (va, vsize.max(rawsize), rawptr)
        })
        .collect();
    let off = |rva: usize| -> Result<usize, String> {
        sections
            .iter()
            .find(|(va, size, _)| *va <= rva && rva < va + size)
            .map(|(va, _, raw)| rva - va + raw)
            .ok_or_else(|| format!("rva {rva:#x} not in any section"))
    };
    let mut exports = Vec::new();
    if export_rva != 0 {
        let e = off(export_rva)?;
        let n = rd32(&d, e + 24) as usize;
        let names = off(rd32(&d, e + 32) as usize)?;
        for i in 0..n {
            let o = off(rd32(&d, names + 4 * i) as usize)?;
            let end = d[o..].iter().position(|&b| b == 0).ok_or("bad export name")? + o;
            exports.push(String::from_utf8_lossy(&d[o..end]).into_owned());
        }
    }
    Ok(Pe { timestamp, size_of_image, exports })
}

// ------------------------------------------------------------------ dbghelp
#[allow(non_snake_case)]
mod dbghelp {
    use std::ffi::c_void;
    pub type HANDLE = *mut c_void;

    #[repr(C)]
    pub struct SYMBOL_INFOW {
        pub SizeOfStruct: u32,
        pub TypeIndex: u32,
        pub Reserved: [u64; 2],
        pub Index: u32,
        pub Size: u32,
        pub ModBase: u64,
        pub Flags: u32,
        pub Value: u64,
        pub Address: u64,
        pub Register: u32,
        pub Scope: u32,
        pub Tag: u32,
        pub NameLen: u32,
        pub MaxNameLen: u32,
        pub Name: [u16; 1024],
    }

    #[link(name = "dbghelp")]
    extern "system" {
        pub fn SymSetOptions(opts: u32) -> u32;
        pub fn SymInitializeW(h: HANDLE, search: *const u16, invade: i32) -> i32;
        pub fn SymLoadModuleExW(
            h: HANDLE,
            file: HANDLE,
            image: *const u16,
            module: *const u16,
            base: u64,
            size: u32,
            data: *mut c_void,
            flags: u32,
        ) -> u64;
        pub fn SymFromNameW(h: HANDLE, name: *const u16, sym: *mut SYMBOL_INFOW) -> i32;
        pub fn SymGetTypeFromNameW(h: HANDLE, base: u64, name: *const u16, sym: *mut SYMBOL_INFOW) -> i32;
        pub fn SymGetTypeInfo(h: HANDLE, base: u64, type_id: u32, info: u32, out: *mut c_void) -> i32;
        pub fn SymCleanup(h: HANDLE) -> i32;
    }
    pub const TI_GET_LENGTH: u32 = 2;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

struct Pdb {
    h: dbghelp::HANDLE,
    base: u64,
}

impl Pdb {
    fn open(dll: &Path) -> Result<Self, String> {
        use dbghelp::*;
        let h = 0x4853usize as HANDLE;
        let dir = dll.parent().unwrap_or(Path::new("."));
        unsafe {
            SymSetOptions(0x2); // SYMOPT_UNDNAME
            if SymInitializeW(h, wide(&dir.to_string_lossy()).as_ptr(), 0) == 0 {
                return Err("SymInitialize failed".into());
            }
            let base = 0x1_8000_0000u64;
            if SymLoadModuleExW(h, std::ptr::null_mut(), wide(&dll.to_string_lossy()).as_ptr(), std::ptr::null(), base, 0,
                std::ptr::null_mut(), 0) == 0
            {
                return Err("SymLoadModuleEx failed (is UE4SS.pdb next to UE4SS.dll?)".into());
            }
            Ok(Pdb { h, base })
        }
    }

    fn new_info() -> Box<dbghelp::SYMBOL_INFOW> {
        // SAFETY: plain-old-data struct, all-zero is valid
        let mut si: Box<dbghelp::SYMBOL_INFOW> = Box::new(unsafe { std::mem::zeroed() });
        si.SizeOfStruct = (std::mem::size_of::<dbghelp::SYMBOL_INFOW>() - 2 * 1023) as u32;
        si.MaxNameLen = 1024;
        si
    }

    /// (rva, size) of a function or data symbol.
    fn symbol(&self, name: &str) -> Result<(u64, u32), String> {
        let mut si = Self::new_info();
        if unsafe { dbghelp::SymFromNameW(self.h, wide(name).as_ptr(), &mut *si) } == 0 {
            return Err(format!("symbol {name} not in PDB"));
        }
        Ok((si.Address - self.base, si.Size))
    }

    fn type_size(&self, name: &str) -> Result<u64, String> {
        let mut si = Self::new_info();
        if unsafe { dbghelp::SymGetTypeFromNameW(self.h, self.base, wide(name).as_ptr(), &mut *si) } == 0 {
            return Err(format!("type {name} not in PDB"));
        }
        let mut len = 0u64;
        let ok = unsafe {
            dbghelp::SymGetTypeInfo(self.h, self.base, si.TypeIndex, dbghelp::TI_GET_LENGTH, &mut len as *mut u64 as *mut _)
        };
        if ok == 0 {
            return Err(format!("no length for type {name}"));
        }
        Ok(len)
    }
}

impl Drop for Pdb {
    fn drop(&mut self) {
        unsafe { dbghelp::SymCleanup(self.h) };
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        return Err("usage: ue4ss_pins <path\\to\\UE4SS.dll> [git_sha]".into());
    }
    let dll = PathBuf::from(&args[1]);
    let sha = args.get(2).cloned().unwrap_or_else(|| "unknown".into());
    let pe = read_pe(&dll)?;

    let mut chosen: Vec<String> = Vec::new();
    for &(pat, exact) in REQUIRED {
        let hits: Vec<&String> = pe
            .exports
            .iter()
            .filter(|e| {
                if exact {
                    e.as_str() == pat
                } else if pat == "?on_" {
                    e.starts_with(pat) && e.contains("@CppUserModBase@RC@@")
                } else {
                    e.starts_with(pat)
                }
            })
            .collect();
        if hits.is_empty() {
            return Err(format!("ABI BREAK: no export matches {pat}"));
        }
        for h in hits {
            if !chosen.contains(h) {
                chosen.push(h.clone());
            }
        }
    }

    let pdb = Pdb::open(&dll)?;
    let mut syms = Vec::new();
    for n in PDB_SYMBOLS {
        syms.push((*n, pdb.symbol(n)?));
    }
    if syms[0].1 .1 != 48 {
        return Err(format!("ABI BREAK: luauser.c Gl is {} bytes, expected 48 (CRITICAL_SECTION + BOOL)", syms[0].1 .1));
    }
    let mut types = Vec::new();
    for t in LUA_TYPES {
        types.push((*t, pdb.type_size(t)?));
    }

    let abi = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("cpp").join("abi");
    std::fs::create_dir_all(&abi).map_err(|e| e.to_string())?;
    let file_name = dll.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();

    let mut def = String::new();
    writeln!(def, "; generated by tools/ue4ss_pins from {file_name} (TimeDateStamp 0x{:08X}, git {sha})", pe.timestamp).unwrap();
    def.push_str("LIBRARY UE4SS.dll\nEXPORTS\n");
    for e in &chosen {
        writeln!(def, "    {e}").unwrap();
    }
    std::fs::write(abi.join("UE4SS.def"), def).map_err(|e| e.to_string())?;

    let mut h = String::new();
    h.push_str("// generated by tools/ue4ss_pins -- do not edit\n#pragma once\n");
    writeln!(h, "#define HSMP_UE4SS_GIT_SHA \"{sha}\"").unwrap();
    writeln!(h, "#define HSMP_UE4SS_TIMESTAMP 0x{:08X}u", pe.timestamp).unwrap();
    writeln!(h, "#define HSMP_UE4SS_SIZE_OF_IMAGE 0x{:08X}u", pe.size_of_image).unwrap();
    for (n, (rva, size)) in &syms {
        writeln!(h, "#define HSMP_UE4SS_RVA_{n} 0x{rva:08X}u /* size {size} */").unwrap();
    }
    h.push_str("// sizeof() of Lua 5.4 internals as compiled into UE4SS.dll (from its PDB)\n");
    for (t, size) in &types {
        writeln!(h, "#define HSMP_UE4SS_SIZEOF_{t} {size}u").unwrap();
    }
    std::fs::write(abi.join("ue4ss_pins.h"), h).map_err(|e| e.to_string())?;

    println!(
        "ok: {} exports, TimeDateStamp 0x{:08X}, SizeOfImage 0x{:08X}, Gl @ RVA 0x{:08X}, {} Lua struct sizes",
        chosen.len(),
        pe.timestamp,
        pe.size_of_image,
        syms[0].1 .0,
        types.len()
    );
    for (t, s) in &types {
        println!("  sizeof({t}) = {s}");
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("ue4ss_pins: {e}");
        std::process::exit(1);
    }
}
