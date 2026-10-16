//! DPAPI port of ProtectedData usage in SettingsManager.cs — CurrentUser scope,
//! Base64 transport, swallow-all on unprotect (returns empty string, like the source).

use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CryptProtectData, CryptUnprotectData,
};

/// `ProtectedData.Protect(utf8(key), null, CurrentUser)`.
///
/// `DataProtectionScope.CurrentUser` is `CryptProtectData` with `dwFlags = 0` and
/// no prompt struct, so the resulting blob is readable only inside the same user
/// session and never pops a DPAPI UI.
pub fn protect(data: &[u8]) -> std::io::Result<Vec<u8>> {
    if data.len() > u32::MAX as usize {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "payload too large for a DPAPI blob",
        ));
    }

    // A null `pbData` with `cbData = 0` is a valid (empty) input blob.
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr().cast_mut(),
    };

    let mut output = CRYPT_INTEGER_BLOB::default();

    // SAFETY: `input.pbData` borrows `data` (length `cbData`) and `output` is a
    // stack local; both outlive the call, and the descriptor pointers are only
    // read by DPAPI, never retained past the return.
    unsafe { CryptProtectData(&input, None, None, None, None, 0, &mut output) }
        .map_err(to_io_error)?;

    // SAFETY: on success `output` describes a DPAPI-allocated buffer of exactly
    // `output.cbData` bytes.
    let result = unsafe { copy_out_blob(&output) };

    unsafe { release_blob(output) };

    Ok(result)
}

/// `ProtectedData.Unprotect` — Err on failure so the caller can fall back to empty.
///
/// `DataProtectionScope.CurrentUser` is again a plain `CryptUnprotectData` with no
/// prompt struct; failures (wrong user, corrupted blob) surface as `Err`.
pub fn unprotect(data: &[u8]) -> std::io::Result<Vec<u8>> {
    if data.len() > u32::MAX as usize {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "payload too large for a DPAPI blob",
        ));
    }

    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr().cast_mut(),
    };

    let mut output = CRYPT_INTEGER_BLOB::default();

    // SAFETY: same invariant as `protect` — the input borrows `data` for the call
    // and `output` is a stack local that is released before returning.
    unsafe { CryptUnprotectData(&input, None, None, None, None, 0, &mut output) }
        .map_err(to_io_error)?;

    // SAFETY: on success `output` describes a DPAPI-allocated buffer.
    let result = unsafe { copy_out_blob(&output) };

    unsafe { release_blob(output) };

    Ok(result)
}

/// Copy a DPAPI out-blob into an owned `Vec` before its owner frees it.
unsafe fn copy_out_blob(blob: &CRYPT_INTEGER_BLOB) -> Vec<u8> {
    let len = blob.cbData as usize;
    let mut result = Vec::with_capacity(len);
    // SAFETY: `blob.pbData` points at `len` readable bytes, `result` has room for
    // them and the regions cannot overlap because `result` is freshly allocated.
    unsafe {
        std::ptr::copy_nonoverlapping(blob.pbData, result.as_mut_ptr(), len);
        result.set_len(len);
    }
    result
}

/// Free a DPAPI out-blob with the same allocator `CryptProtectData`/`CryptUnprotectData` used.
unsafe fn release_blob(blob: CRYPT_INTEGER_BLOB) {
    if blob.cbData == 0 || blob.pbData.is_null() {
        return;
    }
    // SAFETY: `blob.pbData` came from DPAPI and has not been freed yet; DPAPI uses
    // the local heap, which `LocalFree` releases.
    unsafe {
        LocalFree(Some(HLOCAL(blob.pbData.cast())));
    }
}

/// Map a `windows::core::Error` onto a `std::io::Error`, keeping the Win32 code when
/// the HRESULT wraps one (`HRESULT_FROM_WIN32`) so callers can branch on the reason.
fn to_io_error(error: windows::core::Error) -> std::io::Error {
    const FACILITY_WIN32_BIT: u32 = 0x0007;
    const SEVERITY_ERROR_BIT: u32 = 0x8000_0000;
    let raw = error.code().0 as u32;
    let code = if raw & (SEVERITY_ERROR_BIT | (FACILITY_WIN32_BIT << 16))
        == (SEVERITY_ERROR_BIT | (FACILITY_WIN32_BIT << 16))
    {
        (raw & 0xFFFF) as i32
    } else {
        raw as i32
    };
    std::io::Error::from_raw_os_error(code)
}
