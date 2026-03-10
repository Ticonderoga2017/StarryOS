//! AIC8800 芯片型号和固件地址常量 

// ============================================================  
// SDIO Vendor / Device ID  
// ============================================================  
pub const VID_AIC8801: u16 = 0x5449;  
pub const VID_AIC8800DC: u16 = 0xc8a1;  
pub const VID_AIC8800D80: u16 = 0xc8a1;  
pub const VID_AIC8800D80X2: u16 = 0xc8a1;  
  
pub const DID_AIC8801: u16 = 0x0145;  
pub const DID_AIC8800DC: u16 = 0xc08d;  
pub const DID_AIC8800D80: u16 = 0x0082;  
pub const DID_AIC8800D80X2: u16 = 0x2082;  

// ============================================================  
// 芯片版本 (chip_rev) — 值来自寄存器 0x40500000 >> 16  
// ============================================================  
pub const CHIP_REV_U01: u8 = 1;  
pub const CHIP_REV_U02: u8 = 3;  
pub const CHIP_REV_U03: u8 = 7;  
pub const CHIP_REV_U04: u8 = 7;  // 与 U03 相同  

// ============================================================  
// 固件 RAM 地址  
// ============================================================  
/// WiFi FMAC 固件加载地址 (AIC8801/D80/D80X2)  
pub const RAM_FMAC_FW_ADDR: u32 = 0x0012_0000;  
/// WiFi FMAC 固件补丁地址 (AIC8801)  
pub const RAM_FMAC_FW_PATCH_ADDR: u32 = 0x0019_0000;  
/// ROM FMAC 固件补丁地址 (AIC8800DC)  
pub const ROM_FMAC_PATCH_ADDR: u32 = 0x0018_0000;    
/// 芯片版本寄存器地址 (所有型号通用)  
pub const CHIP_REV_ADDR: u32 = 0x4050_0000;  

// ============================================================  
// AIC8800 SDIO 功能寄存器 — V1 (AIC8801/DC/DW)  
// ============================================================  
pub const SDIOWIFI_FUNC_BLOCKSIZE: u16 = 512;  
pub const SDIOWIFI_BYTEMODE_LEN_REG: u32 = 0x02;  
pub const SDIOWIFI_INTR_CONFIG_REG: u32 = 0x04;  
pub const SDIOWIFI_SLEEP_REG: u32 = 0x05;  
pub const SDIOWIFI_WR_FIFO_ADDR: u32 = 0x07;  
pub const SDIOWIFI_RD_FIFO_ADDR: u32 = 0x08;  
pub const SDIOWIFI_WAKEUP_REG: u32 = 0x09;  
pub const SDIOWIFI_FLOW_CTRL_REG: u32 = 0x0A;  
pub const SDIOWIFI_REGISTER_BLOCK: u32 = 0x0B;  
pub const SDIOWIFI_BYTEMODE_ENABLE_REG: u32 = 0x11;  
pub const SDIOWIFI_BLOCK_CNT_REG: u32 = 0x12;  
pub const SDIOWIFI_FLOWCTRL_MASK: u8 = 0x7F;  

// ============================================================  
// AIC8800 SDIO 功能寄存器 — V3 (D80/D80X2)  
// ============================================================  
pub const SDIOWIFI_INTR_ENABLE_REG_V3: u32 = 0x00;  
/// V3 "sleep_reg" = INTR_PENDING_REG (读取 bit4=1 表示芯片就绪)  
pub const SDIOWIFI_SLEEP_REG_V3: u32 = 0x01;  
/// V3 "wakeup_reg" = INTR_TO_DEVICE_REG (写 0x11 唤醒芯片)  
pub const SDIOWIFI_WAKEUP_REG_V3: u32 = 0x02;  
pub const SDIOWIFI_FLOW_CTRL_Q1_REG_V3: u32 = 0x03;  
pub const SDIOWIFI_MISC_INT_STATUS_REG_V3: u32 = 0x04;  
pub const SDIOWIFI_BYTEMODE_LEN_REG_V3: u32 = 0x05;  
pub const SDIOWIFI_BYTEMODE_LEN_MSB_REG_V3: u32 = 0x06;  
pub const SDIOWIFI_BYTEMODE_ENABLE_REG_V3: u32 = 0x07;  
pub const SDIOWIFI_MISC_CTRL_REG_V3: u32 = 0x08;  
pub const SDIOWIFI_FLOW_CTRL_Q2_REG_V3: u32 = 0x09;  
pub const SDIOWIFI_CLK_TEST_RESULT_REG_V3: u32 = 0x0A;  
pub const SDIOWIFI_RD_FIFO_ADDR_V3: u32 = 0x0F;  
pub const SDIOWIFI_WR_FIFO_ADDR_V3: u32 = 0x10;  

// ============================================================  
// IPC 消息类型  
// ============================================================  
pub const SDIO_TYPE_DATA: u8 = 0x00;  
pub const SDIO_TYPE_CFG: u8 = 0x10;  
pub const SDIO_TYPE_CFG_CMD_RSP: u8 = 0x11;  
pub const SDIO_TYPE_CFG_DATA_CFM: u8 = 0x12;  

// ============================================================  
// 任务 ID  
// ============================================================  
pub const TASK_DBG: u16 = 1;  
pub const DRV_TASK_ID: u16 = 100;  
  
