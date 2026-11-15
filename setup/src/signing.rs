//! Signs HomeBar.exe (uiAccess needs a trusted signature) with a certificate trusted only on this PC.
//! The key exists only in memory while signing, so nothing else can ever be signed with it.

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::Security::Cryptography::*;
use windows_sys::Win32::System::SystemInformation::GetSystemTime;

use crate::win::{check, last_error, wide, Result};

const SUBJECT: &str = "CN=HomeBar local signing";
const SUBJECT_NAME: &str = "HomeBar local signing";
const TRUSTED_STORES: [&str; 2] = ["Root", "TrustedPublisher"];
/// Also cleaned up: the PowerShell installer left its (keyless) certificate here.
const ALL_STORES: [&str; 3] = ["Root", "TrustedPublisher", "My"];
const OID_CODE_SIGNING: &[u8] = b"1.3.6.1.5.5.7.3.3\0";
const OID_ENHANCED_KEY_USAGE: &[u8] = b"2.5.29.37\0";
const OID_SHA256_RSA: &[u8] = b"1.2.840.113549.1.1.11\0";
const ENCODING: CERT_QUERY_ENCODING_TYPE = X509_ASN_ENCODING | PKCS_7_ASN_ENCODING;

struct Store(HCERTSTORE);

impl Store {
    fn local_machine(name: &str) -> Result<Self> {
        let name = wide(name);
        let store = unsafe { CertOpenStore(CERT_STORE_PROV_SYSTEM_W, 0, 0, CERT_SYSTEM_STORE_LOCAL_MACHINE, name.as_ptr() as *const c_void) };
        if store.is_null() {
            return Err(last_error("opening the certificate store"));
        }
        Ok(Store(store))
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        unsafe { CertCloseStore(self.0, 0) };
    }
}

struct Key(NCRYPT_KEY_HANDLE);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe { NCryptFreeObject(self.0) };
    }
}

struct Certificate(*const CERT_CONTEXT);

impl Drop for Certificate {
    fn drop(&mut self) {
        unsafe { CertFreeCertificateContext(self.0) };
    }
}

/// Signs `exe` with a new certificate trusted on this PC, replacing any earlier one.
pub fn sign(exe: &Path) -> Result<()> {
    remove_certificates()?;
    let key = new_key()?;
    let certificate = self_signed(&key)?;
    for store in TRUSTED_STORES {
        let store = Store::local_machine(store)?;
        let added = unsafe {
            let cert = &*certificate.0;
            CertAddEncodedCertificateToStore(store.0, ENCODING, cert.pbCertEncoded, cert.cbCertEncoded, CERT_STORE_ADD_REPLACE_EXISTING, null_mut())
        };
        if added == 0 {
            return Err(last_error("trusting the signing certificate"));
        }
    }
    sign_file(exe, &certificate, &key)
}

/// Removes every HomeBar signing certificate from the machine stores.
pub fn remove_certificates() -> Result<()> {
    let subject = wide(SUBJECT_NAME);
    for name in ALL_STORES {
        let store = Store::local_machine(name)?;
        loop {
            let found = unsafe { CertFindCertificateInStore(store.0, ENCODING, 0, CERT_FIND_SUBJECT_STR_W, subject.as_ptr() as *const c_void, null()) };
            if found.is_null() {
                break;
            }
            // frees `found` whether or not it succeeds
            if unsafe { CertDeleteCertificateFromStore(found) } == 0 {
                return Err(last_error("removing the old signing certificate"));
            }
        }
    }
    Ok(())
}

fn new_key() -> Result<Key> {
    unsafe {
        let mut provider = 0;
        check("opening the key provider", NCryptOpenStorageProvider(&mut provider, MS_KEY_STORAGE_PROVIDER, 0))?;
        let mut handle = 0;
        // no key name: an ephemeral key that exists only in this process
        let created = NCryptCreatePersistedKey(provider, &mut handle, BCRYPT_RSA_ALGORITHM, null(), 0, 0);
        NCryptFreeObject(provider);
        check("creating the signing key", created)?;
        let key = Key(handle);
        let bits = 2048u32.to_le_bytes();
        check("creating the signing key", NCryptSetProperty(key.0, NCRYPT_LENGTH_PROPERTY, bits.as_ptr(), 4, 0))?;
        check("creating the signing key", NCryptFinalizeKey(key.0, 0))?;
        Ok(key)
    }
}

fn encode_subject() -> Result<Vec<u8>> {
    let subject = wide(SUBJECT);
    unsafe {
        let mut size = 0;
        if CertStrToNameW(X509_ASN_ENCODING, subject.as_ptr(), CERT_X500_NAME_STR, null(), null_mut(), &mut size, null_mut()) == 0 {
            return Err(last_error("encoding the certificate name"));
        }
        let mut name = vec![0u8; size as usize];
        if CertStrToNameW(X509_ASN_ENCODING, subject.as_ptr(), CERT_X500_NAME_STR, null(), name.as_mut_ptr(), &mut size, null_mut()) == 0 {
            return Err(last_error("encoding the certificate name"));
        }
        name.truncate(size as usize);
        Ok(name)
    }
}

