use std::ffi::{CStr, CString};
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::ptr;
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result, bail};
use libloading::{Library, Symbol};

unsafe extern "system" {
    fn SetDllDirectoryW(lpPathName: *const u16) -> i32;
}

// look for apple's DLLs in all the usual places.
// 3uTools ships its own copies so we check those too — saves people from
// having to install the full desktop iTunes just for the DLLs.
const APPLE_SUPPORT_DIRS: &[&str] = &[
    r"C:\Program Files\Common Files\Apple\Mobile Device Support",
    r"C:\Program Files (x86)\Common Files\Apple\Mobile Device Support",
    r"C:\Program Files\3uTools9\itunesFlashDll",
    r"C:\Program Files (x86)\3uTools9\itunesFlashDll",
    r"C:\Program Files\3uTools\itunesFlashDll",
    r"C:\Program Files (x86)\3uTools\itunesFlashDll",
];

// ---------- CoreFoundation types ----------
pub type CFTypeRef = *const std::ffi::c_void;
pub type CFStringRef = *const std::ffi::c_void;
pub type CFDataRef = *const std::ffi::c_void;
pub type CFDictionaryRef = *const std::ffi::c_void;
pub type CFArrayRef = *const std::ffi::c_void;
pub type CFPropertyListRef = *const std::ffi::c_void;
pub type CFAllocatorRef = *const std::ffi::c_void;
pub type CFIndex = isize;
pub type CFStringEncoding = u32;

pub const K_CFSTRING_ENCODING_UTF8: CFStringEncoding = 0x08000100;
pub const K_CFPROPERTY_LIST_BINARY_FORMAT_V1_0: isize = 200;

// ---------- MobileDevice / AFC types ----------
pub type ATHostConnectionRef = *mut std::ffi::c_void;
pub type AMDeviceRef = *const std::ffi::c_void;
pub type AMDeviceNotificationRef = *const std::ffi::c_void;
pub type AMDServiceConnectionRef = *mut std::ffi::c_void;
pub type AFCConnectionRef = *mut std::ffi::c_void;
pub type AFCKeyValueRef = *mut std::ffi::c_void;
pub type AFCFileRef = u64;
pub type AFCDirectoryRef = *mut std::ffi::c_void;

#[repr(C)]
pub struct AMDeviceNotificationCallbackInfo {
    pub device: AMDeviceRef,
    pub message: u32,
}

pub type AMDeviceNotificationCallback =
    extern "C" fn(*const AMDeviceNotificationCallbackInfo, *mut std::ffi::c_void);

/// All the function pointers we load from Apple's three DLLs.
/// Keeping them in one struct means we only do the dlopen dance once.
#[allow(dead_code)]
pub struct AppleLibraries {
    // prevent the libraries from being unloaded
    _cf_lib: Library,
    _md_lib: Library,
    _ath_lib: Library,

    // --- CoreFoundation ---
    pub cf_string_create: unsafe extern "C" fn(CFAllocatorRef, *const std::ffi::c_char, CFStringEncoding) -> CFStringRef,
    pub cf_string_get_length: unsafe extern "C" fn(CFStringRef) -> CFIndex,
    pub cf_string_get_max_size: unsafe extern "C" fn(CFIndex, CFStringEncoding) -> CFIndex,
    pub cf_string_get_c_string: unsafe extern "C" fn(CFStringRef, *mut std::ffi::c_char, CFIndex, CFStringEncoding) -> u8,
    pub cf_data_create: unsafe extern "C" fn(CFAllocatorRef, *const u8, CFIndex) -> CFDataRef,
    pub cf_data_get_byte_ptr: unsafe extern "C" fn(CFDataRef) -> *const u8,
    pub cf_data_get_length: unsafe extern "C" fn(CFDataRef) -> CFIndex,
    pub cf_property_list_create_with_data: unsafe extern "C" fn(CFAllocatorRef, CFDataRef, usize, *mut isize, *mut CFTypeRef) -> CFPropertyListRef,
    pub cf_property_list_create_data: unsafe extern "C" fn(CFAllocatorRef, CFPropertyListRef, isize, usize, *mut CFTypeRef) -> CFDataRef,
    pub cf_release: unsafe extern "C" fn(CFTypeRef),
    pub cf_retain: unsafe extern "C" fn(CFTypeRef) -> CFTypeRef,
    pub cf_equal: unsafe extern "C" fn(CFTypeRef, CFTypeRef) -> u32,
    pub cf_run_loop_get_main: unsafe extern "C" fn() -> *const std::ffi::c_void,
    pub cf_run_loop_run_in_mode: unsafe extern "C" fn(CFStringRef, f64, u8) -> i32,
    pub cf_run_loop_stop: unsafe extern "C" fn(*const std::ffi::c_void),

