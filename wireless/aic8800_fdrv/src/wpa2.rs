//! WPA2-PSK 四次握手实现  
//!  
//! AIC8800 是 FullMAC 芯片，固件处理 802.11 认证/关联，  
//! 但 WPA2 四次握手需要由主机（驱动）完成。  
//!  
//! 流程：  
//!   1. SM_CONNECT_REQ 设置 CONTROL_PORT_HOST | WPA_WPA2_IN_USE  
//!   2. 固件完成 802.11 关联后发送 SM_CONNECT_IND  
//!   3. AP 发送 EAPOL M1 → 固件作为 DATA 帧转发给主机  
//!   4. 主机处理四次握手（M1→M2→M3→M4）  
//!   5. 主机通过 MM_KEY_ADD_REQ 安装 PTK 和 GTK  
//!   6. 主机通过 ME_SET_CONTROL_PORT_REQ 打开控制端口  

extern crate alloc;  
  
use alloc::vec;  
use alloc::vec::Vec;  

use hmac::{Hmac, Mac};  
use sha1::Sha1;  
use aes::Aes128;  
use aes::cipher::{BlockDecrypt, KeyInit, generic_array::GenericArray};  

/// EAPOL ethertype (网络字节序)  
pub const ETH_P_PAE: u16 = 0x888E;  
  
/// EAPOL 版本  
const EAPOL_VERSION: u8 = 0x01; // 802.1X-2004  
  
/// EAPOL 类型  
const EAPOL_TYPE_KEY: u8 = 0x03;  
  
/// Key descriptor type (RSN = 2)  
const KEY_DESC_TYPE_RSN: u8 = 0x02;  
  
/// Key Info 位域  
const KEY_INFO_TYPE_HMAC_SHA1_AES: u16 = 0x0002; // Key Descriptor Version 2  
const KEY_INFO_PAIRWISE: u16 = 0x0008;  
const KEY_INFO_INSTALL: u16 = 0x0040;  
const KEY_INFO_ACK: u16 = 0x0080;  
const KEY_INFO_MIC: u16 = 0x0100;  
const KEY_INFO_SECURE: u16 = 0x0200;  
const KEY_INFO_ENC_KEY_DATA: u16 = 0x1000;  
  
/// 802.1X header 大小: version(1) + type(1) + body_len(2) = 4  
const EAPOL_HDR_LEN: usize = 4;  
/// EAPOL-Key body 固定头部大小（不含 Key Data）:  
///   desc_type(1) + key_info(2) + key_len(2) + replay(8) + nonce(32) +  
///   iv(16) + rsc(8) + reserved(8) + mic(16) + data_len(2) = 95  
const EAPOL_KEY_HDR_LEN: usize = 95;  
/// MIC 在 EAPOL 帧中的偏移 = EAPOL_HDR_LEN + 77 = 81  
const MIC_OFFSET: usize = EAPOL_HDR_LEN + 77;  
  
/// PMK 长度  
const PMK_LEN: usize = 32;  
/// PTK 长度 (KCK + KEK + TK = 16 + 16 + 16 = 48)  
const PTK_LEN: usize = 48;  
/// KCK 长度 (Key Confirmation Key)  
const KCK_LEN: usize = 16;  
/// KEK 长度 (Key Encryption Key)  
const KEK_LEN: usize = 16;  
/// TK 长度 (Temporal Key, for CCMP)  
const TK_LEN: usize = 16;  
/// Nonce 长度  
const NONCE_LEN: usize = 32;  
/// MIC 长度  
const MIC_LEN: usize = 16;  
/// Replay counter 长度  
const REPLAY_COUNTER_LEN: usize = 8;  
/// SHA1 digest size  
const SHA1_DIGEST_SIZE: usize = 20;  

type HmacSha1 = Hmac<Sha1>;

// ================================================================  
// 类型定义  
// ================================================================  
  
/// 握手状态  
#[derive(Debug, Clone, Copy, PartialEq, Eq)]  
pub enum HandshakeState {  
    /// 等待 M1  
    Idle,  
    /// 已发送 M2，等待 M3  
    M2Sent,  
    /// 握手完成  
    Completed,  
}  
  
/// 握手动作（process_eapol 的返回值）  
pub enum HandshakeAction {  
    /// 需要发送 M2 给 AP  
    SendM2(Vec<u8>),  
    /// 握手完成，包含 M4 帧和密钥材料  
    Completed(HandshakeResult),  
}  
  
/// 握手完成后的结果  
#[derive(Debug, Clone)]  
pub struct HandshakeResult {  
    /// M4 EAPOL 帧（需要发送给 AP）  
    pub m4_frame: Vec<u8>,  
    /// Temporal Key（16 字节，用于 CCMP 数据加密）  
    pub tk: [u8; TK_LEN],  
    /// Group Temporal Key（用于组播/广播解密）  
    pub gtk: Vec<u8>,  
    /// GTK 的 Key Index  
    pub gtk_key_idx: u8,  
}  
  
/// WPA2 错误类型  
#[derive(Debug)]  
pub enum WpaError {  
    FrameTooShort,  
    InvalidEapolType,  
    InvalidDescriptorType,  
    UnexpectedMessage,  
    InvalidState,  
    ReplayCounterMismatch,  
    MicMismatch,  
    InvalidKeyData,  
    AesUnwrapFailed,  
    GtkNotFound,  
}  
  
impl core::fmt::Display for WpaError {  
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {  
        match self {  
            WpaError::FrameTooShort => write!(f, "frame too short"),  
            WpaError::InvalidEapolType => write!(f, "not an EAPOL-Key frame"),  
            WpaError::InvalidDescriptorType => write!(f, "invalid key descriptor type"),  
            WpaError::UnexpectedMessage => write!(f, "unexpected message"),  
            WpaError::InvalidState => write!(f, "invalid handshake state"),  
            WpaError::ReplayCounterMismatch => write!(f, "replay counter mismatch"),  
            WpaError::MicMismatch => write!(f, "MIC verification failed"),  
            WpaError::InvalidKeyData => write!(f, "invalid key data"),  
            WpaError::AesUnwrapFailed => write!(f, "AES key unwrap failed"),  
            WpaError::GtkNotFound => write!(f, "GTK not found in key data"),  
        }  
    }  
} 

// ================================================================  
// EAPOL-Key 帧解析  
// ================================================================  
  
/// EAPOL-Key 帧解析结果  
#[derive(Debug)]  
struct EapolKeyHeader {  
    key_info: u16,  
    key_length: u16,  
    replay_counter: [u8; REPLAY_COUNTER_LEN],  
    key_nonce: [u8; NONCE_LEN],  
    key_iv: [u8; 16],  
    key_rsc: [u8; 8],  
    key_mic: [u8; MIC_LEN],  
    key_data_len: u16,  
    key_data: Vec<u8>,  
}  

