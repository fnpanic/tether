//! Admin (Ed25519), updater (minisign) and server (Ed25519, hbbs/hbbr `id_ed25519`) keys, sealed at
//! rest with argon2id + XChaCha20-Poly1305.

use argon2::{Algorithm, Argon2, Params, Version};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chacha20poly1305::aead::{rand_core::RngCore, Aead, AeadCore, KeyInit, OsRng, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use tether_trust::SigningKey;
use zeroize::Zeroize;

const AAD: &[u8] = b"tether-manage-keys-v1";

/// Vault format (`Sealed.v`): 1 = admin + updater key; 2 = may also hold server keys and fields
/// this version does not know (kept as they are). Tether Manage before 0.5.5 opens only 1 and
/// would drop the server keys when it seals again, so a vault with server keys is written as 2.
/// The encryption is the same in every format so far.
pub const FORMAT: u32 = 2;

pub struct Keys {
    pub admin: SigningKey,
    pub updater: minisign::SecretKey,
    /// Server key pairs generated here for a key rotation (docs/key-rotation.md), oldest first.
    pub server: Vec<SigningKey>,
    /// `Sealed.v` the vault was read with; `seal` refuses one newer than `FORMAT`.
    format: u32,
    /// Fields of `Plain` this version does not know, written back unchanged.
    extra: serde_json::Map<String, serde_json::Value>,
}

/// On-disk (and backup) format.
#[derive(Serialize, Deserialize)]
struct Sealed {
    v: u32,
    kdf: String,
    m_kib: u32,
    t: u32,
    p: u32,
    salt: String,
    nonce: String,
    ct: String,
}

#[derive(Serialize, Deserialize)]
struct Plain {
    admin: String,
    updater: String,
    /// Older versions of Tether Manage ignore it (no `deny_unknown_fields`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    server: Vec<String>,
    /// Written by a newer Tether Manage: kept and sealed again unchanged.
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

fn derive(pass: &str, salt: &[u8], m_kib: u32, t: u32, p: u32) -> Result<[u8; 32], String> {
    let params = Params::new(m_kib, t, p, Some(32)).map_err(|e| e.to_string())?;
    let mut out = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(pass.as_bytes(), salt, &mut out)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

fn d64(s: &str) -> Result<Vec<u8>, String> {
    B64.decode(s).map_err(|_| "the key file is damaged".to_owned())
}

impl Keys {
    pub fn generate() -> Result<Self, String> {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().map_err(|e| e.to_string())?;
        Ok(Keys { admin: tether_trust::generate_key(), updater: kp.sk, server: Vec::new(), format: 1, extra: Default::default() })
    }

    /// A new server key pair, kept in `server`. Like hbbs, no `/` in the public key (it can appear
    /// in `id@server?key=` links).
    pub fn add_server_key(&mut self) -> String {
        let k = loop {
            let k = tether_trust::generate_key();
            if !tether_trust::key_to_b64(&k.verifying_key()).contains('/') {
                break k;
            }
        };
        self.server.push(k);
        tether_trust::key_to_b64(&self.server.last().unwrap().verifying_key())
    }

    pub fn server_pubs(&self) -> Vec<String> {
        self.server.iter().map(|k| tether_trust::key_to_b64(&k.verifying_key())).collect()
    }

    /// `(id_ed25519, id_ed25519.pub)` for hbbs/hbbr: base64 of the 64-byte libsodium secret key
    /// (seed + public key) and of the public key, as hbbs writes them.
    pub fn server_key_files(&self, public: &str) -> Option<(String, String)> {
        let k = self.server.iter().find(|k| tether_trust::key_to_b64(&k.verifying_key()) == public)?;
        Some((B64.encode(k.to_keypair_bytes()), public.to_owned()))
    }

    pub fn admin_pub(&self) -> String {
        tether_trust::key_to_b64(&self.admin.verifying_key())
    }

    /// The form `tauri signer generate` prints: base64 of the minisign public key file.
    pub fn updater_pub(&self) -> String {
        let pk = minisign::PublicKey::from_secret_key(&self.updater).expect("valid secret key");
        B64.encode(pk.to_box().expect("encodable").to_string())
    }

    pub fn seal(&self, pass: &str) -> Result<Vec<u8>, String> {
        if self.format > FORMAT {
            return Err(format!(
                "This key file was written by a newer version of Tether Manage (format {}, this version knows up to {FORMAT}). Saving it here could lose keys: update Tether Manage first.",
                self.format
            ));
        }
        if pass.chars().count() < 8 {
            return Err("Use a passphrase of at least 8 characters.".into());
        }
        let (m_kib, t, p) = (64 * 1024, 3, 1);
        let mut salt = [0u8; 16];
        OsRng.fill_bytes(&mut salt);
        let mut key = derive(pass, &salt, m_kib, t, p)?;
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let mut plain = serde_json::to_vec(&Plain {
            admin: B64.encode(self.admin.to_bytes()),
            updater: B64.encode(self.updater.to_bytes()),
            server: self.server.iter().map(|k| B64.encode(k.to_bytes())).collect(),
            extra: self.extra.clone(),
        })
        .map_err(|e| e.to_string())?;
        let ct = XChaCha20Poly1305::new((&key).into())
            .encrypt(&nonce, Payload { msg: &plain, aad: AAD })
            .map_err(|_| "encryption failed".to_owned());
        plain.zeroize();
        key.zeroize();
        // 1 while an older Tether Manage loses nothing by opening it.
        let v = if self.server.is_empty() && self.extra.is_empty() && self.format < 2 { 1 } else { FORMAT };
        let sealed = Sealed {
            v,
            kdf: "argon2id".into(),
            m_kib,
            t,
            p,
            salt: B64.encode(salt),
            nonce: B64.encode(nonce),
            ct: B64.encode(ct?),
        };
        serde_json::to_vec_pretty(&sealed).map_err(|e| e.to_string())
    }

    pub fn open(data: &[u8], pass: &str) -> Result<Self, String> {
        let s: Sealed = serde_json::from_slice(data).map_err(|_| "This is not a Tether Manage key file.".to_owned())?;
        // A newer format is opened (same encryption) so its keys can be used, but never sealed again.
        if s.v == 0 || s.kdf != "argon2id" {
            return Err("This key file was written by a newer version of Tether Manage.".into());
        }
        // Bound attacker-controlled KDF cost so a crafted backup cannot exhaust memory.
        if s.m_kib > 1024 * 1024 || s.t > 20 || s.p > 16 {
            return Err("The key file has unsupported parameters.".into());
        }
        let nonce = d64(&s.nonce)?;
        if nonce.len() != 24 {
            return Err("the key file is damaged".into());
        }
        let mut key = derive(pass, &d64(&s.salt)?, s.m_kib, s.t, s.p)?;
        let plain = XChaCha20Poly1305::new((&key).into())
            .decrypt(XNonce::from_slice(&nonce), Payload { msg: &d64(&s.ct)?, aad: AAD })
            .map_err(|_| if s.v > FORMAT { "Wrong passphrase, or a key file of a newer version of Tether Manage.".to_owned() } else { "Wrong passphrase.".to_owned() });
        key.zeroize();
        let mut plain = plain?;
        let parsed: Result<Plain, _> = serde_json::from_slice(&plain);
        plain.zeroize();
        let mut p = parsed.map_err(|_| "the key file is damaged".to_owned())?;
        let admin: [u8; 32] = d64(&p.admin)?.try_into().map_err(|_| "the key file is damaged".to_owned())?;
        let updater = minisign::SecretKey::from_bytes(&d64(&p.updater)?).map_err(|_| "the key file is damaged".to_owned())?;
        let mut server = Vec::new();
        for k in &p.server {
            let b: [u8; 32] = d64(k)?.try_into().map_err(|_| "the key file is damaged".to_owned())?;
            server.push(SigningKey::from_bytes(&b));
        }
        Ok(Keys { admin: SigningKey::from_bytes(&admin), updater, server, format: s.v, extra: std::mem::take(&mut p.extra) })
    }

    /// Tauri updater signature for `data` named `file_name`: base64 of the minisign signature
    /// file text, exactly what `tauri signer sign` writes to `<file>.sig`.
    pub fn sign_update(&self, data: &[u8], file_name: &str, version: &str) -> Result<String, String> {
        if file_name.contains(['\t', '\r', '\n']) || version.contains(['\t', '\r', '\n']) {
            return Err("file name and version must not contain tabs or newlines".into());
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let trusted = format!("timestamp:{now}\tfile:{file_name}\tversion:{version}");
        let sig = minisign::sign(None, &self.updater, data, Some(&trusted), Some("signature from tauri secret key"))
            .map_err(|e| e.to_string())?;
        Ok(B64.encode(sig.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_roundtrip_and_wrong_passphrase() {
        let k = Keys::generate().unwrap();
        let sealed = k.seal("correct horse").unwrap();
        let back = Keys::open(&sealed, "correct horse").unwrap();
        assert_eq!(back.admin.to_bytes(), k.admin.to_bytes());
        assert_eq!(back.updater.to_bytes(), k.updater.to_bytes());
        assert_eq!(back.updater_pub(), k.updater_pub());
        assert_eq!(Keys::open(&sealed, "wrong horse").err().unwrap(), "Wrong passphrase.");
        assert!(k.seal("short").is_err());
        // Private key material never appears in the sealed file.
        let text = String::from_utf8(sealed).unwrap();
        assert!(!text.contains(&B64.encode(k.admin.to_bytes())));
    }

    #[test]
    fn server_keys_are_sealed_and_written_as_hbbs_expects() {
        let mut k = Keys::generate().unwrap();
        let old_format = k.seal("correct horse").unwrap();
        assert!(Keys::open(&old_format, "correct horse").unwrap().server.is_empty());
        let public = k.add_server_key();
        assert!(!public.contains('/'));
        let sealed = k.seal("correct horse").unwrap();
        let back = Keys::open(&sealed, "correct horse").unwrap();
        assert_eq!(back.server_pubs(), [public.clone()]);
        let (secret, pub_file) = back.server_key_files(&public).unwrap();
        assert_eq!(pub_file, public);
        // hbbs `gen_sk`: 64 bytes, the public key is the second half.
        let raw = B64.decode(&secret).unwrap();
        assert_eq!(raw.len(), 64);
        assert_eq!(B64.encode(&raw[32..]), public);
        assert!(!String::from_utf8(sealed).unwrap().contains(&secret[..20]));
        assert!(back.server_key_files("unknown").is_none());
    }

    /// Seal `plain` as format `v` the way some other version would.
    fn sealed_as(v: u32, plain: &serde_json::Value, pass: &str) -> Vec<u8> {
        let mut salt = [0u8; 16];
        OsRng.fill_bytes(&mut salt);
        let key = derive(pass, &salt, 64 * 1024, 3, 1).unwrap();
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ct = XChaCha20Poly1305::new((&key).into()).encrypt(&nonce, Payload { msg: &serde_json::to_vec(plain).unwrap(), aad: AAD }).unwrap();
        let s = Sealed { v, kdf: "argon2id".into(), m_kib: 64 * 1024, t: 3, p: 1, salt: B64.encode(salt), nonce: B64.encode(nonce), ct: B64.encode(ct) };
        serde_json::to_vec(&s).unwrap()
    }

    fn version_of(sealed: &[u8]) -> u32 {
        serde_json::from_slice::<Sealed>(sealed).unwrap().v
    }

    #[test]
    fn format_version_and_unknown_fields() {
        let pass = "correct horse";
        let mut k = Keys::generate().unwrap();
        // Nothing an older version would drop: still format 1, so it can open it.
        assert_eq!(version_of(&k.seal(pass).unwrap()), 1);
        // Server keys: format 2, which Tether Manage before 0.5.5 refuses to open (instead of
        // dropping them when it seals again).
        k.add_server_key();
        assert_eq!(version_of(&k.seal(pass).unwrap()), FORMAT);

        // Fields a newer version added within format 2 are kept, and the format stays 2.
        let plain = serde_json::json!({ "admin": B64.encode(k.admin.to_bytes()), "updater": B64.encode(k.updater.to_bytes()), "future": { "a": 1 } });
        let opened = Keys::open(&sealed_as(2, &plain, pass), pass).unwrap();
        let resealed = opened.seal(pass).unwrap();
        assert_eq!(version_of(&resealed), 2);
        assert_eq!(Keys::open(&resealed, pass).unwrap().extra["future"], serde_json::json!({ "a": 1 }));

        // A newer format: its keys can be used, but this version refuses to write it.
        let newer = Keys::open(&sealed_as(FORMAT + 1, &plain, pass), pass).unwrap();
        assert_eq!(newer.admin.to_bytes(), k.admin.to_bytes());
        assert!(newer.seal(pass).unwrap_err().contains("newer version of Tether Manage"));
        assert!(Keys::open(&sealed_as(FORMAT + 1, &plain, pass), "wrong horse").err().unwrap().contains("newer version"));
    }

    /// Mirrors tauri-plugin-updater: base64-decode pubkey and signature, parse the minisign
    /// texts, verify the bytes.
    #[test]
    fn updater_signature_verifies_like_tauri_updater() {
        let k = Keys::generate().unwrap();
        let data = b"installer bytes".repeat(1000);
        let sig_b64 = k.sign_update(&data, "Tether_0.3.1_x64-setup.exe", "0.3.1").unwrap();

        let pk_text = String::from_utf8(B64.decode(k.updater_pub()).unwrap()).unwrap();
        assert!(pk_text.starts_with("untrusted comment: minisign public key"));
        let sig_text = String::from_utf8(B64.decode(&sig_b64).unwrap()).unwrap();
        let lines: Vec<&str> = sig_text.lines().collect();
        assert_eq!(lines[0], "untrusted comment: signature from tauri secret key");
        assert!(lines[2].starts_with("trusted comment: timestamp:"));
        assert!(lines[2].contains("\tfile:Tether_0.3.1_x64-setup.exe\tversion:0.3.1"));

        let pk = minisign_verify::PublicKey::decode(&pk_text).unwrap();
        let sig = minisign_verify::Signature::decode(&sig_text).unwrap();
        pk.verify(&data, &sig, true).unwrap();
        assert!(pk.verify(b"tampered", &sig, true).is_err());
        let other = Keys::generate().unwrap();
        let other_pk = minisign_verify::PublicKey::decode(&String::from_utf8(B64.decode(other.updater_pub()).unwrap()).unwrap()).unwrap();
        assert!(other_pk.verify(&data, &sig, true).is_err());
    }
}