    // --- MobileDevice ---
    pub am_device_create_from_properties: unsafe extern "C" fn(CFDictionaryRef) -> AMDeviceRef,
    pub am_device_notification_subscribe: unsafe extern "C" fn(
        AMDeviceNotificationCallback, i32, u32, *mut std::ffi::c_void,
        *mut AMDeviceNotificationRef, CFDictionaryRef,
    ) -> i32,
    pub am_device_notification_unsubscribe: unsafe extern "C" fn(AMDeviceNotificationRef) -> i32,
    pub am_device_copy_device_identifier: unsafe extern "C" fn(AMDeviceRef) -> CFStringRef,
    pub am_device_copy_value: unsafe extern "C" fn(AMDeviceRef, CFStringRef, CFStringRef) -> CFTypeRef,
    pub am_device_connect: unsafe extern "C" fn(AMDeviceRef) -> i32,
    pub am_device_disconnect: unsafe extern "C" fn(AMDeviceRef) -> i32,
    pub am_device_is_paired: unsafe extern "C" fn(AMDeviceRef) -> i32,
    pub am_device_pair: unsafe extern "C" fn(AMDeviceRef) -> i32,
    pub am_device_validate_pairing: unsafe extern "C" fn(AMDeviceRef) -> i32,
    pub am_device_start_session: unsafe extern "C" fn(AMDeviceRef) -> i32,
    pub am_device_stop_session: unsafe extern "C" fn(AMDeviceRef) -> i32,
    pub am_device_secure_start_service: unsafe extern "C" fn(
        AMDeviceRef, CFStringRef, CFDictionaryRef, *mut AMDServiceConnectionRef,
    ) -> i32,
    pub amd_service_connection_get_socket: unsafe extern "C" fn(AMDServiceConnectionRef) -> i32,
    pub amd_service_connection_get_secure_io_context: unsafe extern "C" fn(AMDServiceConnectionRef) -> *mut std::ffi::c_void,
    pub amd_service_connection_invalidate: unsafe extern "C" fn(AMDServiceConnectionRef) -> i32,
    pub amd_service_connection_send: unsafe extern "C" fn(AMDServiceConnectionRef, *const u8, usize) -> i32,
    pub amd_service_connection_receive: unsafe extern "C" fn(AMDServiceConnectionRef, *mut u8, usize) -> i32,
    pub amd_service_connection_send_message: unsafe extern "C" fn(AMDServiceConnectionRef, CFTypeRef, isize) -> i32,
    pub amd_service_connection_receive_message: unsafe extern "C" fn(AMDServiceConnectionRef, *mut CFTypeRef, *mut isize) -> i32,

    // --- AFC ---
    pub afc_connection_open: unsafe extern "C" fn(i32, u32, *mut AFCConnectionRef) -> i32,
    pub afc_connection_close: unsafe extern "C" fn(AFCConnectionRef) -> i32,
    pub afc_connection_set_secure_context: unsafe extern "C" fn(AFCConnectionRef, *mut std::ffi::c_void) -> i32,
    pub afc_connection_set_dispose_secure_context: unsafe extern "C" fn(AFCConnectionRef, i32) -> i32,
    pub afc_connection_set_io_timeout: unsafe extern "C" fn(AFCConnectionRef, u32) -> i32,
    pub afc_file_info_open: unsafe extern "C" fn(AFCConnectionRef, *const std::ffi::c_char, *mut AFCKeyValueRef) -> i32,
    pub afc_key_value_read: unsafe extern "C" fn(AFCKeyValueRef, *mut *const std::ffi::c_char, *mut *const std::ffi::c_char) -> i32,
    pub afc_key_value_close: unsafe extern "C" fn(AFCKeyValueRef) -> i32,
    pub afc_file_ref_open: unsafe extern "C" fn(AFCConnectionRef, *const std::ffi::c_char, u64, *mut AFCFileRef) -> i32,
    pub afc_file_ref_read: unsafe extern "C" fn(AFCConnectionRef, AFCFileRef, *mut u8, *mut isize) -> i32,
    pub afc_file_ref_write: unsafe extern "C" fn(AFCConnectionRef, AFCFileRef, *const u8, isize) -> i32,
    pub afc_file_ref_close: unsafe extern "C" fn(AFCConnectionRef, AFCFileRef) -> i32,
    pub afc_directory_open: unsafe extern "C" fn(AFCConnectionRef, *const std::ffi::c_char, *mut AFCDirectoryRef) -> i32,
    pub afc_directory_read: unsafe extern "C" fn(AFCConnectionRef, AFCDirectoryRef, *mut *const std::ffi::c_char) -> i32,
    pub afc_directory_close: unsafe extern "C" fn(AFCConnectionRef, AFCDirectoryRef) -> i32,
    pub afc_directory_create: unsafe extern "C" fn(AFCConnectionRef, *const std::ffi::c_char) -> i32,
    pub afc_remove_path: unsafe extern "C" fn(AFCConnectionRef, *const std::ffi::c_char) -> i32,