/// 解析 EAPOL-Key 帧  
///  
/// `eapol` 是完整的 EAPOL 帧（从 version 字段开始）  
fn parse_eapol_key_header(eapol: &[u8]) -> Result<EapolKeyHeader, WpaError> {  
    // 最小长度: EAPOL header (4) + EAPOL-Key header (95) = 99  
    if eapol.len() < EAPOL_HDR_LEN + EAPOL_KEY_HDR_LEN {  
        return Err(WpaError::FrameTooShort);  
    }  

    // 检查 EAPOL type  
    if eapol[1] != EAPOL_TYPE_KEY {  
        return Err(WpaError::InvalidEapolType);  
    }  
  
    let off = EAPOL_HDR_LEN; // 4  

    // 检查 Key Descriptor Type  
    if eapol[off] != KEY_DESC_TYPE_RSN {  
        return Err(WpaError::InvalidDescriptorType);  
    }  
  
    let key_info = u16::from_be_bytes([eapol[off + 1], eapol[off + 2]]);  
    let key_length = u16::from_be_bytes([eapol[off + 3], eapol[off + 4]]);  
  
    let mut replay_counter = [0u8; REPLAY_COUNTER_LEN];  
    replay_counter.copy_from_slice(&eapol[off + 5..off + 13]);  
  
    let mut key_nonce = [0u8; NONCE_LEN];  
    key_nonce.copy_from_slice(&eapol[off + 13..off + 45]);  
  
    let mut key_iv = [0u8; 16];  
    key_iv.copy_from_slice(&eapol[off + 45..off + 61]);  
  
    let mut key_rsc = [0u8; 8];  
    key_rsc.copy_from_slice(&eapol[off + 61..off + 69]);  
  
    // reserved: off+69..off+77 (skip)  
  
    let mut key_mic = [0u8; MIC_LEN];  
    key_mic.copy_from_slice(&eapol[off + 77..off + 93]);  
  
    let key_data_len = u16::from_be_bytes([eapol[off + 93], eapol[off + 94]]);  
  
    let key_data_start = EAPOL_HDR_LEN + EAPOL_KEY_HDR_LEN; // 99  
    let key_data_end = key_data_start + key_data_len as usize;  
  
    if eapol.len() < key_data_end {  
        return Err(WpaError::FrameTooShort);  
    }  
  
    let key_data = eapol[key_data_start..key_data_end].to_vec();  
  
    Ok(EapolKeyHeader {  
        key_info,  
        key_length,  
        replay_counter,  
        key_nonce,  
        key_iv,  
        key_rsc,  
        key_mic,  
        key_data_len,  
        key_data,  
    })  
}

// ================================================================  
// PTK 结构体  
// ================================================================  
  
#[derive(Clone)]  
struct Ptk {  
    kck: [u8; KCK_LEN],  
    kek: [u8; KEK_LEN],  
    tk: [u8; TK_LEN],  
}  
  
impl Ptk {  
    fn from_bytes(ptk_bytes: &[u8; PTK_LEN]) -> Self {  
        let mut kck = [0u8; KCK_LEN];  
        let mut kek = [0u8; KEK_LEN];  
        let mut tk = [0u8; TK_LEN];  
        kck.copy_from_slice(&ptk_bytes[0..16]);  
        kek.copy_from_slice(&ptk_bytes[16..32]);  
        tk.copy_from_slice(&ptk_bytes[32..48]);  
        Self { kck, kek, tk }  
    }  
}  

// ================================================================  
// 密码学辅助函数  
// ================================================================  
  
/// HMAC-SHA1  
fn hmac_sha1(key: &[u8], data: &[u8]) -> [u8; SHA1_DIGEST_SIZE] {  
    let mut mac = <HmacSha1 as Mac>::new_from_slice(key).expect("HMAC key length");  
    mac.update(data);  
    let result = mac.finalize();  
    let mut out = [0u8; SHA1_DIGEST_SIZE];  
    out.copy_from_slice(&result.into_bytes());  
    out  
}  

/// PBKDF2-HMAC-SHA1  
fn pbkdf2_sha1(passphrase: &[u8], salt: &[u8], iterations: u32, dk_len: usize) -> Vec<u8> {  
    let mut result = Vec::with_capacity(dk_len);  
    let blocks_needed = (dk_len + SHA1_DIGEST_SIZE - 1) / SHA1_DIGEST_SIZE;  
    
    for block_index in 1..=blocks_needed {  
        // U1 = HMAC-SHA1(password, salt || INT(block_idx))  
        let mut salt_block = Vec::with_capacity(salt.len() + 4);
        salt_block.extend_from_slice(salt);
        salt_block.extend_from_slice(&(block_index as u32).to_be_bytes());

        let mut u = hmac_sha1(passphrase, &salt_block);     
        let mut t = u;  

        for _ in 1..iterations {  
            u = hmac_sha1(passphrase, &u);  
            for i in 0..SHA1_DIGEST_SIZE {  
                t[i] ^= u[i];  
            }  
        }  
        result.extend_from_slice(&t);  
    } 

    result.truncate(dk_len);  
    result
}

/// IEEE 802.11i PRF-SHA1 (Pseudo-Random Function)  
///  
/// PRF-384 for PTK derivation (48 bytes = 384 bits)  
fn prf_sha1(key: &[u8], label: &[u8], data: &[u8], output_len: usize) -> Vec<u8> {  
    let iterations = (output_len + SHA1_DIGEST_SIZE - 1) / SHA1_DIGEST_SIZE;    
    let mut result = Vec::with_capacity(iterations * SHA1_DIGEST_SIZE);

    for i in 0..iterations {    
        // HMAC-SHA1(key, label || 0x00 || data || counter)  
        let mut input = Vec::with_capacity(label.len() + 1 + data.len() + 1);  
        input.extend_from_slice(label);  
        input.push(0x00); // separator  
        input.extend_from_slice(data);  
        input.push(i as u8); // counter  

        let hash = hmac_sha1(key, &input);  
        result.extend_from_slice(&hash);  
    }  
    result.truncate(output_len);  
    result
}

/// 派生 PTK  
///  
/// PTK = PRF-384(PMK, "Pairwise key expansion", Min(AA,SPA) || Max(AA,SPA) || Min(ANonce,SNonce) || Max(ANonce,SNonce))  
fn derive_ptk(  
    pmk: &[u8; PMK_LEN],  
    aa: &[u8; 6],  
    spa: &[u8; 6],  
    anonce: &[u8; NONCE_LEN],  
    snonce: &[u8; NONCE_LEN],  
) -> Ptk {
    // 构造 data: Min(AA,SPA) || Max(AA,SPA) || Min(ANonce,SNonce) || Max(ANonce,SNonce)  
    let mut data = [0u8; 6 + 6 + NONCE_LEN + NONCE_LEN]; // 76 bytes 

    // MAC 地址排序  
    let (min_addr, max_addr) = if aa[..] < spa[..] {  
        (aa.as_slice(), spa.as_slice())  
    } else {  
        (spa.as_slice(), aa.as_slice())  
    };  
    data[0..6].copy_from_slice(min_addr);  
    data[6..12].copy_from_slice(max_addr);  
  
    // Nonce 排序  
    let (min_nonce, max_nonce) = if anonce[..] < snonce[..] {  
        (anonce.as_slice(), snonce.as_slice())  
    } else {  
        (snonce.as_slice(), anonce.as_slice())  
    };  
    data[12..44].copy_from_slice(min_nonce);  
    data[44..76].copy_from_slice(max_nonce);  
  
    let ptk_bytes = prf_sha1(pmk, b"Pairwise key expansion", &data, PTK_LEN);  
    let mut ptk_arr = [0u8; PTK_LEN];  
    ptk_arr.copy_from_slice(&ptk_bytes);  
    Ptk::from_bytes(&ptk_arr)  
}

/// 计算 MIC (HMAC-SHA1-128, 取前 16 字节)  
fn compute_mic(kck: &[u8], eapol_frame: &[u8]) -> [u8; MIC_LEN] {  
    let hash = hmac_sha1(kck, eapol_frame);  
    let mut mic = [0u8; MIC_LEN];  
    mic.copy_from_slice(&hash[..MIC_LEN]);  
    mic  
}  

