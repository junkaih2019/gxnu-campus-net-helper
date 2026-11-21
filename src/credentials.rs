//! Windows 凭据管理器读写。

use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
use windows_sys::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
    CRED_TYPE_GENERIC,
};

/// Win32 的 `CRED_MAX_CREDENTIAL_BLOB_SIZE`（固定 5 × 512 字节）。
const CRED_MAX_CREDENTIAL_BLOB_SIZE: usize = 5 * 512;
// This marker identifies credentials encoded as UTF-16LE.
const UTF16_COMMENT: &str = "GXNU campus network login tool";

/// 把 &str 转成以 NUL 结尾的 UTF-16。
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[derive(Clone)]
pub struct StoredCredential {
    pub username: String,
    pub password: String,
}

impl StoredCredential {
    pub fn matches_account(&self, configured_id: &str) -> bool {
        !self.username.is_empty()
            && !self.password.is_empty()
            && (configured_id.is_empty() || self.username == configured_id)
    }
}

/// 读取凭据；不存在返回 `Ok(None)`，其它错误返回 `Err`。
pub fn load(target: &str) -> Result<Option<StoredCredential>, String> {
    let target_w = wide(target);
    let mut credential_ptr: *mut CREDENTIALW = null_mut();
    // SAFETY: 指针与长度由我们提供，成功后必须 CredFree。
    let ok = unsafe { CredReadW(target_w.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential_ptr) };
    if ok == 0 {
        let err = unsafe { GetLastError() };
        if err == ERROR_NOT_FOUND {
            return Ok(None);
        }
        return Err(format!("CredReadW 失败（错误码 {err}）"));
    }
    let credential = unsafe { &*credential_ptr };
    let username = unsafe { from_wide(credential.UserName) };
    let marked_utf16 = unsafe { from_wide(credential.Comment) } == UTF16_COMMENT;
    let size = credential.CredentialBlobSize as usize;
    let blob = if credential.CredentialBlob.is_null() || size == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(credential.CredentialBlob, size) }.to_vec()
    };
    unsafe { CredFree(credential_ptr as *const _) };
    let password = decode_credential_blob(&blob, marked_utf16)?;
    Ok(Some(StoredCredential { username, password }))
}

pub fn save(target: &str, username: &str, password: &str) -> Result<(), String> {
    let target_w = wide(target);
    let username_w = wide(username);
    let comment = wide(UTF16_COMMENT);
    // 使用 UTF-16-LE 编码写入。
    let mut blob = encode_credential_blob(password);
    if blob.len() > CRED_MAX_CREDENTIAL_BLOB_SIZE {
        return Err("密码过长".to_string());
    }
    let mut credential: CREDENTIALW = unsafe { std::mem::zeroed() };
    credential.Type = CRED_TYPE_GENERIC;
    credential.TargetName = target_w.as_ptr() as *mut u16;
    credential.Comment = comment.as_ptr() as *mut u16;
    credential.UserName = username_w.as_ptr() as *mut u16;
    credential.CredentialBlobSize = blob.len() as u32;
    credential.CredentialBlob = blob.as_mut_ptr();
    credential.Persist = CRED_PERSIST_LOCAL_MACHINE;
    let ok = unsafe { CredWriteW(&credential, 0) };
    if ok == 0 {
        let err = unsafe { GetLastError() };
        return Err(format!("CredWriteW 失败（错误码 {err}）"));
    }
    Ok(())
}

fn encode_credential_blob(password: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(password.len() * 2);
    for unit in password.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

/// 优先按 UTF-16-LE 解码，
/// 同时兼容旧版 Rust 误按 UTF-8 写入的凭据。
fn decode_credential_blob(blob: &[u8], marked_utf16: bool) -> Result<String, String> {
    // Older Rust builds wrote unmarked UTF-8 and UTF-16. Preserve their legacy
    // decoding rule only for blobs without the encoding marker.
    if !marked_utf16 && !blob.contains(&0) {
        if let Ok(legacy) = std::str::from_utf8(blob) {
            return Ok(legacy.to_owned());
        }
    }
    let (pairs, remainder) = blob.as_chunks::<2>();
    if !remainder.is_empty() {
        return Err("保存的密码编码无效，请重新保存密码".into());
    }
    let units: Vec<_> = pairs.iter().copied().map(u16::from_le_bytes).collect();
    String::from_utf16(&units)
        .map(|text| text.trim_end_matches('\0').to_owned())
        .map_err(|_| "保存的密码编码无效，请重新保存密码".into())
}

pub fn delete(target: &str) -> Result<(), String> {
    let target_w = wide(target);
    let ok = unsafe { CredDeleteW(target_w.as_ptr(), CRED_TYPE_GENERIC, 0) };
    if ok == 0 {
        let err = unsafe { GetLastError() };
        if err == ERROR_NOT_FOUND {
            return Ok(());
        }
        return Err(format!("CredDeleteW 失败（错误码 {err}）"));
    }
    Ok(())
}

unsafe fn from_wide(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    while unsafe { *ptr.add(len) } != 0 {
        len += 1;
    }
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
    String::from_utf16_lossy(slice)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16le_credentials_roundtrip() {
        let blob = encode_credential_blob("Abc@123456!");
        assert_eq!(blob.len(), 22);
        assert!(blob.contains(&0));
        assert_eq!(decode_credential_blob(&blob, true).unwrap(), "Abc@123456!");
    }

    #[test]
    fn utf16le_handles_unicode_and_trailing_nul() {
        let mut blob = encode_credential_blob("密码123");
        blob.extend_from_slice(&[0, 0]);
        assert_eq!(decode_credential_blob(&blob, true).unwrap(), "密码123");
    }

    #[test]
    fn legacy_utf8_blob_is_still_readable() {
        let legacy = b"OldUtf8Pass!".to_vec();
        assert_eq!(
            decode_credential_blob(&legacy, false).unwrap(),
            "OldUtf8Pass!"
        );
    }

    #[test]
    fn marked_unicode_roundtrips_even_when_bytes_are_valid_ascii() {
        let blob = encode_credential_blob("中山");
        assert!(std::str::from_utf8(&blob).is_ok());
        assert_eq!(decode_credential_blob(&blob, true).unwrap(), "中山");
    }
}