    // --- AirTrafficHost ---
    pub at_host_connection_create: unsafe extern "C" fn(CFStringRef) -> ATHostConnectionRef,
    pub at_host_connection_release: unsafe extern "C" fn(ATHostConnectionRef),
    pub at_host_connection_send_host_info: unsafe extern "C" fn(ATHostConnectionRef, CFDictionaryRef),
    pub at_host_connection_send_sync_request: unsafe extern "C" fn(ATHostConnectionRef, CFArrayRef, CFDictionaryRef, CFDictionaryRef),
    pub at_host_connection_send_metadata_sync_finished: unsafe extern "C" fn(ATHostConnectionRef, CFDictionaryRef, CFDictionaryRef),
    pub at_host_connection_send_asset_completed: unsafe extern "C" fn(ATHostConnectionRef, CFStringRef, CFStringRef, CFStringRef),
    pub at_host_connection_read_message: unsafe extern "C" fn(ATHostConnectionRef) -> CFDictionaryRef,
    pub at_cf_message_get_name: unsafe extern "C" fn(CFDictionaryRef) -> CFStringRef,
    pub at_cf_message_get_param: unsafe extern "C" fn(CFDictionaryRef, CFStringRef) -> CFTypeRef,
}

static LIBRARIES: OnceLock<Arc<AppleLibraries>> = OnceLock::new();

pub fn locate_support_dir() -> Option<PathBuf> {
    APPLE_SUPPORT_DIRS
        .iter()
        .map(PathBuf::from)
        .find(|dir| {
            dir.join("CoreFoundation.dll").is_file()
                && dir.join("MobileDevice.dll").is_file()
                && dir.join("AirTrafficHost.dll").is_file()
        })
}