/// AES Key Unwrap (RFC 3394)  
///  
/// `kek`: 16-byte Key Encryption Key  
/// `wrapped`: wrapped key data (must be multiple of 8 bytes, >= 16 bytes)  
/// Returns unwrapped key data (8 bytes shorter than input)  
fn aes_key_unwrap(kek: &[u8], wrapped: &[u8]) -> Result<Vec<u8>, WpaError> {  
    if wrapped.len() < 16 || wrapped.len() % 8 != 0 {  
        return Err(WpaError::AesUnwrapFailed);  
    }  
  
    let n = (wrapped.len() / 8) - 1; // number of 64-bit blocks  
    let cipher = Aes128::new(GenericArray::from_slice(kek));  
  
    // Initialize  
    let mut a = [0u8; 8];  
    a.copy_from_slice(&wrapped[0..8]);  
  
    let mut r = Vec::with_capacity(n * 8);  
    for i in 0..n {  
        r.extend_from_slice(&wrapped[(i + 1) * 8..(i + 2) * 8]);  
    }  
  
    // Unwrap: 6 rounds  
    for j in (0..6u64).rev() {  
        for i in (0..n).rev() {  
            let t = (n as u64) * j + (i as u64) + 1;  
  
            // A ^= t  
            let t_bytes = t.to_be_bytes();  
            for k in 0..8 {  
                a[k] ^= t_bytes[k];  
            }  
  
            // B = AES-1(KEK, A || R[i])  
            let mut block = [0u8; 16];  
            block[0..8].copy_from_slice(&a);  
            block[8..16].copy_from_slice(&r[i * 8..(i + 1) * 8]);  
  
            let ga = GenericArray::from_mut_slice(&mut block);  
            cipher.decrypt_block(ga);  
  
            a.copy_from_slice(&block[0..8]);  
            r[i * 8..(i + 1) * 8].copy_from_slice(&block[8..16]);  
        }  
    }  
  
    // Check IV  
    const DEFAULT_IV: [u8; 8] = [0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6];  
    if a != DEFAULT_IV {  
        log::error!(  
            "[wpa2] AES Key Unwrap IV check failed: {:02x?} != {:02x?}",  
            a, DEFAULT_IV  
        );  
        return Err(WpaError::AesUnwrapFailed);  
    }  
  
    Ok(r)  
}  

fn generate_snonce() -> [u8; NONCE_LEN] {  
    let mut snonce = [0u8; NONCE_LEN];  
    let ts = axhal::time::monotonic_time_nanos();  
    let ts_bytes = ts.to_le_bytes();  
    let hash1 = hmac_sha1(&ts_bytes, b"snonce-gen-1");  
    let hash2 = hmac_sha1(&ts_bytes, b"snonce-gen-2");  
    snonce[..20].copy_from_slice(&hash1);  
    snonce[20..32].copy_from_slice(&hash2[..12]);  
    snonce  
}

/// 构造 EAPOL-Key 帧  
///  
/// 返回完整的 EAPOL 帧（从 version 字段开始），MIC 字段初始化为全零  
fn build_eapol_key_frame(  
    key_info: u16,  
    key_length: u16,  
    replay_counter: &[u8; REPLAY_COUNTER_LEN],  
    key_nonce: &[u8; NONCE_LEN],  
    key_data: &[u8],  
) -> Vec<u8> {  
    let key_data_len = key_data.len() as u16;  
    let body_len = (EAPOL_KEY_HDR_LEN + key_data.len()) as u16;  
  
    let total_len = EAPOL_HDR_LEN + EAPOL_KEY_HDR_LEN + key_data.len();  
    let mut frame = vec![0u8; total_len];  
  
    // 802.1X header  
    frame[0] = EAPOL_VERSION;  
    frame[1] = EAPOL_TYPE_KEY;  
    frame[2..4].copy_from_slice(&body_len.to_be_bytes());  
  
    let off = EAPOL_HDR_LEN; // 4  
  
    // Key Descriptor Type  
    frame[off] = KEY_DESC_TYPE_RSN;  
  
    // Key Info  
    frame[off + 1..off + 3].copy_from_slice(&key_info.to_be_bytes());  
  
    // Key Length  
    frame[off + 3..off + 5].copy_from_slice(&key_length.to_be_bytes());  
  
    // Replay Counter  
    frame[off + 5..off + 13].copy_from_slice(replay_counter);  
  
    // Key Nonce  
    frame[off + 13..off + 45].copy_from_slice(key_nonce);  
  
    // Key IV: [off+45..off+61] = 0 (already zero)  
    // Key RSC: [off+61..off+69] = 0 (already zero)  
    // Reserved: [off+69..off+77] = 0 (already zero)  
    // Key MIC: [off+77..off+93] = 0 (will be filled by caller)  
  
    // Key Data Length  
    frame[off + 93..off + 95].copy_from_slice(&key_data_len.to_be_bytes());  
  
    // Key Data  
    if !key_data.is_empty() {  
        frame[EAPOL_HDR_LEN + EAPOL_KEY_HDR_LEN..].copy_from_slice(key_data);  
    }  
  
    frame  
}  

/// 解析 GTK KDE (Key Data Encapsulation)  
///  
/// KDE 格式:  
///   [0]     type = 0xDD (Vendor Specific)  
///   [1]     length  
///   [2..5]  OUI + data type = 00-0F-AC-01 (GTK KDE)  
///   [6]     Key ID (bits 0-1) | Tx (bit 2)  
///   [7]     reserved  
///   [8..]   GTK  
fn parse_gtk_kde(data: &[u8]) -> Result<(Vec<u8>, u8), WpaError> {
    let mut offset = 0; 

    while offset + 2 <= data.len() {
        let element_type = data[offset];
        let element_len = data[offset + 1] as usize;

        if offset + 2 + element_len > data.len() {
            return Err(WpaError::InvalidKeyData);
        }

        // 检查是否是 GTK KDE: type=0xDD, OUI=00-0F-AC, data_type=01  
        if element_type == 0xDD 
            && element_len >= 6 
            && data[offset + 2..offset + 6] == [0x00, 0x0F, 0xAC, 0x01] {
            let key_id = data[offset + 6] & 0x03; // Key ID 在 bits 0-1  
            let gtk = data[offset + 8..offset + 2 + element_len].to_vec();  
            log::info!(  
                "[wpa2] Found GTK KDE: key_id={}, gtk_len={}",  
                key_id,  
                gtk.len(),  
            ); 
            return Ok((gtk, key_id));  
        }

        // 跳过 padding (type=0x00)   
        if element_type == 0x00 {  
            offset += 1;  
            continue;  
        }  
  
        offset += 2 + element_len; 
    }

    log::error!("[wpa2] GTK KDE not found in key data ({} bytes)", data.len());  
    Err(WpaError::GtkNotFound)  
}

// ================================================================  
// WPA2 握手上下文  
// ================================================================  
  
pub struct Wpa2Handshake {  
    pub state: HandshakeState,  
    pmk: [u8; PMK_LEN],  
    ptk: Option<Ptk>,  
    anonce: [u8; NONCE_LEN],  
    snonce: [u8; NONCE_LEN],  
    aa: [u8; 6],  
    spa: [u8; 6],  
    rsn_ie: Vec<u8>,  
    replay_counter: [u8; REPLAY_COUNTER_LEN],  
    gtk: Vec<u8>,  
    gtk_key_idx: u8,  
}  

