/// LMAC 消息头（对应 Linux struct lmac_msg）
#[repr(C)]
#[derive(Clone, Debug)]
pub struct LmacMsg {
    pub id: u16,
    pub dest_id: u16,
    pub src_id: u16,
    pub param_len: u16,
    pub pattern: u32, 
}

impl LmacMsg {
    pub const SIZE: usize = 12;

    /// 从字节切片解析 LmacMsg（小端序） 
    pub fn from_le_bytes(data: &[u8]) -> Self {
        Self { 
            id: u16::from_le_bytes([data[0], data[1]]),
            dest_id: u16::from_le_bytes([data[2], data[3]]), 
            src_id: u16::from_le_bytes([data[4], data[5]]), 
            param_len: u16::from_le_bytes([data[6], data[7]]), 
            pattern: u32::from_le_bytes([data[8], data[9], data[10], data[11]]), 
        }
    }

    /// 序列化为 8 字节小端序 
    pub fn to_le_bytes(&self) -> [u8; 8] {
        let mut buf = [0u8; 8];  
        buf[0..2].copy_from_slice(&self.id.to_le_bytes());  
        buf[2..4].copy_from_slice(&self.dest_id.to_le_bytes());  
        buf[4..6].copy_from_slice(&self.src_id.to_le_bytes());  
        buf[6..8].copy_from_slice(&self.param_len.to_le_bytes());  
        buf 
    }
}

/// MSG_T(task, idx) = (task << 8) | idx  
pub const fn msg_t(task: u16, idx: u8) -> u16 {  
    (task << 8) | (idx as u16)  
}

/// 构造宏：LMAC_FIRST_MSG(task) = (task << 10)
pub const fn lmac_first_msg(task: u16) -> u16 {
    task << 10
}

/// 从 msg_id 提取 message index: bits[9..0]  
pub const fn msg_index(msg_id: u16) -> u16 {  
    msg_id & ((1 << 10) - 1)  
} 

// Task IDs（对应 Linux lmac_msg.h — FDRV 版本，含 TASK_TDLS）  
pub const TASK_MM: u16 = 0;  
pub const TASK_DBG: u16 = 1;  
pub const TASK_SCAN: u16 = 2;  
pub const TASK_TDLS: u16 = 3;  
pub const TASK_SCANU: u16 = 4;  
pub const TASK_ME: u16 = 5;  
pub const TASK_SM: u16 = 6;  
pub const TASK_APM: u16 = 7;  
pub const TASK_BAM: u16 = 8;  
pub const TASK_MESH: u16 = 9;  
pub const TASK_RXU: u16 = 10;  
pub const TASK_RM: u16 = 11;  
pub const TASK_TWT: u16 = 12;  
pub const TASK_API: u16 = 13;  

/// Linux 驱动中所有 rwnx_msg_zalloc 调用使用 DRV_TASK_ID = 100 作为 src_id  
pub const DRV_TASK_ID: u16 = 100;  

// ============================================================  
// LMAC 消息 ID（TASK_MM = 0, LMAC_FIRST_MSG(0) = 0）  
// ============================================================  
pub const MM_SET_STACK_START_REQ: u16 = 0x007B; // 枚举偏移 123  
pub const MM_SET_STACK_START_CFM: u16 = 0x007C;  