pub fn get_apple_libraries() -> Result<Arc<AppleLibraries>> {
    if let Some(libs) = LIBRARIES.get() {
        return Ok(Arc::clone(libs));
    }

    let dir = locate_support_dir().context(
        "Apple Mobile Device Support not found. Install iTunes (desktop version) from apple.com.",
    )?;

    // tell Windows where to find the satellite DLLs (objc.dll, pthread, etc)
    let wide_dir: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    unsafe {
        SetDllDirectoryW(wide_dir.as_ptr());
    }

    let cf_path = dir.join("CoreFoundation.dll");
    let md_path = dir.join("MobileDevice.dll");
    let ath_path = dir.join("AirTrafficHost.dll");

    unsafe {
        let cf_lib = Library::new(&cf_path).context("Failed to load CoreFoundation.dll")?;
        let md_lib = Library::new(&md_path).context("Failed to load MobileDevice.dll")?;
        let ath_lib = Library::new(&ath_path).context("Failed to load AirTrafficHost.dll")?;

        macro_rules! sym {
            ($lib:expr, $name:expr) => {{
                let s: Symbol<_> = $lib.get($name.as_bytes()).context(concat!("Missing: ", $name))?;
                *s
            }};
        }

        let apple_libs = Arc::new(AppleLibraries {
            // CoreFoundation
            cf_string_create: sym!(cf_lib, "CFStringCreateWithCString"),
            cf_string_get_length: sym!(cf_lib, "CFStringGetLength"),
            cf_string_get_max_size: sym!(cf_lib, "CFStringGetMaximumSizeForEncoding"),
            cf_string_get_c_string: sym!(cf_lib, "CFStringGetCString"),
            cf_data_create: sym!(cf_lib, "CFDataCreate"),
            cf_data_get_byte_ptr: sym!(cf_lib, "CFDataGetBytePtr"),
            cf_data_get_length: sym!(cf_lib, "CFDataGetLength"),
            cf_property_list_create_with_data: sym!(cf_lib, "CFPropertyListCreateWithData"),
            cf_property_list_create_data: sym!(cf_lib, "CFPropertyListCreateData"),
            cf_release: sym!(cf_lib, "CFRelease"),
            cf_retain: sym!(cf_lib, "CFRetain"),
            cf_equal: sym!(cf_lib, "CFEqual"),
            cf_run_loop_get_main: sym!(cf_lib, "CFRunLoopGetMain"),
            cf_run_loop_run_in_mode: sym!(cf_lib, "CFRunLoopRunInMode"),
            cf_run_loop_stop: sym!(cf_lib, "CFRunLoopStop"),

            // MobileDevice
            am_device_create_from_properties: sym!(md_lib, "AMDeviceCreateFromProperties"),
            am_device_notification_subscribe: sym!(md_lib, "AMDeviceNotificationSubscribeWithOptions"),
            am_device_notification_unsubscribe: sym!(md_lib, "AMDeviceNotificationUnsubscribe"),
            am_device_copy_device_identifier: sym!(md_lib, "AMDeviceCopyDeviceIdentifier"),
            am_device_copy_value: sym!(md_lib, "AMDeviceCopyValue"),
            am_device_connect: sym!(md_lib, "AMDeviceConnect"),
            am_device_disconnect: sym!(md_lib, "AMDeviceDisconnect"),
            am_device_is_paired: sym!(md_lib, "AMDeviceIsPaired"),
            am_device_pair: sym!(md_lib, "AMDevicePair"),
            am_device_validate_pairing: sym!(md_lib, "AMDeviceValidatePairing"),
            am_device_start_session: sym!(md_lib, "AMDeviceStartSession"),
            am_device_stop_session: sym!(md_lib, "AMDeviceStopSession"),
            am_device_secure_start_service: sym!(md_lib, "AMDeviceSecureStartService"),
            amd_service_connection_get_socket: sym!(md_lib, "AMDServiceConnectionGetSocket"),
            amd_service_connection_get_secure_io_context: sym!(md_lib, "AMDServiceConnectionGetSecureIOContext"),
            amd_service_connection_invalidate: sym!(md_lib, "AMDServiceConnectionInvalidate"),
            amd_service_connection_send: sym!(md_lib, "AMDServiceConnectionSend"),
            amd_service_connection_receive: sym!(md_lib, "AMDServiceConnectionReceive"),
            amd_service_connection_send_message: sym!(md_lib, "AMDServiceConnectionSendMessage"),
            amd_service_connection_receive_message: sym!(md_lib, "AMDServiceConnectionReceiveMessage"),

            // AFC
            afc_connection_open: sym!(md_lib, "AFCConnectionOpen"),
            afc_connection_close: sym!(md_lib, "AFCConnectionClose"),
            afc_connection_set_secure_context: sym!(md_lib, "AFCConnectionSetSecureContext"),
            afc_connection_set_dispose_secure_context: sym!(md_lib, "AFCConnectionSetDisposeSecureContextOnInvalidate"),
            afc_connection_set_io_timeout: sym!(md_lib, "AFCConnectionSetIOTimeout"),
            afc_file_info_open: sym!(md_lib, "AFCFileInfoOpen"),
            afc_key_value_read: sym!(md_lib, "AFCKeyValueRead"),
            afc_key_value_close: sym!(md_lib, "AFCKeyValueClose"),
            afc_file_ref_open: sym!(md_lib, "AFCFileRefOpen"),
            afc_file_ref_read: sym!(md_lib, "AFCFileRefRead"),
            afc_file_ref_write: sym!(md_lib, "AFCFileRefWrite"),
            afc_file_ref_close: sym!(md_lib, "AFCFileRefClose"),
            afc_directory_open: sym!(md_lib, "AFCDirectoryOpen"),
            afc_directory_read: sym!(md_lib, "AFCDirectoryRead"),
            afc_directory_close: sym!(md_lib, "AFCDirectoryClose"),
            afc_directory_create: sym!(md_lib, "AFCDirectoryCreate"),
            afc_remove_path: sym!(md_lib, "AFCRemovePath"),

            // AirTrafficHost
            at_host_connection_create: sym!(ath_lib, "ATHostConnectionCreate"),
            at_host_connection_release: sym!(ath_lib, "ATHostConnectionRelease"),
            at_host_connection_send_host_info: sym!(ath_lib, "ATHostConnectionSendHostInfo"),
            at_host_connection_send_sync_request: sym!(ath_lib, "ATHostConnectionSendSyncRequest"),
            at_host_connection_send_metadata_sync_finished: sym!(ath_lib, "ATHostConnectionSendMetadataSyncFinished"),
            at_host_connection_send_asset_completed: sym!(ath_lib, "ATHostConnectionSendAssetCompleted"),
            at_host_connection_read_message: sym!(ath_lib, "ATHostConnectionReadMessage"),
            at_cf_message_get_name: sym!(ath_lib, "ATCFMessageGetName"),
            at_cf_message_get_param: sym!(ath_lib, "ATCFMessageGetParam"),

            _cf_lib: cf_lib,
            _md_lib: md_lib,
            _ath_lib: ath_lib,
        });

        let _ = LIBRARIES.set(Arc::clone(&apple_libs));
        Ok(apple_libs)
    }
}