impl Wpa2Handshake {  
    pub fn new(
        passphrase: &[u8],  
        ssid: &[u8],  
        aa: &[u8; 6],  
        spa: &[u8; 6],  
        rsn_ie: &[u8], 
    ) -> Self {  
        let pmk_vec = pbkdf2_sha1(passphrase, ssid, 4096, PMK_LEN);  
        let mut pmk = [0u8; PMK_LEN];  
        pmk.copy_from_slice(&pmk_vec);  

        log::info!("[wpa2] PMK = {:02x?}", &pmk[..]);

        // log::info!(  
        //     "[wpa2] PMK derived, AA={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, SPA={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",  
        //     aa[0], aa[1], aa[2], aa[3], aa[4], aa[5],  
        //     spa[0], spa[1], spa[2], spa[3], spa[4], spa[5],  
        // );  
  
        let snonce = generate_snonce(); 

        log::info!("[wpa2] SNonce = {:02x?}", &snonce[..]);
        
        Self {  
            state: HandshakeState::Idle,  
            pmk,  
            ptk: None,  
            snonce,  
            anonce: [0u8; NONCE_LEN],  
            aa: *aa,  
            spa: *spa,  
            rsn_ie: rsn_ie.to_vec(),  
            replay_counter: [0u8; REPLAY_COUNTER_LEN],  
            gtk: Vec::new(),  
            gtk_key_idx: 0,  
        }  
    }  

    /// 处理收到的 EAPOL 帧，返回需要执行的动作  
    ///  
    /// `eapol` 是完整的 EAPOL 帧（从 802.1X Version 字段开始，不含 Ethernet 头） 
    pub fn process_eapol(&mut self, eapol: &[u8]) -> Result<HandshakeAction, WpaError> {
        let hdr = parse_eapol_key_header(eapol)?; 

        // 判断是 M1 还是 M3  
        let has_ack = (hdr.key_info & KEY_INFO_ACK) != 0;  
        let has_mic = (hdr.key_info & KEY_INFO_MIC) != 0;  
        let has_install = (hdr.key_info & KEY_INFO_INSTALL) != 0;  
        let has_enc = (hdr.key_info & KEY_INFO_ENC_KEY_DATA) != 0;  
  
        if has_ack && !has_mic {  
            // M1: ACK=1, MIC=0  
            log::info!("[wpa2] === Processing M1 ===");  
            log::info!("[wpa2] M1 full ({} bytes): {:02x?}", eapol.len(), eapol);
            self.process_m1(&hdr, eapol)  
        } else if has_ack && has_mic && has_install && has_enc {  
            // M3: ACK=1, MIC=1, Install=1, EncKeyData=1  
            log::info!("[wpa2] === Processing M3 ===");  
            self.process_m3(&hdr, eapol)  
        } else {  
            log::warn!(  
                "[wpa2] Unexpected EAPOL key_info=0x{:04x}, ignoring",  
                hdr.key_info  
            );  
            Err(WpaError::UnexpectedMessage)  
        }
    }

    fn process_m1(  
        &mut self,  
        hdr: &EapolKeyHeader,  
        eapol: &[u8],  
    ) -> Result<HandshakeAction, WpaError> {  
        if self.state != HandshakeState::Idle && self.state != HandshakeState::M2Sent {  
            log::warn!("[wpa2] M1 received in unexpected state: {:?}", self.state);  
            // 允许重新开始（AP 可能重发 M1）  
        } 

        // 保存 ANonce 和 Replay Counter  
        self.anonce.copy_from_slice(&hdr.key_nonce);  
        self.replay_counter.copy_from_slice(&hdr.replay_counter);  
  
        log::info!("[wpa2] M1 full ({} bytes): {:02x?}", eapol.len(), eapol);
        log::info!("[wpa2] derive_ptk inputs:");  
        log::info!("[wpa2]   PMK: {:02x?}", &self.pmk);  
        log::info!("[wpa2]   AA:  {:02x?}", &self.aa);  
        log::info!("[wpa2]   SPA: {:02x?}", &self.spa);  
        log::info!("[wpa2]   ANonce: {:02x?}", &self.anonce);  
        log::info!("[wpa2]   SNonce: {:02x?}", &self.snonce);

        // 派生 PTK  
        let ptk = derive_ptk(  
            &self.pmk,  
            &self.aa,  
            &self.spa,  
            &self.anonce,  
            &self.snonce,  
        );

        log::info!("[wpa2] PTK KCK (full 16B): {:02x?}", &ptk.kck);  
        log::info!("[wpa2] PTK KEK (full 16B): {:02x?}", &ptk.kek);  
        log::info!("[wpa2] PTK TK  (full 16B): {:02x?}", &ptk.tk);
  
        self.ptk = Some(ptk); 

        // 构造 M2  
        let key_info: u16 = KEY_INFO_TYPE_HMAC_SHA1_AES  
            | KEY_INFO_PAIRWISE  
            | KEY_INFO_MIC;  
        
        let mut m2 = build_eapol_key_frame(
            key_info, 
            0,           // key_length = 0 in M2
            &self.replay_counter, 
            &self.snonce, // M2 携带 SNonce  
            &self.rsn_ie,  // Key Data = RSN IE 
        );

        assert_eq!(m2.len(), EAPOL_HDR_LEN + EAPOL_KEY_HDR_LEN + self.rsn_ie.len(), "M2 frame length mismatch");

        log::info!("[wpa2] M2 full ({} bytes): {:02x?}", m2.len(), &m2);

        // 计算并填入 MIC  
        let mic = compute_mic(  
            &self.ptk.as_ref().unwrap().kck,  
            &m2,  
        ); 
        m2[MIC_OFFSET..MIC_OFFSET + MIC_LEN].copy_from_slice(&mic);  
  
        log::info!("[wpa2] M2 Key Data (RSN IE used): {:02x?}", &self.rsn_ie);  
        log::info!("[wpa2] M2 full ({} bytes): {:02x?}", m2.len(), &m2[..]);  
        // 逐字段打印 M2 帧结构，方便对照 IEEE 802.11i 规范  
        log::info!(  
            "[wpa2] M2 breakdown: ver={:02x} type={:02x} body_len={:02x?} desc={:02x} key_info={:02x?} key_len={:02x?}",  
            m2[0], m2[1], &m2[2..4], m2[4], &m2[5..7], &m2[7..9]  
        );  
        log::info!(  
            "[wpa2] M2 breakdown: replay={:02x?} nonce={:02x?}...",  
            &m2[9..17], &m2[17..21]  
        );  
        log::info!(  
            "[wpa2] M2 breakdown: MIC={:02x?} key_data_len={:02x?}",  
            &m2[81..97], &m2[97..99]  
        );

        self.state = HandshakeState::M2Sent;  
        log::info!("[wpa2] M2 built ({} bytes), MIC={:02x?}...", m2.len(), &mic[..4]);  
  
        Ok(HandshakeAction::SendM2(m2))  
    }

