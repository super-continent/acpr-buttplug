#![feature(naked_functions, abi_thiscall)]

use std::{
    ffi::{c_void, OsString},
    os::windows::prelude::OsStringExt,
};

use log::{debug, trace};
use once_cell::sync::OnceCell;
use windows::{
    core::{IUnknown, GUID, HRESULT, HSTRING},
    s,
    Win32::{
        Foundation::{BOOL, E_FAIL, HINSTANCE},
        System::{
            LibraryLoader::{GetProcAddress, LoadLibraryW},
            SystemInformation::GetSystemDirectoryW,
            SystemServices::DLL_PROCESS_ATTACH,
        },
    },
};

mod dll_code;
mod helpers;
mod hooks;
mod global;

#[no_mangle]
extern "system" fn DllMain(
    _dll_module: HINSTANCE,
    call_reason: u32,
    _reserved: *mut c_void,
) -> BOOL {
    match call_reason {
        DLL_PROCESS_ATTACH => {
            std::thread::spawn(dll_code::initialize);
        }
        _ => (),
    };

    true.into()
}

// Implementation for a dinput8.dll proxy, currently does not actually implement ShowJoyCPL

static REAL_DINPUT8_HANDLE: OnceCell<HINSTANCE> = OnceCell::new();

#[no_mangle]
pub unsafe extern "system" fn DirectInput8Create(
    inst_handle: HINSTANCE,
    version: u32,
    r_iid: *const GUID,
    ppv_out: *mut *mut c_void,
    p_unk_outer: *mut IUnknown,
) -> HRESULT {
    // type alias to make transmute cleaner
    type DInput8Create = extern "system" fn(
        HINSTANCE,
        u32,
        r_iid: *const GUID,
        *mut *mut c_void,
        *mut IUnknown,
    ) -> HRESULT;

    trace!("DirectInput8Create called");

    // Load real dinput8.dll if not already loaded
    let real_dinput8: HINSTANCE = *REAL_DINPUT8_HANDLE.get_or_init(|| get_dinput8_handle());

    let dinput8_create = GetProcAddress(real_dinput8, s!("DirectInput8Create"));

    if !real_dinput8.is_invalid() {
        if let Some(dinput8create) = dinput8_create {
            let dinput8create_fn = std::mem::transmute::<_, DInput8Create>(dinput8create);
            return dinput8create_fn(inst_handle, version, r_iid, ppv_out, p_unk_outer);
        }
    }

    E_FAIL // Unspecified failure
}

/// Get a handle to the real dinput8 library, if it fails it will return an invalid [`HINSTANCE`]
unsafe fn get_dinput8_handle() -> HINSTANCE {
    use windows::Win32::Foundation::MAX_PATH;

    const SYSTEM32_DEFAULT: &str = r"C:\Windows\System32";

    let mut buffer = [0u16; MAX_PATH as usize];
    let written_wchars = GetSystemDirectoryW(Some(&mut buffer));

    let system_directory = if written_wchars == 0 {
        SYSTEM32_DEFAULT.into()
    } else {
        // make sure path string does not contain extra trailing nulls
        let str_with_nulls = OsString::from_wide(&buffer)
            .into_string()
            .unwrap_or(SYSTEM32_DEFAULT.into());
        str_with_nulls.trim_matches('\0').to_string()
    };

    let dinput_path = system_directory + r"\dinput8.dll";
    debug!("Got real dinput8.dll path: `{}`", dinput_path);

    LoadLibraryW(&HSTRING::from(dinput_path)).unwrap_or_default()
}

#[no_mangle]
pub unsafe extern "system" fn ShowJoyCPL(_hwnd: windows::Win32::Foundation::HWND) {}
