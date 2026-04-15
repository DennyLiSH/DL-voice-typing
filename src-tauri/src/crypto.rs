use crate::error::AppError;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB,
};

/// Prefix for DPAPI-encrypted values stored in config.
const DPAPI_PREFIX: &str = "DPAPI:";

/// Encrypt a plaintext string using Windows DPAPI.
/// Returns a base64-encoded string prefixed with `DPAPI:`.
pub fn encrypt(plaintext: &str) -> Result<String, AppError> {
    let bytes = plaintext.as_bytes();
    let input_blob = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output_blob = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };

    let result = unsafe {
        CryptProtectData(
            &input_blob,
            None,
            None,
            None,
            None,
            0,
            &mut output_blob,
        )
    };

    if result.is_err() || output_blob.pbData.is_null() {
        return Err(AppError::Crypto("CryptProtectData failed".to_string()));
    }

    let encrypted_bytes =
        unsafe { std::slice::from_raw_parts(output_blob.pbData, output_blob.cbData as usize) };
    let encoded = format!("{}{}", DPAPI_PREFIX, BASE64.encode(encrypted_bytes));

    // Free the buffer allocated by DPAPI.
    unsafe {
        LocalFree(HLOCAL(output_blob.pbData as *mut core::ffi::c_void));
    }

    Ok(encoded)
}

/// Decrypt a DPAPI-encrypted string (base64 with `DPAPI:` prefix).
/// Returns the original plaintext.
pub fn decrypt(ciphertext: &str) -> Result<String, AppError> {
    let b64_data = ciphertext
        .strip_prefix(DPAPI_PREFIX)
        .ok_or_else(|| AppError::Crypto("missing DPAPI prefix".to_string()))?;

    let encrypted_bytes = BASE64
        .decode(b64_data)
        .map_err(|e| AppError::Crypto(format!("base64 decode failed: {}", e)))?;

    let mut input_blob = CRYPT_INTEGER_BLOB {
        cbData: encrypted_bytes.len() as u32,
        pbData: encrypted_bytes.as_ptr() as *mut u8,
    };
    let mut output_blob = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };

    let result = unsafe {
        CryptUnprotectData(
            &mut input_blob,
            None,
            None,
            None,
            None,
            0,
            &mut output_blob,
        )
    };

    if result.is_err() || output_blob.pbData.is_null() {
        return Err(AppError::Crypto("CryptUnprotectData failed".to_string()));
    }

    let plaintext_bytes =
        unsafe { std::slice::from_raw_parts(output_blob.pbData, output_blob.cbData as usize) };
    let plaintext = String::from_utf8(plaintext_bytes.to_vec())
        .map_err(|e| AppError::Crypto(format!("utf8 conversion failed: {}", e)))?;

    // Free the buffer allocated by DPAPI.
    unsafe {
        LocalFree(HLOCAL(output_blob.pbData as *mut core::ffi::c_void));
    }

    Ok(plaintext)
}

/// Check if a stored value is a DPAPI-encrypted blob (starts with `DPAPI:`).
pub fn is_encrypted(value: &str) -> bool {
    value.starts_with(DPAPI_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let original = "sk-test-api-key-12345";
        let encrypted = encrypt(original).unwrap();
        assert!(encrypted.starts_with("DPAPI:"));
        assert_ne!(encrypted, original);

        let decrypted = decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, original);
    }

    #[test]
    fn test_encrypt_empty_string() {
        let encrypted = encrypt("").unwrap();
        assert!(is_encrypted(&encrypted));
        let decrypted = decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, "");
    }

    #[test]
    fn test_is_encrypted_true() {
        let encrypted = encrypt("test").unwrap();
        assert!(is_encrypted(&encrypted));
    }

    #[test]
    fn test_is_encrypted_false() {
        assert!(!is_encrypted("sk-plain-text-key"));
        assert!(!is_encrypted(""));
        assert!(!is_encrypted("random-string"));
    }

    #[test]
    fn test_decrypt_invalid_base64() {
        let result = decrypt("DPAPI:!!!invalid!!!");
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_missing_prefix() {
        let result = decrypt("no-prefix-here");
        assert!(result.is_err());
    }

    #[test]
    fn test_encrypt_unicode() {
        let original = "密钥-日本語-한글";
        let encrypted = encrypt(original).unwrap();
        let decrypted = decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, original);
    }
}