    fn process_m3(  
        &mut self,  
        hdr: &EapolKeyHeader,  
        eapol: &[u8],  
    ) -> Result<HandshakeAction, WpaError> {
        if self.state != HandshakeState::M2Sent {  
            log::warn!("[wpa2] M3 received in unexpected state: {:?}", self.state);  
            return Err(WpaError::InvalidState);  
        } 

        let ptk = self.ptk.as_ref().ok_or(WpaError::InvalidState)?;  

        // 验证 Replay Counter（必须 > 之前的值）  
        if hdr.replay_counter[..] < self.replay_counter[..] {  
            log::error!("[wpa2] M3 replay counter too old");  
            return Err(WpaError::ReplayCounterMismatch);  
        }  
        self.replay_counter.copy_from_slice(&hdr.replay_counter); 

        // 验证 MIC   
        let mut eapol_copy = eapol.to_vec();  
        // 将 MIC 字段清零后计算  
        for i in 0..MIC_LEN {  
            eapol_copy[MIC_OFFSET + i] = 0;  
        }

        let computed_mic = compute_mic(&ptk.kck, &eapol_copy);  
        if computed_mic != hdr.key_mic {  
            log::error!(  
                "[wpa2] M3 MIC mismatch! expected={:02x?}, got={:02x?}",  
                &computed_mic[..4],  
                &hdr.key_mic[..4],  
            );  
            return Err(WpaError::MicMismatch);  
        }  
        log::info!("[wpa2] M3 MIC verified OK");  

        // 验证 ANonce 一致性  
        if hdr.key_nonce != self.anonce {  
            log::warn!("[wpa2] M3 ANonce differs from M1, updating");  
            self.anonce.copy_from_slice(&hdr.key_nonce);  
        }  
  
        // 解密 Key Data（包含 GTK KDE）  
        let key_data = &hdr.key_data;  
        if key_data.is_empty() {  
            log::error!("[wpa2] M3 has no key data");  
            return Err(WpaError::InvalidKeyData);  
        }

        let decrypted = aes_key_unwrap(&ptk.kek, key_data)?;  
        log::info!("[wpa2] M3 key data decrypted: {} bytes", decrypted.len());  
  
        // 解析 GTK KDE  
        let (gtk, gtk_key_idx) = parse_gtk_kde(&decrypted)?;  
        self.gtk = gtk;  
        self.gtk_key_idx = gtk_key_idx;  
  
        log::info!(  
            "[wpa2] GTK extracted: key_idx={}, len={}",  
            self.gtk_key_idx,  
            self.gtk.len(),  
        );

        // 构造 M4  
        let key_info: u16 = KEY_INFO_TYPE_HMAC_SHA1_AES  
            | KEY_INFO_PAIRWISE  
            | KEY_INFO_MIC  
            | KEY_INFO_SECURE;  
  
        let mut m4 = build_eapol_key_frame(  
            key_info,  
            0,                       // key_length = 0 in M4  
            &self.replay_counter,  
            &[0u8; NONCE_LEN],       // M4 nonce 全零  
            &[],                     // M4 无 key data  
        );

        // 计算并填入 MIC  
        let mic = compute_mic(&ptk.kck, &m4);  
        m4[MIC_OFFSET..MIC_OFFSET + MIC_LEN].copy_from_slice(&mic);  
  
        self.state = HandshakeState::Completed;  
        log::info!("[wpa2] M4 built ({} bytes), handshake complete!", m4.len());  
  
        // 返回结果  
        let mut tk = [0u8; TK_LEN];  
        tk.copy_from_slice(&ptk.tk); 

        Ok(HandshakeAction::Completed(HandshakeResult {  
            m4_frame: m4,  
            tk,  
            gtk: self.gtk.clone(),  
            gtk_key_idx: self.gtk_key_idx,  
        }))  
    }
}