// RAII guard for CFString so we don't leak references
pub struct CFStringGuard {
    pub raw: CFStringRef,
    libs: Arc<AppleLibraries>,
}

impl Drop for CFStringGuard {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe { (self.libs.cf_release)(self.raw); }
        }
    }
}

pub struct CFTypeGuard {
    pub raw: CFTypeRef,
    libs: Arc<AppleLibraries>,
}

impl Drop for CFTypeGuard {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe { (self.libs.cf_release)(self.raw); }
        }
    }
}

impl AppleLibraries {
    pub fn create_cf_string(&self, s: &str) -> Result<CFStringGuard> {
        let c_str = CString::new(s).context("String contains null byte")?;
        let raw = unsafe {
            (self.cf_string_create)(ptr::null(), c_str.as_ptr(), K_CFSTRING_ENCODING_UTF8)
        };
        if raw.is_null() {
            bail!("CFStringCreateWithCString failed for '{}'", s);
        }
        Ok(CFStringGuard { raw, libs: get_apple_libraries()? })
    }

    pub fn to_rust_string(&self, cf_str: CFStringRef) -> String {
        if cf_str.is_null() {
            return String::new();
        }
        unsafe {
            let length = (self.cf_string_get_length)(cf_str);
            let max_bytes = (self.cf_string_get_max_size)(length, K_CFSTRING_ENCODING_UTF8) + 1;
            let mut buf: Vec<u8> = vec![0; max_bytes as usize];
            if (self.cf_string_get_c_string)(
                cf_str, buf.as_mut_ptr() as *mut std::ffi::c_char,
                max_bytes, K_CFSTRING_ENCODING_UTF8,
            ) != 0 {
                if let Ok(s) = CStr::from_ptr(buf.as_ptr() as *const std::ffi::c_char).to_str() {
                    return s.to_owned();
                }
            }
        }
        String::new()
    }

    pub fn create_cf_plist_from_bytes(&self, bytes: &[u8]) -> Result<CFTypeGuard> {
        unsafe {
            let cf_data = (self.cf_data_create)(ptr::null(), bytes.as_ptr(), bytes.len() as isize);
            if cf_data.is_null() {
                bail!("CFDataCreate returned null");
            }
            let plist = (self.cf_property_list_create_with_data)(
                ptr::null(), cf_data, 0, ptr::null_mut(), ptr::null_mut(),
            );
            (self.cf_release)(cf_data);
            if plist.is_null() {
                bail!("Failed to parse binary plist");
            }
            Ok(CFTypeGuard { raw: plist, libs: get_apple_libraries()? })
        }
    }

    pub fn cf_plist_to_bytes(&self, plist: CFPropertyListRef) -> Result<Vec<u8>> {
        if plist.is_null() {
            bail!("Null plist reference");
        }
        unsafe {
            let cf_data = (self.cf_property_list_create_data)(
                ptr::null(), plist, K_CFPROPERTY_LIST_BINARY_FORMAT_V1_0, 0, ptr::null_mut(),
            );
            if cf_data.is_null() {
                bail!("Failed to serialize plist");
            }
            let ptr = (self.cf_data_get_byte_ptr)(cf_data);
            let len = (self.cf_data_get_length)(cf_data) as usize;
            let result = std::slice::from_raw_parts(ptr, len).to_vec();
            (self.cf_release)(cf_data);
            Ok(result)
        }
    }
}

pub fn verify_support() -> Result<String> {
    let _libs = get_apple_libraries()?;
    let dir = locate_support_dir().unwrap_or_default();
    Ok(format!("Apple Mobile Device Support loaded from {}", dir.display()))
}