// ========== MM messages (TASK_MM = 0, base = 0x0000) ==========
pub const MM_RESET_REQ:              u16 = 0x0000;
pub const MM_RESET_CFM:              u16 = 0x0001;
pub const MM_START_REQ:              u16 = 0x0002;
pub const MM_START_CFM:              u16 = 0x0003;
pub const MM_VERSION_REQ:            u16 = 0x0004;
pub const MM_VERSION_CFM:            u16 = 0x0005;
pub const MM_ADD_IF_REQ:             u16 = 0x0006;
pub const MM_ADD_IF_CFM:             u16 = 0x0007;
pub const MM_REMOVE_IF_REQ:          u16 = 0x0008;
pub const MM_REMOVE_IF_CFM:          u16 = 0x0009;
pub const MM_STA_ADD_REQ:            u16 = 0x000A;
pub const MM_STA_ADD_CFM:            u16 = 0x000B;
pub const MM_STA_DEL_REQ:            u16 = 0x000C;
pub const MM_STA_DEL_CFM:            u16 = 0x000D;
pub const MM_SET_FILTER_REQ:         u16 = 0x000E;
pub const MM_SET_FILTER_CFM:         u16 = 0x000F;
pub const MM_SET_CHANNEL_REQ:        u16 = 0x0010;
pub const MM_SET_CHANNEL_CFM:        u16 = 0x0011;
pub const MM_SET_IDLE_REQ:           u16 = 0x0022;
pub const MM_SET_IDLE_CFM:           u16 = 0x0023;
pub const MM_KEY_ADD_REQ:            u16 = 0x0024;
pub const MM_KEY_ADD_CFM:            u16 = 0x0025;

// RF 校准相关  
pub const MM_SET_RF_CONFIG_REQ:      u16 = 0x0067; // idx 103  
pub const MM_SET_RF_CONFIG_CFM:      u16 = 0x0068; // idx 104  
pub const MM_SET_RF_CALIB_REQ:       u16 = 0x0069; // idx 105  
pub const MM_SET_RF_CALIB_CFM:       u16 = 0x006A; // idx 106  
  
// MAC 地址  
pub const MM_GET_MAC_ADDR_REQ:       u16 = 0x0073; // idx 115  
pub const MM_GET_MAC_ADDR_CFM:       u16 = 0x0074; // idx 116  
  
// TX 功率  
pub const MM_SET_TXPWR_IDX_LVL_REQ: u16 = 0x0077; // idx 119  
pub const MM_SET_TXPWR_IDX_LVL_CFM: u16 = 0x0078; // idx 120  
pub const MM_SET_TXPWR_OFST_REQ:    u16 = 0x0079; // idx 121  
pub const MM_SET_TXPWR_OFST_CFM:    u16 = 0x007A; // idx 122

// ========== ME messages (TASK_ME = 5, base = 0x1400) ==========
pub const ME_CONFIG_REQ:             u16 = 0x1400;
pub const ME_CONFIG_CFM:             u16 = 0x1401;
pub const ME_CHAN_CONFIG_REQ:        u16 = 0x1402;
pub const ME_CHAN_CONFIG_CFM:        u16 = 0x1403;
pub const ME_SET_CONTROL_PORT_REQ:   u16 = 0x1404;
pub const ME_SET_CONTROL_PORT_CFM:   u16 = 0x1405;

// ========== VIF 类型 ==========
pub const MM_STA:  u8 = 0;
pub const MM_IBSS: u8 = 1;
pub const MM_AP:   u8 = 2;

// ========== PHY BW ==========
pub const PHY_CHNL_BW_20: u8 = 0;
pub const PHY_CHNL_BW_40: u8 = 1;
pub const PHY_CHNL_BW_80: u8 = 2;

/// CMD 超时（与 Linux RWNX_80211_CMD_TIMEOUT_MS 一致）  
pub const CMD_TIMEOUT_MS: u64 = 6000;  
  
// Frame construction constants  
pub const DUMMY_WORD_LEN: usize = 4;  
pub const TAIL_LEN: usize = 4;  
pub const CMD_TX_TIMEOUT_DEFAULT_MS: u64 = 5000;  

#[derive(Debug)]
pub enum CmdError {
    Timeout,
    BusDown, 
    SdioError, 
    InvalidResponse,
    MismatchedCfm { expected: u16, got: u16 },
    FirmwareError,
}

/// 2.4GHz 标准信道频率表 (信道 1-14)
pub const CHAN_2G4_FREQS: [u16; 14] = [
    2412, 2417, 2422, 2427, 2432, 2437, 2442,
    2447, 2452, 2457, 2462, 2467, 2472, 2484,
];