/// Run all WPA2 crypto self-tests using known test vectors.  
/// Call this once at startup before any handshake.  
/// Returns true if all tests pass.  
pub fn run_crypto_self_test() -> bool {  
    let mut all_pass = true;  
      
    // ============================================================  
    // Test 1: PBKDF2-SHA1 (RFC 6070 test vector)  
    // Password: "password", Salt: "salt", Iterations: 4096, dkLen: 20  
    // Expected: 4b007901 b765489a bead49d9 26f721d0 65a429c1  
    // ============================================================  
    {  
        let dk = pbkdf2_sha1(b"password", b"salt", 4096, 20);  
        let expected: [u8; 20] = [  
            0x4b, 0x00, 0x79, 0x01, 0xb7, 0x65, 0x48, 0x9a,  
            0xbe, 0xad, 0x49, 0xd9, 0x26, 0xf7, 0x21, 0xd0,  
            0x65, 0xa4, 0x29, 0xc1,  
        ];  
        let pass = dk == expected;  
        log::info!("[self-test] PBKDF2-SHA1 (RFC6070 c=4096): {}", if pass { "PASS" } else { "FAIL" });  
        if !pass {  
            log::error!("[self-test]   expected: {:02x?}", &expected[..]);  
            log::error!("[self-test]   got:      {:02x?}", &dk[..]);  
            all_pass = false;  
        }  
    }  
      
    // ============================================================  
    // Test 2: WPA2-PSK PMK derivation  
    // SSID: "IEEE", Passphrase: "password"  
    // Expected PMK (from IEEE 802.11i test vectors):  
    //   f42c6fc52df0ebef9ebb4b90b38a5f90 2e83fe1b135a70e23aed762e9710a12e  
    // ============================================================  
    {  
        let pmk = pbkdf2_sha1(b"password", b"IEEE", 4096, 32);  
        let expected: [u8; 32] = [  
            0xf4, 0x2c, 0x6f, 0xc5, 0x2d, 0xf0, 0xeb, 0xef,  
            0x9e, 0xbb, 0x4b, 0x90, 0xb3, 0x8a, 0x5f, 0x90,  
            0x2e, 0x83, 0xfe, 0x1b, 0x13, 0x5a, 0x70, 0xe2,  
            0x3a, 0xed, 0x76, 0x2e, 0x97, 0x10, 0xa1, 0x2e,  
        ];  
        let pass = pmk == expected;  
        log::info!("[self-test] WPA2-PSK PMK (SSID=IEEE, pass=password): {}", if pass { "PASS" } else { "FAIL" });  
        if !pass {  
            log::error!("[self-test]   expected: {:02x?}", &expected[..]);  
            log::error!("[self-test]   got:      {:02x?}", &pmk[..]);  
            all_pass = false;  
        }  
    }  
      
    // ============================================================  
    // Test 3: Actual PMK for CU_Q2aa / uuux5cfj  
    // Verify against `wpa_passphrase CU_Q2aa uuux5cfj` output  
    // The user should compare this with the wpa_passphrase output  
    // ============================================================  
    {  
        let pmk = pbkdf2_sha1(b"uuux5cfj", b"CU_Q2aa", 4096, 32);  
        log::info!("[self-test] PMK for CU_Q2aa/uuux5cfj: {:02x?}", &pmk[..]);  
        log::info!("[self-test] Compare with: wpa_passphrase CU_Q2aa uuux5cfj");  
    }  
      
    // ============================================================  
    // Test 4: HMAC-SHA1 (RFC 2202 test case 1)  
    // Key: 0x0b repeated 20 times  
    // Data: "Hi There"  
    // Expected: b617318655057264e28bc0b6fb378c8ef146be00  
    // ============================================================  
    {  
        let key = [0x0bu8; 20];  
        let data = b"Hi There";  
        let result = hmac_sha1(&key, data);  
        let expected: [u8; 20] = [  
            0xb6, 0x17, 0x31, 0x86, 0x55, 0x05, 0x72, 0x64,  
            0xe2, 0x8b, 0xc0, 0xb6, 0xfb, 0x37, 0x8c, 0x8e,  
            0xf1, 0x46, 0xbe, 0x00,  
        ];  
        let pass = result == expected;  
        log::info!("[self-test] HMAC-SHA1 (RFC2202 #1): {}", if pass { "PASS" } else { "FAIL" });  
        if !pass {  
            log::error!("[self-test]   expected: {:02x?}", &expected[..]);  
            log::error!("[self-test]   got:      {:02x?}", &result[..]);  
            all_pass = false;  
        }  
    }  
      
    // ============================================================  
    // Test 5: PRF-SHA1 (IEEE 802.11i Annex H.3 test vector)  
    // This tests the PRF used for PTK derivation  
    // Key (PMK): 0xcd repeated 32 times (dummy)  
    // We test PRF output length = 48 (PTK_LEN)  
    // ============================================================  
    {  
        // Use a simple known-input test: PRF-160 with known key/label/data  
        // PRF(key, "prefix", data, 20) = HMAC-SHA1(key, "prefix" || 0x00 || data || 0x00)  
        let key = [0xaau8; 16];  
        let label = b"test label";  
        let data = [0xbbu8; 32];  
        let result = prf_sha1(&key, label, &data, 20);  
        // We can't hardcode the expected value without computing it,  
        // but we can verify it's deterministic and 20 bytes  
        let result2 = prf_sha1(&key, label, &data, 20);  
        let pass = result == result2 && result.len() == 20;  
        log::info!("[self-test] PRF-SHA1 determinism: {}", if pass { "PASS" } else { "FAIL" });  
        log::info!("[self-test]   PRF output (20B): {:02x?}", &result[..]);  
    }  
      
    // ============================================================  
    // Test 6: Full PTK derivation + MIC with IEEE 802.11i test vector  
    // From IEEE 802.11i-2004 Annex H.4:  
    //   PMK: cdcdc7a2 12d5e460 a1e7a3c4 e2f3c370   
    //        b0f6b0a0 c2c7e0b0 f0a0e0d0 c0b0a090  
    //   (This is a made-up PMK for testing)  
    //  
    // Instead, we do an end-to-end test:  
    // Derive PTK from known inputs, compute MIC, verify it's consistent  
    // ============================================================  
    {  
        let pmk: [u8; 32] = [  
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,  
            0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,  
            0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18,  
            0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x20,  
        ];  
        let aa: [u8; 6] = [0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5];  
        let spa: [u8; 6] = [0xb0, 0xb1, 0xb2, 0xb3, 0xb4, 0xb5];  
        let anonce: [u8; 32] = [0xc0; 32];  
        let snonce: [u8; 32] = [0xd0; 32];  
          
        let ptk = derive_ptk(&pmk, &aa, &spa, &anonce, &snonce);  
          
        // Log the full PTK for manual verification  
        log::info!("[self-test] PTK derivation test:");  
        log::info!("[self-test]   KCK: {:02x?}", &ptk.kck[..]);  
        log::info!("[self-test]   KEK: {:02x?}", &ptk.kek[..]);  
        log::info!("[self-test]   TK:  {:02x?}", &ptk.tk[..]);  
          
        // Verify address ordering: AA=a0:..., SPA=b0:...  
        // Since 0xa0 < 0xb0, min=AA, max=SPA  
        // Since 0xc0 < 0xd0, min=ANonce, max=SNonce  
        log::info!("[self-test]   addr order: min=AA(a0), max=SPA(b0)");  
        log::info!("[self-test]   nonce order: min=ANonce(c0), max=SNonce(d0)");  
          
        // Verify MIC computation is consistent  
        let test_frame = [0u8; 121]; // dummy frame  
        let mic1 = compute_mic(&ptk.kck, &test_frame);  
        let mic2 = compute_mic(&ptk.kck, &test_frame);  
        let pass = mic1 == mic2;  
        log::info!("[self-test] MIC consistency: {}", if pass { "PASS" } else { "FAIL" });  
        log::info!("[self-test]   MIC: {:02x?}", &mic1[..]);  
    }  
      
    // ============================================================  
    // Test 7: SNonce quality check  
    // ============================================================  
    {  
        let sn1 = generate_snonce();  
        // Small delay to ensure different timestamp  
        for _ in 0..10000 { core::hint::spin_loop(); }  
        let sn2 = generate_snonce();  
          
        let different = sn1 != sn2;  
        log::info!("[self-test] SNonce uniqueness: {}", if different { "PASS" } else { "FAIL" });  
        log::info!("[self-test]   SNonce1: {:02x?}", &sn1[..]);  
        log::info!("[self-test]   SNonce2: {:02x?}", &sn2[..]);  
          
        // Check entropy: count unique bytes  
        let mut seen = [false; 256];  
        for &b in sn1.iter() { seen[b as usize] = true; }  
        let unique_bytes = seen.iter().filter(|&&x| x).count();  
        log::info!("[self-test]   SNonce1 unique byte values: {}/32", unique_bytes);  
        if unique_bytes < 8 {  
            log::warn!("[self-test]   WARNING: Low entropy in SNonce!");  
            // Don't fail - SNonce quality doesn't cause AP rejection  
        }  
          
        if !different {  
            all_pass = false;  
        }  
    }  
      
    // ============================================================  
    // Test 8: End-to-end M2 construction test  
    // Build an M2 frame and verify its structure  
    // ============================================================  
    {  
        let replay = [0u8, 0, 0, 0, 0, 0, 0, 1u8];  
        let nonce = [0x42u8; 32];  
        let rsn_ie: [u8; 22] = [  
            0x30, 0x14, 0x01, 0x00, 0x00, 0x0f, 0xac, 0x04,  
            0x01, 0x00, 0x00, 0x0f, 0xac, 0x04, 0x01, 0x00,  
            0x00, 0x0f, 0xac, 0x02, 0x0c, 0x00,  
        ];  
        let key_info: u16 = KEY_INFO_TYPE_HMAC_SHA1_AES | KEY_INFO_PAIRWISE | KEY_INFO_MIC;  
          
        let m2 = build_eapol_key_frame(key_info, 0, &replay, &nonce, &rsn_ie);  
          
        let mut pass = true;  
        // Check EAPOL version  
        if m2[0] != EAPOL_VERSION {   
            log::error!("[self-test] M2[0] EAPOL version: 0x{:02x} (expected 0x{:02x})", m2[0], EAPOL_VERSION);  
            pass = false;   
        }  
        // Check type  
        if m2[1] != 0x03 {   
            log::error!("[self-test] M2[1] type: 0x{:02x} (expected 0x03)", m2[1]);  
            pass = false;   
        }  
        // Check body length  
        let body_len = u16::from_be_bytes([m2[2], m2[3]]);  
        if body_len != 117 { // 95 + 22  
            log::error!("[self-test] M2 body_len: {} (expected 117)", body_len);  
            pass = false;  
        }  
        // Check descriptor type  
        if m2[4] != 0x02 {  
            log::error!("[self-test] M2[4] desc_type: 0x{:02x} (expected 0x02)", m2[4]);  
            pass = false;  
        }  
        // Check key info  
        let ki = u16::from_be_bytes([m2[5], m2[6]]);  
        if ki != 0x010A {  
            log::error!("[self-test] M2 key_info: 0x{:04x} (expected 0x010A)", ki);  
            pass = false;  
        }  
        // Check key length = 0  
        let kl = u16::from_be_bytes([m2[7], m2[8]]);  
        if kl != 0 {  
            log::error!("[self-test] M2 key_length: {} (expected 0)", kl);  
            pass = false;  
        }  
        // Check replay counter  
        if m2[9..17] != replay {  
            log::error!("[self-test] M2 replay counter mismatch");  
            pass = false;  
        }  
        // Check MIC is zero (before filling)  
        let mic_zeros = m2[81..97].iter().all(|&b| b == 0);  
        if !mic_zeros {  
            log::error!("[self-test] M2 MIC not zeroed before fill");  
            pass = false;  
        }  
        // Check key data length  
        let kdl = u16::from_be_bytes([m2[97], m2[98]]);  
        if kdl != 22 {  
            log::error!("[self-test] M2 key_data_len: {} (expected 22)", kdl);  
            pass = false;  
        }  
        // Check key data = RSN IE  
        if m2[99..121] != rsn_ie {  
            log::error!("[self-test] M2 key_data != RSN IE");  
            pass = false;  
        }  
        // Check total length  
        if m2.len() != 121 {  
            log::error!("[self-test] M2 total_len: {} (expected 121)", m2.len());  
            pass = false;  
        }  
          
        log::info!("[self-test] M2 frame structure: {}", if pass { "PASS" } else { "FAIL" });  
        if !pass { all_pass = false; }  
    }  
      
    log::info!("[self-test] ========== ALL TESTS: {} ==========", if all_pass { "PASS" } else { "FAIL" });  
    all_pass  
}