/// The "Code Signing" extended key usage extension, DER-encoded.
fn encode_code_signing_usage() -> Result<Vec<u8>> {
    let mut oid = OID_CODE_SIGNING.as_ptr() as *mut u8;
    let usage = CTL_USAGE { cUsageIdentifier: 1, rgpszUsageIdentifier: &mut oid };
    let info = &usage as *const CTL_USAGE as *const c_void;
    unsafe {
        let mut size = 0;
        if CryptEncodeObjectEx(X509_ASN_ENCODING, X509_ENHANCED_KEY_USAGE, info, 0, null(), null_mut(), &mut size) == 0 {
            return Err(last_error("encoding the certificate usage"));
        }
        let mut encoded = vec![0u8; size as usize];
        if CryptEncodeObjectEx(X509_ASN_ENCODING, X509_ENHANCED_KEY_USAGE, info, 0, null(), encoded.as_mut_ptr() as *mut c_void, &mut size) == 0 {
            return Err(last_error("encoding the certificate usage"));
        }
        encoded.truncate(size as usize);
        Ok(encoded)
    }
}

fn self_signed(key: &Key) -> Result<Certificate> {
    let mut name = encode_subject()?;
    let mut usage = encode_code_signing_usage()?;
    let name_blob = CRYPT_INTEGER_BLOB { cbData: name.len() as u32, pbData: name.as_mut_ptr() };
    let mut extension = CERT_EXTENSION {
        pszObjId: OID_ENHANCED_KEY_USAGE.as_ptr() as *mut u8,
        fCritical: 0,
        Value: CRYPT_INTEGER_BLOB { cbData: usage.len() as u32, pbData: usage.as_mut_ptr() },
    };
    let extensions = CERT_EXTENSIONS { cExtension: 1, rgExtension: &mut extension };
    let algorithm = CRYPT_ALGORITHM_IDENTIFIER { pszObjId: OID_SHA256_RSA.as_ptr() as *mut u8, Parameters: unsafe { zeroed() } };
    let mut expires: SYSTEMTIME = unsafe { zeroed() };
    unsafe { GetSystemTime(&mut expires) };
    expires.wYear += 20;
    if expires.wMonth == 2 && expires.wDay == 29 {
        expires.wDay = 28;
    }
    let certificate = unsafe {
        CertCreateSelfSignCertificate(key.0, &name_blob, CERT_CREATE_SELFSIGN_NO_KEY_INFO, null(), &algorithm, null(), &expires, &extensions)
    };
    if certificate.is_null() {
        return Err(last_error("creating the signing certificate"));
    }
    Ok(Certificate(certificate))
}

fn sign_file(exe: &Path, certificate: &Certificate, key: &Key) -> Result<()> {
    let what = "signing HomeBar.exe";
    // lend the in-memory key to the certificate; NO_CRYPT_RELEASE keeps ownership with `key`
    let key_context = CERT_KEY_CONTEXT {
        cbSize: size_of::<CERT_KEY_CONTEXT>() as u32,
        Anonymous: CERT_KEY_CONTEXT_0 { hNCryptKey: key.0 },
        dwKeySpec: CERT_NCRYPT_KEY_SPEC,
    };
    let lent: BOOL = unsafe {
        CertSetCertificateContextProperty(certificate.0, CERT_KEY_CONTEXT_PROP_ID, CERT_STORE_NO_CRYPT_RELEASE_FLAG, &key_context as *const _ as *const c_void)
    };
    if lent == 0 {
        return Err(last_error(what));
    }

    let path = wide(exe);
    let mut file = SIGNER_FILE_INFO { cbSize: size_of::<SIGNER_FILE_INFO>() as u32, pwszFileName: path.as_ptr(), hFile: null_mut() };
    let mut index = 0u32;
    let subject = SIGNER_SUBJECT_INFO {
        cbSize: size_of::<SIGNER_SUBJECT_INFO>() as u32,
        pdwIndex: &mut index,
        dwSubjectChoice: SIGNER_SUBJECT_FILE,
        Anonymous: SIGNER_SUBJECT_INFO_0 { pSignerFileInfo: &mut file },
    };
    let mut store_info = SIGNER_CERT_STORE_INFO {
        cbSize: size_of::<SIGNER_CERT_STORE_INFO>() as u32,
        pSigningCert: certificate.0,
        dwCertPolicy: SIGNER_CERT_POLICY_CHAIN,
        hCertStore: null_mut(),
    };
    let signer = SIGNER_CERT {
        cbSize: size_of::<SIGNER_CERT>() as u32,
        dwCertChoice: SIGNER_CERT_STORE,
        Anonymous: SIGNER_CERT_0 { pCertStoreInfo: &mut store_info },
        hwnd: null_mut(),
    };
    let signature = SIGNER_SIGNATURE_INFO {
        cbSize: size_of::<SIGNER_SIGNATURE_INFO>() as u32,
        algidHash: CALG_SHA_256,
        dwAttrChoice: SIGNER_NO_ATTR,
        Anonymous: SIGNER_SIGNATURE_INFO_0 { pAttrAuthcode: null_mut() },
        psAuthenticated: null_mut(),
        psUnauthenticated: null_mut(),
    };
    unsafe {
        let mut context = null_mut();
        let hr = SignerSignEx(0, &subject, &signer, &signature, null(), null(), null(), null(), &mut context);
        if !context.is_null() {
            SignerFreeSignerContext(context);
        }
        check(what, hr)
    }
}
