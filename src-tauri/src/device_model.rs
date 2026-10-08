//! This device's hardware model in plain words ("iPhone 15", "MacBook Air",
//! "Surface Laptop 5"), so two of the user's own devices that would both read
//! "Your iPhone" can read "Your iPhone 15" / "Your iPhone 12" instead.
//!
//! It rides, optional, in the device hello, the link reply and the account
//! roster (`device_model`); older builds neither send nor need it.

use std::sync::OnceLock;

/// The friendly model name of THIS device, worked out once. None when the
/// platform doesn't say (the UI then just uses "iPhone" / "Mac" / "PC").
pub(crate) fn this_model() -> Option<String> {
    static MODEL: OnceLock<Option<String>> = OnceLock::new();
    MODEL.get_or_init(|| detect().and_then(|m| clean(&m))).clone()
}

/// A model name as received from another device: trimmed, printable, short.
pub(crate) fn clean(raw: &str) -> Option<String> {
    let s: String = raw.chars().filter(|c| !c.is_control()).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
    if s.is_empty() || s.chars().count() > 40 { return None; }
    Some(s)
}

#[cfg(target_os = "macos")]
fn detect() -> Option<String> {
    let out = std::process::Command::new("/usr/sbin/sysctl").args(["-n", "hw.model"]).output().ok()?;
    Some(mac_name(String::from_utf8_lossy(&out.stdout).trim()))
}

#[cfg(target_os = "ios")]
fn detect() -> Option<String> {
    // The simulator reports the Mac's CPU ("arm64"); it names the device it plays.
    if let Ok(sim) = std::env::var("SIMULATOR_MODEL_IDENTIFIER") { return Some(ios_name(&sim)); }
    let mut u: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut u) } != 0 { return None; }
    let machine = unsafe { std::ffi::CStr::from_ptr(u.machine.as_ptr()) }.to_string_lossy().into_owned();
    Some(ios_name(&machine))
}

#[cfg(target_os = "linux")]
fn detect() -> Option<String> {
    let read = |f: &str| std::fs::read_to_string(format!("/sys/class/dmi/id/{f}")).unwrap_or_default();
    pc_name(&read("sys_vendor"), &read("product_name"), &read("product_version"), &read("product_family"))
}

#[cfg(target_os = "windows")]
fn detect() -> Option<String> {
    pc_name(&win_bios("SystemManufacturer"), &win_bios("SystemProductName"), &win_bios("SystemVersion"), &win_bios("SystemFamily"))
}

#[cfg(target_os = "windows")]
fn win_bios(value: &str) -> String {
    #[link(name = "advapi32")]
    extern "system" {
        fn RegGetValueW(key: isize, sub: *const u16, value: *const u16, flags: u32, kind: *mut u32,
            data: *mut core::ffi::c_void, len: *mut u32) -> i32;
    }
    const HKEY_LOCAL_MACHINE: isize = 0x8000_0002u32 as i32 as isize;
    const RRF_RT_REG_SZ: u32 = 0x2;
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (sub, name) = (wide(r"HARDWARE\DESCRIPTION\System\BIOS"), wide(value));
    let mut buf = [0u16; 128];
    let mut len = (buf.len() * 2) as u32;
    let rc = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, sub.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(),
        buf.as_mut_ptr().cast(), &mut len) };
    if rc != 0 { return String::new(); }
    let n = (len as usize / 2).min(buf.len());
    String::from_utf16_lossy(&buf[..n]).trim_end_matches('\0').to_owned()
}

#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "linux", target_os = "windows")))]
fn detect() -> Option<String> { None }