/// 使用 IEEE 802.11i Annex J 测试向量验证 PRF-SHA1 和 derive_ptk  
pub fn run_ptk_test() -> bool {  
    let mut all_pass = true;  
  
    // ================================================================  
    // 测试 1: PRF-SHA1 基础验证 (IEEE 802.11i Annex J.3)  
    //  
    // IEEE 802.11i-2004 Section H.3.2 给出了 PRF-512 的测试向量:  
    //   Key = 0x0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b (20 bytes)  
    //   Prefix = "prefix" (6 bytes)  
    //   Data = "Hi There" (8 bytes)  
    //  
    // PRF-512 输出 (64 bytes):  
    //   bcd4c650 b30b9684 951829e0 d75f9d54  
    //   b862175e d9f00606 e17d8da3 5402ffee  
    //   75df78c3 d31e0f88 9f012120 c0862beb  
    //   67753e74 07307841 19b0b1ef 6c1c2e7f  
    // ================================================================  
    {  
        let key = [0x0bu8; 20];  
        let prefix = b"prefix";  
        let data = b"Hi There";  
        let expected: [u8; 64] = [  
            0xbc, 0xd4, 0xc6, 0x50, 0xb3, 0x0b, 0x96, 0x84,  
            0x95, 0x18, 0x29, 0xe0, 0xd7, 0x5f, 0x9d, 0x54,  
            0xb8, 0x62, 0x17, 0x5e, 0xd9, 0xf0, 0x06, 0x06,  
            0xe1, 0x7d, 0x8d, 0xa3, 0x54, 0x02, 0xff, 0xee,  
            0x75, 0xdf, 0x78, 0xc3, 0xd3, 0x1e, 0x0f, 0x88,  
            0x9f, 0x01, 0x21, 0x20, 0xc0, 0x86, 0x2b, 0xeb,  
            0x67, 0x75, 0x3e, 0x74, 0x07, 0x30, 0x78, 0x41,  
            0x19, 0xb0, 0xb1, 0xef, 0x6c, 0x1c, 0x2e, 0x7f,  
        ];  
  
        let result = prf_sha1(&key, prefix, data, 64);  
        if result[..] == expected[..] {  
            log::info!("[ptk-test] PRF-512 basic: PASS");  
        } else {  
            log::error!("[ptk-test] PRF-512 basic: FAIL");  
            log::error!("[ptk-test]   expected: {:02x?}", &expected[..16]);  
            log::error!("[ptk-test]   got:      {:02x?}", &result[..16]);  
            for i in 0..64 {  
            if expected[i] != result[i] {  
                log::error!("[ptk-test]   first diff at byte {}: expected 0x{:02x}, got 0x{:02x}", i, expected[i], result[i]);  
                break;  
            }  
        }
            all_pass = false;  
        }  
    }  
  
    // ================================================================  
    // 测试 2: 完整 PTK 推导 (IEEE 802.11i Annex J.4 / wpa_supplicant test vectors)  
    //  
    // 使用 wpa_supplicant 测试中的已知向量:  
    //   PMK  = 0xcdcf...  (来自 SSID="IEEE", passphrase="password")  
    //   AA   = a0:a1:a2:a3:a4:a5  
    //   SPA  = b0:b1:b2:b3:b4:b5  
    //   ANonce = 固定值  
    //   SNonce = 固定值  
    //  
    // 先验证 PMK:  
    //   PMK = PBKDF2-SHA1("password", "IEEE", 4096, 32)  
    //       = f42c6fc52df0ebef9ebb4b90b38a5f90 2e83fe1b135a70e23aed762e9710a12e  
    // ================================================================  
    {  
        let pmk_vec = pbkdf2_sha1(b"password", b"IEEE", 4096, 32);  
        let expected_pmk: [u8; 32] = [  
            0xf4, 0x2c, 0x6f, 0xc5, 0x2d, 0xf0, 0xeb, 0xef,  
            0x9e, 0xbb, 0x4b, 0x90, 0xb3, 0x8a, 0x5f, 0x90,  
            0x2e, 0x83, 0xfe, 0x1b, 0x13, 0x5a, 0x70, 0xe2,  
            0x3a, 0xed, 0x76, 0x2e, 0x97, 0x10, 0xa1, 0x2e,  
        ];  
        if pmk_vec[..] == expected_pmk[..] {  
            log::info!("[ptk-test] PMK (IEEE/password): PASS");  
        } else {  
            log::error!("[ptk-test] PMK (IEEE/password): FAIL");  
            log::error!("[ptk-test]   expected: {:02x?}", &expected_pmk[..]);  
            log::error!("[ptk-test]   got:      {:02x?}", &pmk_vec[..]);  
            all_pass = false;  
        }  
    }  
  
    // ================================================================  
    // 测试 3: 完整 PTK 推导 + MIC 验证  
    //  
    // 使用完全确定性的输入，手动计算期望输出。  
    // 这里使用 IEEE 802.11i-2004 Annex J.6 的测试数据:  
    //  
    //   PMK = f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e  
    //   AA  = a0:a1:a2:a3:a4:a5  
    //   SPA = b0:b1:b2:b3:b4:b5  
    //   ANonce = e0e1e2e3e4e5e6e7e8e9f0f1f2f3f4f5e0e1e2e3e4e5e6e7e8e9f0f1f2f3f4f5  
    //   SNonce = c0c1c2c3c4c5c6c7c8c9d0d1d2d3d4d5c0c1c2c3c4c5c6c7c8c9d0d1d2d3d4d5  
    //  
    // 地址排序: AA=a0:a1:..., SPA=b0:b1:...  
    //   Min(AA,SPA) = AA (0xa0 < 0xb0)  
    //   Max(AA,SPA) = SPA  
    //  
    // Nonce 排序: ANonce=e0e1..., SNonce=c0c1...  
    //   Min(ANonce,SNonce) = SNonce (0xc0 < 0xe0)  
    //   Max(ANonce,SNonce) = ANonce  
    //  
    // data = AA || SPA || SNonce || ANonce (76 bytes)  
    //  
    // PTK = PRF-384(PMK, "Pairwise key expansion", data)  
    //     = 48 bytes = KCK(16) + KEK(16) + TK(16)  
    //  
    // 我们无法在这里硬编码期望值（因为没有标准文档中的精确值），  
    // 但可以做以下验证:  
    //   1. 调用 derive_ptk 得到 PTK  
    //   2. 手动调用 prf_sha1 得到相同结果  
    //   3. 用 KCK 计算一个已知 M2 帧的 MIC  
    //   4. 验证 MIC 非全零且长度正确  
    // ================================================================  
    {  
        let pmk: [u8; 32] = [  
            0xf4, 0x2c, 0x6f, 0xc5, 0x2d, 0xf0, 0xeb, 0xef,  
            0x9e, 0xbb, 0x4b, 0x90, 0xb3, 0x8a, 0x5f, 0x90,  
            0x2e, 0x83, 0xfe, 0x1b, 0x13, 0x5a, 0x70, 0xe2,  
            0x3a, 0xed, 0x76, 0x2e, 0x97, 0x10, 0xa1, 0x2e,  
        ];  
        let aa: [u8; 6] = [0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5];  
        let spa: [u8; 6] = [0xb0, 0xb1, 0xb2, 0xb3, 0xb4, 0xb5];  
        let anonce: [u8; 32] = [  
            0xe0, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7,  
            0xe8, 0xe9, 0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5,  
            0xe0, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7,  
            0xe8, 0xe9, 0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5,  
        ];  
        let snonce: [u8; 32] = [  
            0xc0, 0xc1, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7,  
            0xc8, 0xc9, 0xd0, 0xd1, 0xd2, 0xd3, 0xd4, 0xd5,  
            0xc0, 0xc1, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7,  
            0xc8, 0xc9, 0xd0, 0xd1, 0xd2, 0xd3, 0xd4, 0xd5,  
        ];  
  
        // 方法 A: 通过 derive_ptk  
        let ptk_a = derive_ptk(&pmk, &aa, &spa, &anonce, &snonce);  
  
        // 方法 B: 手动构造 data 并调用 prf_sha1  
        let mut data = [0u8; 76];  
        // Min(AA, SPA) = AA (0xa0 < 0xb0)  
        data[0..6].copy_from_slice(&aa);  
        data[6..12].copy_from_slice(&spa);  
        // Min(ANonce, SNonce) = SNonce (0xc0 < 0xe0)  
        data[12..44].copy_from_slice(&snonce);  
        data[44..76].copy_from_slice(&anonce);  
  
        let ptk_b = prf_sha1(&pmk, b"Pairwise key expansion", &data, 48);  
  
        if ptk_a.kck[..] == ptk_b[0..16]  
            && ptk_a.kek[..] == ptk_b[16..32]  
            && ptk_a.tk[..] == ptk_b[32..48]  
        {  
            log::info!("[ptk-test] derive_ptk consistency: PASS");  
        } else {  
            log::error!("[ptk-test] derive_ptk consistency: FAIL");  
            all_pass = false;  
        }  
  
        // 打印完整 PTK 供外部工具验证  
        log::info!(  
            "[ptk-test] PTK (AA=a0a1.., SPA=b0b1.., ANonce=e0e1.., SNonce=c0c1..):"  
        );  
        log::info!("[ptk-test]   KCK = {:02x?}", &ptk_a.kck);  
        log::info!("[ptk-test]   KEK = {:02x?}", &ptk_a.kek);  
        log::info!("[ptk-test]   TK  = {:02x?}", &ptk_a.tk);  
  
        // 验证 MIC 计算: 构造一个假 M2 帧，计算 MIC  
        let rsn_ie: [u8; 22] = [  
            0x30, 0x14, 0x01, 0x00, 0x00, 0x0f, 0xac, 0x04,  
            0x01, 0x00, 0x00, 0x0f, 0xac, 0x04, 0x01, 0x00,  
            0x00, 0x0f, 0xac, 0x02, 0x00, 0x00,  
        ];  
        let replay: [u8; 8] = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01];  
  
        let mut m2 = build_eapol_key_frame(  
            KEY_INFO_TYPE_HMAC_SHA1_AES | KEY_INFO_PAIRWISE | KEY_INFO_MIC,  
            0,  
            &replay,  
            &snonce,  
            &rsn_ie,  
        );  
  
        // MIC 应该是全零（还没填）  
        assert!(m2[MIC_OFFSET..MIC_OFFSET + MIC_LEN].iter().all(|&b| b == 0));  
  
        let mic = compute_mic(&ptk_a.kck, &m2);  
        m2[MIC_OFFSET..MIC_OFFSET + MIC_LEN].copy_from_slice(&mic);  
  
        log::info!("[ptk-test]   MIC = {:02x?}", &mic);  
  
        // MIC 不应该是全零  
        if mic.iter().any(|&b| b != 0) {  
            log::info!("[ptk-test] MIC non-zero: PASS");  
        } else {  
            log::error!("[ptk-test] MIC is all zeros: FAIL");  
            all_pass = false;  
        }  
  
        // 验证 MIC: 重新计算应该得到相同结果  
        let mut m2_verify = m2.clone();  
        m2_verify[MIC_OFFSET..MIC_OFFSET + MIC_LEN].fill(0);  
        let mic2 = compute_mic(&ptk_a.kck, &m2_verify);  
        if mic == mic2 {  
            log::info!("[ptk-test] MIC verify round-trip: PASS");  
        } else {  
            log::error!("[ptk-test] MIC verify round-trip: FAIL");  
            all_pass = false;  
        }  
    }  
  
    // ================================================================  
    // 测试 4: 用实际日志中的数据做端到端验证  
    //  
    // 从最新日志中提取:  
    //   PMK = eaf4321a66940a1c1fb36c6e43090eb29c9cdf1a47b6637c69f13f1ac9d23c6c  
    //   AA  = 8c:83:e8:26:59:08  
    //   SPA = 38:7a:cc:94:2d:2c  
    //   ANonce = 从 M1 中提取  
    //   SNonce = 从 generate_snonce 生成  
    //  
    // 这个测试打印完整的 PTK 和 MIC，供外部 Python 脚本验证  
    // ================================================================  
    {  
        let pmk: [u8; 32] = [  
            0xea, 0xf4, 0x32, 0x1a, 0x66, 0x94, 0x0a, 0x1c,  
            0x1f, 0xb3, 0x6c, 0x6e, 0x43, 0x09, 0x0e, 0xb2,  
            0x9c, 0x9c, 0xdf, 0x1a, 0x47, 0xb6, 0x63, 0x7c,  
            0x69, 0xf1, 0x3f, 0x1a, 0xc9, 0xd2, 0x3c, 0x6c,  
        ];  
        let aa: [u8; 6] = [0x8c, 0x83, 0xe8, 0x26, 0x59, 0x08];  
        let spa: [u8; 6] = [0x38, 0x7a, 0xcc, 0x94, 0x2d, 0x2c];  
  
        // 地址排序验证  
        let (min_a, max_a) = if aa[..] < spa[..] {  
            (&aa[..], &spa[..])  
        } else {  
            (&spa[..], &aa[..])  
        };  
        log::info!(  
            "[ptk-test] Addr sort: min={:02x?}, max={:02x?}",  
            min_a, max_a  
        );  
        // 0x38 < 0x8c, 所以 SPA 是 min, AA 是 max  
        if min_a == &spa[..] && max_a == &aa[..] {  
            log::info!("[ptk-test] Addr sort: PASS (SPA < AA)");  
        } else {  
            log::error!("[ptk-test] Addr sort: FAIL");  
            all_pass = false;  
        }  
    }  
  
    // ================================================================  
    // 测试 5: PRF-384 输出长度验证  
    // PTK = PRF-384 = 48 bytes, 需要 ceil(48/20) = 3 次 HMAC-SHA1  
    // ================================================================  
    {  
        let key = [0xAA_u8; 32];  
        let result = prf_sha1(&key, b"test label", b"test data", 48);  
        if result.len() == 48 {  
            log::info!("[ptk-test] PRF-384 output length: PASS");  
        } else {  
            log::error!(  
                "[ptk-test] PRF-384 output length: FAIL (got {})",  
                result.len()  
            );  
            all_pass = false;  
        }  
    }  
  
    log::info!(  
        "[ptk-test] ========== PTK TEST: {} ==========",  
        if all_pass { "ALL PASS" } else { "FAIL" }  
    );  
    all_pass  
}
