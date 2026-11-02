//! 网易云 weapi 加密（与 Binaryify/NeteaseCloudMusicApi 对齐）

use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use cbc::Encryptor;
use num_bigint::BigUint;
use rand::Rng;

type Aes128CbcEnc = Encryptor<aes::Aes128>;

const AES_KEY1: &[u8; 16] = b"0CoJUm6Qyw8W8jud";
const AES_IV: &[u8; 16] = b"0102030405060708";

const RSA_MODULUS_HEX: &str = "00e0b509f6259df8642dbc35662901477df22677ec152b5ff68ace615bb7b725152b3ab17a876aea8a5aa76d2e417629ec4ee341f56135fccf695280104e0312ecbda92557c93870114af6c9d05c4f7f0c3685b7a46bee255932575cce10b424d813cfe4875d3e82047b97ddef52741d546b8e289dc6935b3ece0462db0a22b8e7";
const RSA_EXPONENT_HEX: &str = "010001";

const SECKEY_ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

fn random_seckey() -> String {
    let mut rng = rand::thread_rng();
    (0..16)
        .map(|_| SECKEY_ALPHABET[rng.gen_range(0..SECKEY_ALPHABET.len())] as char)
        .collect()
}

/// AES-128-CBC PKCS7 加密，输出 base64
fn aes_cbc_encrypt(data: &[u8], key: &[u8; 16], iv: &[u8; 16]) -> String {
    let len = data.len();
    // 关键：buf 实际长度必须 ≥ len + 16（放 padding）
    let mut buf = vec![0u8; len + 16];
    buf[..len].copy_from_slice(data);

    let enc = Aes128CbcEnc::new(key.into(), iv.into());
    let cipher = enc.encrypt_padded_mut::<Pkcs7>(&mut buf, len)
        .expect("AES encrypt failed");
    B64.encode(cipher)
}

/// RSA 加密 secKey：反转字符串 → hex → BigUint modPow
fn rsa_encrypt_seckey(seckey: &str) -> String {
    let reversed: Vec<u8> = seckey.bytes().rev().collect();
    let hex_str = hex::encode(&reversed);
    let m = BigUint::parse_bytes(RSA_MODULUS_HEX.as_bytes(), 16).expect("invalid modulus");
    let e = BigUint::parse_bytes(RSA_EXPONENT_HEX.as_bytes(), 16).expect("invalid exponent");
    let c = BigUint::parse_bytes(hex_str.as_bytes(), 16).expect("invalid seckey hex");
    let enc = c.modpow(&e, &m);
    format!("{:0>256x}", enc)
}

/// 输入 JSON 字符串，返回 (params_base64, encSecKey_hex)
pub fn weapi_encrypt(json_text: &str) -> (String, String) {
    let seckey = random_seckey();
    let first = aes_cbc_encrypt(json_text.as_bytes(), AES_KEY1, AES_IV);
    let sec_key_bytes: &[u8; 16] = seckey.as_bytes().try_into().expect("seckey must be 16 bytes");
    let params = aes_cbc_encrypt(first.as_bytes(), sec_key_bytes, AES_IV);
    let enc_sec_key = rsa_encrypt_seckey(&seckey);
    (params, enc_sec_key)
}