/// `sysctl hw.model` → "MacBook Air", "Mac mini"… ("Mac" when unknown).
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) fn mac_name(id: &str) -> String {
    // Older style: the family is the identifier's name part.
    for (prefix, name) in [("MacBookAir", "MacBook Air"), ("MacBookPro", "MacBook Pro"), ("MacBook", "MacBook"),
        ("Macmini", "Mac mini"), ("MacPro", "Mac Pro"), ("iMacPro", "iMac Pro"), ("iMac", "iMac")] {
        if id.strip_prefix(prefix).is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit())) { return name.into(); }
    }
    // Apple silicon: "MacNN,M" says nothing by itself.
    let name = match id {
        "Mac13,1" | "Mac13,2" | "Mac14,13" | "Mac14,14" | "Mac15,14" | "Mac16,9" => "Mac Studio",
        "Mac14,2" | "Mac14,15" | "Mac15,12" | "Mac15,13" | "Mac16,12" | "Mac16,13" => "MacBook Air",
        "Mac14,3" | "Mac14,12" | "Mac16,10" | "Mac16,11" => "Mac mini",
        "Mac14,5" | "Mac14,6" | "Mac14,7" | "Mac14,9" | "Mac14,10" | "Mac15,3" | "Mac15,6" | "Mac15,7" | "Mac15,8"
        | "Mac15,9" | "Mac15,10" | "Mac15,11" | "Mac16,1" | "Mac16,5" | "Mac16,6" | "Mac16,7" | "Mac16,8" => "MacBook Pro",
        "Mac14,8" => "Mac Pro",
        "Mac15,4" | "Mac15,5" | "Mac16,2" | "Mac16,3" => "iMac",
        _ => "Mac",
    };
    name.into()
}

/// utsname machine → "iPhone 15 Pro", "iPad"… ("iPhone" / "iPad" when unknown).
#[cfg_attr(not(any(target_os = "ios", test)), allow(dead_code))]
pub(crate) fn ios_name(id: &str) -> String {
    let name = match id {
        "iPhone10,1" | "iPhone10,4" => "iPhone 8",
        "iPhone10,2" | "iPhone10,5" => "iPhone 8 Plus",
        "iPhone10,3" | "iPhone10,6" => "iPhone X",
        "iPhone11,2" => "iPhone XS",
        "iPhone11,4" | "iPhone11,6" => "iPhone XS Max",
        "iPhone11,8" => "iPhone XR",
        "iPhone12,1" => "iPhone 11",
        "iPhone12,3" => "iPhone 11 Pro",
        "iPhone12,5" => "iPhone 11 Pro Max",
        "iPhone12,8" | "iPhone14,6" => "iPhone SE",
        "iPhone13,1" => "iPhone 12 mini",
        "iPhone13,2" => "iPhone 12",
        "iPhone13,3" => "iPhone 12 Pro",
        "iPhone13,4" => "iPhone 12 Pro Max",
        "iPhone14,4" => "iPhone 13 mini",
        "iPhone14,5" => "iPhone 13",
        "iPhone14,2" => "iPhone 13 Pro",
        "iPhone14,3" => "iPhone 13 Pro Max",
        "iPhone14,7" => "iPhone 14",
        "iPhone14,8" => "iPhone 14 Plus",
        "iPhone15,2" => "iPhone 14 Pro",
        "iPhone15,3" => "iPhone 14 Pro Max",
        "iPhone15,4" => "iPhone 15",
        "iPhone15,5" => "iPhone 15 Plus",
        "iPhone16,1" => "iPhone 15 Pro",
        "iPhone16,2" => "iPhone 15 Pro Max",
        "iPhone17,3" => "iPhone 16",
        "iPhone17,4" => "iPhone 16 Plus",
        "iPhone17,1" => "iPhone 16 Pro",
        "iPhone17,2" => "iPhone 16 Pro Max",
        "iPhone17,5" => "iPhone 16e",
        "iPhone18,3" => "iPhone 17",
        "iPhone18,1" => "iPhone 17 Pro",
        "iPhone18,2" => "iPhone 17 Pro Max",
        "iPhone18,4" => "iPhone Air",
        _ if id.starts_with("iPad") => "iPad",
        _ if id.starts_with("iPod") => "iPod touch",
        _ => "iPhone",
    };
    name.into()
}