// ============================================================  
// Host start app 类型  
// ============================================================  
pub const HOST_START_APP_AUTO: u32 = 1;  
pub const HOST_START_APP_CUSTOM: u32 = 2;  
pub const HOST_START_APP_FNCALL: u32 = 4;  
pub const HOST_START_APP_DUMMY: u32 = 5;

// ============================================================  
// 芯片型号枚举  
// ============================================================  
#[derive(Debug, Clone, Copy, PartialEq, Eq)]  
pub enum ChipVariant {  
    Aic8801,  
    Aic8800DC,  
    Aic8800DW,  
    Aic8800D80,  
    Aic8800D80X2,  
    Unknown,
}  

impl ChipVariant {  
    pub fn from_vid_did(vid: u16, did: u16) -> Self {  
        match (vid, did) {  
            (VID_AIC8801, DID_AIC8801) => Self::Aic8801,  
            (VID_AIC8800DC, DID_AIC8800DC) => Self::Aic8800DC,  
            (VID_AIC8800D80, DID_AIC8800D80) => Self::Aic8800D80,  
            (VID_AIC8800D80X2, DID_AIC8800D80X2) => Self::Aic8800D80X2,  
            _ => Self::Unknown,  
        }  
    }  
    /// 是否为 SDIO V3 协议芯片 (D80/D80X2) 
    pub fn is_v3(&self) -> bool {  
        matches!(self, Self::Aic8800D80 | Self::Aic8800D80X2)  
    }  
}

/// 完整芯片修订信息  
#[derive(Debug, Clone, Copy)]  
pub struct ChipRevision {  
    pub rev: u8,            // 芯片版本号 (CHIP_REV_U01=1, U02=3, U03=7)  
    pub is_chip_id_h: bool, // 高性能变体标志 (仅 DC/D80)  
}  



// // ============================================================  
// // AIC8800 SDIO 功能寄存器 (Function 1 地址空间)  
// // ============================================================  
// /// 写 FIFO 地址 (CMD53 写入此地址发送消息)  
// pub const SDIOWIFI_FUNC_BLOCKSIZE: u16 = 512;  
// pub const SDIOWIFI_BYTEMODE_LEN_REG: u8 = 0x02;
// pub const SDIOWIFI_WR_FIFO_ADDR: u32 = 0x07;  
// /// 读 FIFO 地址 (CMD53 从此地址读取响应)  
// pub const SDIOWIFI_RD_FIFO_ADDR: u32 = 0x08;  
// /// 流控寄存器  
// pub const SDIOWIFI_FLOW_CTRL_REG: u32 = 0x0A;  
// /// 流控掩码  
// pub const SDIOWIFI_FLOWCTRL_MASK: u8 = 0x7F;  
// /// 块计数寄存器 (中断状态 / 数据块数)  
// pub const SDIOWIFI_BLOCK_CNT_REG: u32 = 0x12;  
// /// 中断配置寄存器  
// pub const SDIOWIFI_INTR_CONFIG_REG: u32 = 0x04;  
// /// 块模式寄存器  
// pub const SDIOWIFI_REGISTER_BLOCK: u32 = 0x0B;  
// /// 字节模式使能寄存器  
// pub const SDIOWIFI_BYTEMODE_ENABLE_REG: u32 = 0x11;  
// /// 唤醒寄存器  
// pub const SDIOWIFI_WAKEUP_REG: u32 = 0x09;  
// /// 休眠寄存器  
// pub const SDIOWIFI_SLEEP_REG: u32 = 0x05;  

// /// SDIO 功能寄存器地址 (AIC8800D80/D80X2, v3 协议)  
// pub const SDIOWIFI_FLOW_CTRL_Q1_REG_V3: u8 = 0x03;  
// pub const SDIOWIFI_MISC_INT_STATUS_REG_V3: u8 = 0x04; 
// pub const SDIOWIFI_RD_FIFO_ADDR_V3: u8 = 0x0F;  
// pub const SDIOWIFI_WR_FIFO_ADDR_V3: u8 = 0x10;  
  
// /// IPC 消息类型  
// pub const SDIO_TYPE_DATA: u8 = 0x00;  
// pub const SDIO_TYPE_CFG: u8 = 0x10;  
// pub const SDIO_TYPE_CFG_CMD_RSP: u8 = 0x11;  
// pub const SDIO_TYPE_CFG_DATA_CFM: u8 = 0x12;  
  
// /// 任务 ID  
// pub const TASK_DBG: u16 = 1;  
// pub const DRV_TASK_ID: u16 = 100;  
  
// // ============================================================  
// // Host start app 类型  
// // ============================================================  
// pub const HOST_START_APP_AUTO: u32 = 1;  
// pub const HOST_START_APP_DUMMY: u32 = 5;  

// // AIC8800 自定义 SDIO 寄存器 (AIC8801/DC/DW, v1 版本)  
// pub const SDIOWIFI_REGISTER_BLOCK:      u32 = 0x0B;  
// pub const SDIOWIFI_BYTEMODE_ENABLE_REG: u32 = 0x11;  
// pub const SDIOWIFI_WAKEUP_REG:          u32 = 0x09;  
// pub const SDIOWIFI_SLEEP_REG:           u32 = 0x05;  
  
// // AIC8800D80/D80X2 (v3 版本)  
// pub const SDIOWIFI_BYTEMODE_ENABLE_REG_V3: u32 = 0x07;  
// pub const SDIOWIFI_WAKEUP_REG_V3:          u32 = 0x02;  
// pub const SDIOWIFI_SLEEP_REG_V3:           u32 = 0x01;  