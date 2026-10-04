//! The machine facts a bug report needs: Windows version, CPU, memory and GPUs, read from
//! the registry (no WMI, no extra process). Nothing that identifies the machine or its owner.

use serde_json::{json, Value};

#[cfg(windows)]
fn reg_str(key: &winreg::RegKey, name: &str) -> Option<String> {
    key.get_value::<String, _>(name).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// "Windows 11 Pro 24H2 (build 26100.4061)"
#[cfg(windows)]
pub fn windows_version() -> String {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    let Ok(k) = winreg::RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion") else { return "Windows (unknown)".into() };
    let build: String = reg_str(&k, "CurrentBuild").unwrap_or_default();
    let ubr: Option<u32> = k.get_value("UBR").ok();
    let mut product = reg_str(&k, "ProductName").unwrap_or_else(|| "Windows".into());
    // Windows 11 still reports "Windows 10" in ProductName
    if build.parse::<u32>().is_ok_and(|b| b >= 22000) {
        product = product.replace("Windows 10", "Windows 11");
    }
    let disp = reg_str(&k, "DisplayVersion").or_else(|| reg_str(&k, "ReleaseId")).unwrap_or_default();
    let ubr = ubr.map(|u| format!(".{u}")).unwrap_or_default();
    format!("{product} {disp} (build {build}{ubr})").replace("  ", " ")
}

#[cfg(windows)]
fn gpus() -> Vec<Value> {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    let mut out = vec![];
    let Ok(cls) = winreg::RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey("SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e968-e325-11ce-bfc1-08002be10318}") else { return out };
    for name in cls.enum_keys().flatten().filter(|n| n.len() == 4 && n.bytes().all(|b| b.is_ascii_digit())) {
        let Ok(k) = cls.open_subkey(&name) else { continue };
        let Some(desc) = reg_str(&k, "DriverDesc") else { continue };
        let mem: Option<u64> = k.get_value("HardwareInformation.qwMemorySize").ok();
        out.push(json!({"name": desc, "driver": reg_str(&k, "DriverVersion"), "driver_date": reg_str(&k, "DriverDate"), "vram_mb": mem.map(|m| m / (1024 * 1024))}));
    }
    out
}

#[cfg(windows)]
fn cpu() -> Value {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    let root = winreg::RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey("HARDWARE\\DESCRIPTION\\System\\CentralProcessor").ok();
    let name = root.as_ref().and_then(|r| r.open_subkey("0").ok()).and_then(|k| reg_str(&k, "ProcessorNameString"));
    let threads = root.as_ref().map(|r| r.enum_keys().count());
    json!({"name": name, "threads": threads})
}

#[cfg(windows)]
fn memory_mb() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: the struct is zeroed and its length set, as the call requires.
    unsafe {
        let mut m: MEMORYSTATUSEX = std::mem::zeroed();
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        let ok = GlobalMemoryStatusEx(&mut m) != 0;
        ok.then_some(m.ullTotalPhys / (1024 * 1024))
    }
}

#[cfg(not(windows))]
pub fn windows_version() -> String {
    format!("{} (not Windows)", std::env::consts::OS)
}
#[cfg(not(windows))]
fn gpus() -> Vec<Value> {
    vec![]
}
#[cfg(not(windows))]
fn cpu() -> Value {
    json!({"name": null, "threads": std::thread::available_parallelism().ok().map(|n| n.get())})
}
#[cfg(not(windows))]
fn memory_mb() -> Option<u64> {
    None
}

pub fn collect() -> Value {
    json!({"windows": windows_version(), "cpu": cpu(), "memory_mb": memory_mb(), "gpus": gpus()})
}

#[cfg(test)]
mod tests {
    #[test]
    fn has_the_fields() {
        let v = super::collect();
        assert!(v["windows"].as_str().is_some_and(|s| !s.is_empty()));
        assert!(v.get("gpus").is_some() && v.get("cpu").is_some());
        if cfg!(windows) {
            assert!(v["windows"].as_str().unwrap().contains("Windows"), "{v}");
            assert!(v["memory_mb"].as_u64().unwrap_or(0) > 256, "{v}");
        }
    }
}