/// Windows/Linux firmware fields → "Surface Laptop 5", "ThinkPad X1 Carbon Gen 9",
/// "XPS 13 9310"… None when the maker left placeholders in.
#[cfg_attr(not(any(target_os = "windows", target_os = "linux", test)), allow(dead_code))]
pub(crate) fn pc_name(vendor: &str, product: &str, version: &str, family: &str) -> Option<String> {
    let real = |s: &str| -> Option<String> {
        let s = s.trim();
        let l = s.to_ascii_lowercase();
        let junk = s.len() < 2 || ["to be filled", "system product", "system version", "default string", "not applicable",
            "not specified", "none", "unknown", "o.e.m", "oem", "invalid", "type1", "sku", "x.x", "123456789"]
            .iter().any(|j| l.contains(j));
        (!junk).then(|| s.to_owned())
    };
    // A bare part number ("20XWCTO1WW", "81YK") means little to anyone.
    let readable = |s: &String| s.contains(' ') || !s.chars().any(|c| c.is_ascii_digit());
    let vendor_l = vendor.trim().to_ascii_lowercase();
    // Lenovo keeps the readable name in the version field.
    let name = [product, version, family].into_iter().filter_map(real).find(readable)?;
    // Virtual machines aren't a model anyone would recognise.
    if ["virtualbox", "vmware", "kvm", "qemu", "virtual machine", "standard pc"].iter().any(|v| name.to_ascii_lowercase().contains(v)) {
        return None;
    }
    // "HP" alone before "EliteBook 840" reads better; skip when already there.
    let short_vendor = match vendor_l.as_str() { v if v.starts_with("hewlett") || v == "hp" => Some("HP"), _ => None };
    match short_vendor {
        Some(v) if !name.starts_with(v) => clean(&format!("{v} {name}")),
        _ => clean(&name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macs_read_as_their_family() {
        assert_eq!(mac_name("Mac14,2"), "MacBook Air");
        assert_eq!(mac_name("Mac14,3"), "Mac mini");
        assert_eq!(mac_name("Mac16,1"), "MacBook Pro");
        assert_eq!(mac_name("Mac13,1"), "Mac Studio");
        assert_eq!(mac_name("MacBookAir10,1"), "MacBook Air");
        assert_eq!(mac_name("MacBookPro18,3"), "MacBook Pro");
        assert_eq!(mac_name("Macmini9,1"), "Mac mini");
        assert_eq!(mac_name("iMac21,1"), "iMac");
        assert_eq!(mac_name("iMacPro1,1"), "iMac Pro");
        assert_eq!(mac_name("Mac99,9"), "Mac");
        assert_eq!(mac_name(""), "Mac");
    }

    #[test]
    fn iphones_read_as_their_marketing_name() {
        assert_eq!(ios_name("iPhone15,2"), "iPhone 14 Pro");
        assert_eq!(ios_name("iPhone15,4"), "iPhone 15");
        assert_eq!(ios_name("iPhone13,2"), "iPhone 12");
        assert_eq!(ios_name("iPhone17,1"), "iPhone 16 Pro");
        assert_eq!(ios_name("iPhone99,1"), "iPhone");
        assert_eq!(ios_name("iPad13,4"), "iPad");
        assert_eq!(ios_name("arm64"), "iPhone");
    }

    #[test]
    fn pcs_use_firmware_names_and_skip_placeholders() {
        assert_eq!(pc_name("Microsoft Corporation", "Surface Laptop 5", "", "Surface").as_deref(), Some("Surface Laptop 5"));
        assert_eq!(pc_name("LENOVO", "20XWCTO1WW", "ThinkPad X1 Carbon Gen 9", "ThinkPad X1").as_deref(), Some("ThinkPad X1 Carbon Gen 9"));
        assert_eq!(pc_name("Dell Inc.", "XPS 13 9310", "", "XPS").as_deref(), Some("XPS 13 9310"));
        assert_eq!(pc_name("HP", "EliteBook 840 G8", "", "").as_deref(), Some("HP EliteBook 840 G8"));
        assert_eq!(pc_name("ASUS", "System Product Name", "System Version", "To Be Filled By O.E.M.").as_deref(), None);
        assert_eq!(pc_name("innotek GmbH", "VirtualBox", "1.2", "Virtual Machine"), None);
        assert_eq!(pc_name("", "", "", ""), None);
    }

    #[test]
    fn received_names_are_cleaned() {
        assert_eq!(clean("  iPhone\n 15 ").as_deref(), Some("iPhone 15"));
        assert_eq!(clean(""), None);
        assert_eq!(clean(&"x".repeat(41)), None);
    }
}
