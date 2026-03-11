//! AIC8800 IPC 消息协议  
//!  
//! 实现消息构建、SDIO 传输、响应解析。

use aic8800_sdio::{SdioHost, error::SdioError};

use crate::chip_id::*;

// ============================================================  
// LMAC 消息 ID 定义  
// ============================================================  
const TASK_DBG: u16 = 1;  
const DRV_TASK_ID: u16 = 100;  
const LMAC_FIRST_DBG: u16 = 0x0400; // TASK_DBG(1) << 10   

/// Debug 消息 ID  
#[repr(u16)]  
#[derive(Debug, Clone, Copy)]  
pub enum DbgMsgId {  
    MemReadReq,          // 0x0400  
    MemReadCfm,          // 0x0401  
    MemWriteReq,         // 0x0402  
    MemWriteCfm,         // 0x0403  
    SetModFilterReq,     // 0x0404  
    SetModFilterCfm,     // 0x0405  
    SetSevFilterReq,     // 0x0406  
    SetSevFilterCfm,     // 0x0407  
    ErrorInd,            // 0x0408    
    GetSysStatReq,       // 0x0409  
    GetSysStatCfm,       // 0x040a  
    MemBlockWriteReq,    // 0x040b  
    MemBlockWriteCfm,    // 0x040c  
    StartAppReq,         // 0x040d  
    StartAppCfm,         // 0x040e  
    StartNpcReq,         // 0x040f  
    StartNpcCfm,         // 0x0410  
    MemMaskWriteReq,     // 0x0411  
    MemMaskWriteCfm,     // 0x0412  
} 
  
impl DbgMsgId {  
    pub fn msg_id(self) -> u16 {  
        LMAC_FIRST_DBG + match self {  
            Self::MemReadReq       => 0,  
            Self::MemReadCfm       => 1,  
            Self::MemWriteReq      => 2,  
            Self::MemWriteCfm      => 3,  
            Self::SetModFilterReq  => 4,  
            Self::SetModFilterCfm  => 5,  
            Self::SetSevFilterReq  => 6,  
            Self::SetSevFilterCfm  => 7,  
            Self::ErrorInd         => 8,    
            Self::GetSysStatReq    => 9,  
            Self::GetSysStatCfm    => 10,  
            Self::MemBlockWriteReq => 11,   
            Self::MemBlockWriteCfm => 12,  
            Self::StartAppReq      => 13,    
            Self::StartAppCfm      => 14,  
            Self::StartNpcReq      => 15,  
            Self::StartNpcCfm      => 16,  
            Self::MemMaskWriteReq  => 17,  
            Self::MemMaskWriteCfm  => 18,  
        }  
    }  
}

// ============================================================  
// MM (MAC Management) 消息 — 用于与运行中固件通信  
// ============================================================  
// TASK_MM = 0, LMAC_FIRST_MSG(TASK_MM) = 0x0000  
  
/// MM_RESET_REQ: 复位固件 MAC 层  
const MM_RESET_REQ: u16       = 0x0000;  
/// MM_VERSION_REQ: 获取 LMAC/PHY 版本信息  
const MM_VERSION_REQ: u16     = 0x0004;  
/// MM_GET_MAC_ADDR_REQ: 获取 WiFi MAC 地址  
const MM_GET_MAC_ADDR_REQ: u16 = 0x0073;  
/// MM_SET_STACK_START_REQ: 启动 WiFi 协议栈  
const MM_SET_STACK_START_REQ: u16 = 0x007B;  
/// MM_GET_FW_VERSION_REQ: 获取固件版本字符串  
const MM_GET_FW_VERSION_REQ: u16 = 0x0080;  
  
/// LMAC 版本信息 (MM_VERSION_CFM 响应)  
#[derive(Debug)]  
pub struct MmVersionCfm {  
    pub version_lmac: u32,  
    pub version_machw_1: u32,  
    pub version_machw_2: u32,  
    pub version_phy_1: u32,  
    pub version_phy_2: u32,  
    pub features: u32,  
    pub max_sta_nb: u16,  
    pub max_vif_nb: u8,  
}  

/// WiFi 协议栈启动确认 (MM_SET_STACK_START_CFM 响应)  
#[derive(Debug)]  
pub struct MmStackStartCfm {  
    pub is_5g_support: u8,  
    pub vendor_info: u8,  
}  
  
/// 固件版本字符串 (MM_GET_FW_VERSION_CFM 响应)  
#[derive(Debug)]  
pub struct MmFwVersionCfm {  
    pub fw_version_len: u8,  
    pub fw_version: [u8; 63],  
} 

// ============================================================  
// SDIO 传输类型  
// ============================================================  
const SDIO_TYPE_CFG_CMD_RSP: u8 = 0x11;  
  
// ============================================================  
// 消息缓冲区  
// ============================================================  
/// 最大消息大小: 8 (transport header) + 8 (lmac_msg header) + 1032 (block write payload)  
const MSG_BUF_MAX: usize = 1536;  

/// CRC-8 with polynomial 0x107 (X^8 + X^2 + X + 1)  
/// Used by AIC8800D80/D80X2 for SDIO transport header CRC  
fn crc8_ponl_107(data: &[u8]) -> u8 {  
    let mut crc: u8 = 0;  
    for &byte in data {  
        let mut mask: u8 = 0x80;  
        while mask > 0 {  
            if crc & 0x80 != 0 {  
                crc = crc.wrapping_shl(1) ^ 0x07;  
            } else {  
                crc = crc.wrapping_shl(1);  
            }  
            if byte & mask != 0 {  
                crc ^= 0x07;  
            }  
            mask >>= 1;  
        }  
    }  
    crc  
}

/// IPC 消息传输层  
///  
/// 封装了消息的构建、发送和响应接收。  
pub struct IpcTransport<'a, H: SdioHost> {  
    sdio_host: &'a mut H,  
    chip: ChipVariant,
    tx_buf: [u8; MSG_BUF_MAX],  
    rx_buf: [u8; MSG_BUF_MAX],
}

impl<'a, H: SdioHost> IpcTransport<'a, H> {  
    pub fn new(sdio_host: &'a mut H, chip: ChipVariant) -> Self {  
        Self {  
            sdio_host,  
            chip,  
            tx_buf: [0; MSG_BUF_MAX],  
            rx_buf: [0; MSG_BUF_MAX],  
        }  
    }  

    /// 构建 lmac_msg 头部 + transport header, 写入 tx_buf  
    /// 返回总长度 (含 transport header)  
    fn build_msg(&mut self, msg_id: u16, payload: &[u8]) -> usize {  
        let lmac_msg_len = 8 + payload.len(); //lmac_msg header + payload  
        let total_payload_len = 4 + lmac_msg_len; // transport header + lmac_msg
        
        // Transport header (4 bytes)  
        self.tx_buf[0] = (total_payload_len & 0xFF) as u8; // 消息长度低字节
        self.tx_buf[1] = ((total_payload_len >> 8) & 0x0F) as u8; // 消息长度高字节
        self.tx_buf[2] = SDIO_TYPE_CFG_CMD_RSP; // 消息类型
        // byte[3]: AIC8800D80/D80X2 需要 CRC8, 其他为 0x00 
        match self.chip {
            ChipVariant::Aic8800D80 | ChipVariant::Aic8800D80X2 => {
                self.tx_buf[3] = crc8_ponl_107(&self.tx_buf[0..3]);
            }
            _ => {
                self.tx_buf[3] = 0x00;
            }
        }

        // Dummy word (4 bytes)  
        self.tx_buf[4..8].fill(0);   

        // lmac_msg header (8 bytes), little-endian  
        let idx = 8;
        self.tx_buf[idx..idx + 2].copy_from_slice(&msg_id.to_le_bytes()); // 消息 ID
        
        // dest_id 从 msg_id 动态推导
        let dest_id = (msg_id >> 10) as u16; 
        self.tx_buf[idx + 2..idx + 4].copy_from_slice(&dest_id.to_le_bytes());
        // self.tx_buf[idx + 2..idx + 4].copy_from_slice(&TASK_DBG.to_le_bytes()); // dest_id
        self.tx_buf[idx + 4..idx + 6].copy_from_slice(&DRV_TASK_ID.to_le_bytes()); // src_id
        let payload_len = payload.len() as u16;
        self.tx_buf[idx + 6..idx + 8].copy_from_slice(&payload_len.to_le_bytes()); // 消息负载长度

        // payload
        let payload_start = idx + 8;
        self.tx_buf[payload_start..payload_start + payload.len()].copy_from_slice(payload);
        
        // raw_len = transport header (4) + dummy (4) + lmac_msg header (8) + param  
        let raw_len = 8 + lmac_msg_len;

        // Step 1: 4 字节对齐 (TX_ALIGNMENT)  
        let aligned4 = (raw_len + 3) & !3; // 向上对齐到 4 字节边界
        for i in raw_len..aligned4 {  
            self.tx_buf[i] = 0; // 填充对齐字节为 0  
        }

        // Step 2: 块对齐 — 仅当不是 512 倍数时加 TAIL_LEN(4)  
        let send_len = if aligned4 % 512 != 0 {
            let with_tail = aligned4 + 4;
            let block_aligned = (with_tail / 512 + 1) * 512; // 向上对齐到下一个 512 字节边界
            for i in aligned4..block_aligned.min(MSG_BUF_MAX) {
                self.tx_buf[i] = 0; // 填充对齐字节为 0
            }
            block_aligned
        }
        else {
            aligned4 // 已是 512 的整数倍, 不加 tail
        };
        send_len
    }
  
    /// 发送 IPC 消息并等待响应  
    pub fn send_msg(&mut self, msg_id: DbgMsgId, payload: &[u8], wait_cfm: bool, cfm_buf: &mut [u8]) -> Result<usize, SdioError> {  
        let id = msg_id.msg_id();
        let send_len = self.build_msg(id, payload);

        // ---- 流控 (AIC8801/D80 必须) ---- 
        let mut fc_retry = 0u32;
        loop {
            let fc_reg = self.sdio_host.read_byte(1, SDIOWIFI_FLOW_CTRL_REG)?;
            if fc_reg & 0x7F != 0 {
                break; // 流控允许发送
            }
            fc_retry += 1;
            if fc_retry > 50 {
                log::error!("IPC: flow control timeout"); 
                return Err(SdioError::Timeout);
            } 
            for _ in 0..2000 {  
                core::hint::spin_loop();  
            } 
        }
        // 写入 WR_FIFO 
        self.sdio_host.write_fifo(
            1, // function 1  
            SDIOWIFI_WR_FIFO_ADDR,
            &self.tx_buf[..send_len],
        )?;

        if !wait_cfm {
            return Ok(0); // 不等待响应, 直接返回
        }

        // 轮询等待响应  
        // 读取 BLOCK_CNT_REG 等待非零值 (表示有数据可读)  
        let mut retry = 0u32;
        let mut read_err_cnt = 0u32; //read_fifo 错误重试计数器
        loop {
            let raw_cnt = self.sdio_host.read_byte(1, SDIOWIFI_BLOCK_CNT_REG)?;
            // mask 掉 SDIO_OTHER_INTERRUPT (bit7)  
            if raw_cnt & 0x80 != 0 {
                log::warn!("IPC: SDIO_OTHER_INTERRUPT set, raw_cnt=0x{:02x}", raw_cnt);
                retry += 1;
            } else if raw_cnt > 0 {
                let block_cnt = raw_cnt as usize;
                let read_len = (block_cnt * 512).min(MSG_BUF_MAX);  
                match self.sdio_host.read_fifo(
                    1, 
                    SDIOWIFI_RD_FIFO_ADDR,
                    &mut self.rx_buf[..read_len],
                ) {
                    Ok(()) => {
                        // 验证响应 msg_id (CFM = REQ + 1)  
                        let resp_id = u16::from_le_bytes([
                            self.rx_buf[4], 
                            self.rx_buf[5]
                        ]); // lmac_msg header 中的 msg_id 位于 offset 8-9
                        if resp_id != id + 1 {
                            log::error!("IPC: unexpected response id=0x{:04x}, expected=0x{:04x}", resp_id, id + 1);
                            return Err(SdioError::CrcError); 
                        }
                        // 解析响应: 跳过 transport header (4) + dummy (4) + lmac_msg header (8)
                        let payload_offset = 16;
                        let cfm_len = cfm_buf.len().min(read_len.saturating_sub(payload_offset));
                        if cfm_len > 0 {
                            cfm_buf[..cfm_len].copy_from_slice(&self.rx_buf[payload_offset..payload_offset + cfm_len]);                                
                        }
                        return Ok(cfm_len); // 返回响应长度
                    }
                    Err(e) => {
                        // CRC Error / IoError 时不立即退出，重试  
                        read_err_cnt += 1;
                        log::warn!(  
                            "IPC: read_fifo error ({}/5) for msg_id=0x{:04x}: {:?}",  
                            read_err_cnt, id, e  
                        ); 
                        if read_err_cnt > 5 {
                            log::error!("IPC: too many read errors for msg_id=0x{:04x}", id);  
                            return Err(e); 
                        }
                        // 等待芯片 SDIO 接口稳定 (DAT 线已在 sdhci 层复位)  
                        for _ in 0..500_000 {  
                            core::hint::spin_loop();  
                        }  
                        continue; // 回到轮询 block_cnt_reg 
                    }
                }        
            } else {
                retry += 1;
            }        
            
            if retry > 5_000 {
                log::error!("IPC: response timeout for msg_id=0x{:04x}", id); 
                return Err(SdioError::Timeout); // 超时错误
            }
            // 简单忙等 (no_std 环境下没有 sleep)  
            for _ in 0..1000 {  
                core::hint::spin_loop();  
            }      
        }        
    }

    /// 通用 IPC 消息发送 — 直接使用 raw msg_id (不经过 DbgMsgId 枚举)  
///  
/// 用于固件启动后发送 TASK_MM 消息。  
/// build_msg 内部根据 msg_id >> 10 自动设置 dest_id。  
pub fn send_msg_raw(  
    &mut self,  
    msg_id: u16,  
    payload: &[u8],  
    wait_cfm: bool,  
    cfm_buf: &mut [u8],  
) -> Result<usize, SdioError> {  
    let send_len = self.build_msg(msg_id, payload);  
  
    // 流控  
    let mut fc_retry = 0u32;  
    loop {  
        let fc_reg = self.sdio_host.read_byte(1, SDIOWIFI_FLOW_CTRL_REG)?;  
        if fc_reg & 0x7F != 0 {  
            break;  
        }  
        fc_retry += 1;  
        if fc_retry > 50 {  
            log::error!("IPC: flow control timeout (raw msg_id=0x{:04x})", msg_id);  
            return Err(SdioError::Timeout);  
        }  
        for _ in 0..2000 {  
            core::hint::spin_loop();  
        }  
    }  
  
    self.sdio_host.write_fifo(1, SDIOWIFI_WR_FIFO_ADDR, &self.tx_buf[..send_len])?;  
  
    if !wait_cfm {  
        return Ok(0);  
    }  
  
    let cfm_id = msg_id + 1;  
    let mut retry = 0u32;  
    let mut read_err_cnt = 0u32;  
    loop {  
        let raw_cnt = self.sdio_host.read_byte(1, SDIOWIFI_BLOCK_CNT_REG)?;  
        if raw_cnt & 0x80 != 0 {  
            retry += 1;  
        } else if raw_cnt > 0 {  
            let block_cnt = raw_cnt as usize;  
            let read_len = (block_cnt * 512).min(MSG_BUF_MAX);  
            match self.sdio_host.read_fifo(  
                1,  
                SDIOWIFI_RD_FIFO_ADDR,  
                &mut self.rx_buf[..read_len],  
            ) {  
                Ok(()) => {  
                    let resp_id =  
                        u16::from_le_bytes([self.rx_buf[4], self.rx_buf[5]]);  
                    if resp_id != cfm_id {  
                        log::error!(  
                            "IPC: unexpected resp_id=0x{:04x}, expected=0x{:04x}",  
                            resp_id,  
                            cfm_id  
                        );  
                        return Err(SdioError::CrcError);  
                    }  
                    let payload_offset = 16;  
                    let cfm_len =  
                        cfm_buf.len().min(read_len.saturating_sub(payload_offset));  
                    if cfm_len > 0 {  
                        cfm_buf[..cfm_len].copy_from_slice(  
                            &self.rx_buf[payload_offset..payload_offset + cfm_len],  
                        );  
                    }  
                    return Ok(cfm_len);  
                }  
                Err(e) => {  
                    read_err_cnt += 1;  
                    log::warn!(  
                        "IPC: read_fifo error ({}/5) for raw msg_id=0x{:04x}: {:?}",  
                        read_err_cnt,  
                        msg_id,  
                        e  
                    );  
                    if read_err_cnt > 5 {  
                        return Err(e);  
                    }  
                    for _ in 0..500_000 {  
                        core::hint::spin_loop();  
                    }  
                    continue;  
                }  
            }  
        } else {  
            retry += 1;  
        }  
  
        if retry > 5_000 {  
            log::error!("IPC: response timeout for raw msg_id=0x{:04x}", msg_id);  
            return Err(SdioError::Timeout);  
        }  
        for _ in 0..1000 {  
            core::hint::spin_loop();  
        }  
    }  
}
}

// ============================================================  
// 高层消息接口  
// ============================================================  
  
/// 读取芯片内存 (4 字节)  
pub fn ipc_mem_read<H: SdioHost>(transport: &mut IpcTransport<H>, addr: u32) -> Result<u32, SdioError> {  
    let payload = addr.to_le_bytes(); // 4 字节地址作为消息负载
    let mut cfm = [0u8; 8];
    transport.send_msg(DbgMsgId::MemReadReq, &payload, true, &mut cfm)?;
    let data = u32::from_le_bytes([cfm[4], cfm[5], cfm[6], cfm[7]]);  // cfm[0..4]=memaddr, cfm[4..8]=memdata 
    Ok(data)
}

/// 写入芯片内存 (4 字节)  
pub fn ipc_mem_write<H: SdioHost>(transport: &mut IpcTransport<H>, addr: u32, data: u32) -> Result<(), SdioError> {  
    // param: memaddr (4) + memdata (4)  
    let mut payload = [0u8; 8];
    payload[..4].copy_from_slice(&addr.to_le_bytes()); // 前 4 字节为地址
    payload[4..].copy_from_slice(&data.to_le_bytes()); // 后 4 字节为数据
    let mut cfm = [0u8; 8];
    transport.send_msg(DbgMsgId::MemWriteReq, &payload, true, &mut cfm)?;
    Ok(())
}

/// 块写入芯片内存 (最大 1032 字节)
pub fn ipc_mem_block_write<H: SdioHost>(
    transport: &mut IpcTransport<H>,
     addr: u32, data: &[u8]
    ) -> Result<(), SdioError> {  
    assert!(data.len() <= 1024, "block write max 1024 bytes");
    // param: memaddr (4) + memsize (4) + memdata[256] (up to 1024 bytes)  
    let payload_len = 4 + 4 + data.len(); // 地址 + 大小 + 数据
    // 构建 param 到栈上缓冲区  
    let mut payload = [0u8; 1032]; // 4 + 4 + 1024  
    payload[..4].copy_from_slice(&addr.to_le_bytes()); // 前 4 字节为地址
    payload[4..8].copy_from_slice(&(data.len() as u32).to_le_bytes()); // 后 4 字节为大小
    payload[8..8 + data.len()].copy_from_slice(data); // 后续字节为数据
    let mut cfm = [0u8; 4];
    transport.send_msg(
        DbgMsgId::MemBlockWriteReq,
         &payload[..payload_len], 
         true, 
         &mut cfm)?;
    Ok(())
}

/// 掩码写入芯片内存 (DBG_MEM_MASK_WRITE_REQ = 0x0411)
pub fn ipc_mem_mask_write<H: SdioHost> (
    transport: &mut IpcTransport<H>,
    addr: u32,
    mask: u32,
    data: u32,
) -> Result<(), SdioError> {
    // param: memaddr(4) + memmask(4) + memdata(4) = 12 bytes
    let mut payload = [0u8; 12];
    payload[0..4].copy_from_slice(&addr.to_le_bytes());
    payload[4..8].copy_from_slice(&mask.to_le_bytes());
    payload[8..12].copy_from_slice(&data.to_le_bytes());
    // cfm: memaddr(4) + memdata(4) = 8 bytes
    let mut cfm = [0u8; 8];
    transport.send_msg(DbgMsgId::MemMaskWriteReq, &payload, true, &mut cfm)?;
    Ok(())
}

/// 启动固件  
pub fn ipc_start_app<H: SdioHost>(transport: &mut IpcTransport<H>, boot_addr: u32, boot_type: u32) -> Result<u32, SdioError> {  
    // param: bootaddr (4) + boottype (4) 
    let mut payload = [0u8; 8];
    payload[..4].copy_from_slice(&boot_addr.to_le_bytes()); // 前 4 字节为启动地址
    payload[4..].copy_from_slice(&boot_type.to_le_bytes()); // 后 4 字节为启动类型
    let mut cfm = [0u8; 4];
    transport.send_msg(DbgMsgId::StartAppReq, &payload, true, &mut cfm)?;
    let boot_status = u32::from_le_bytes([cfm[0], cfm[1], cfm[2], cfm[3]]);
    Ok(boot_status)
}

/// 发送 MM_VERSION_REQ, 获取 LMAC/PHY 版本信息  
///  
/// 参考: rwnx_msg_tx.c rwnx_send_version_req() (line 494-506)  
pub fn mm_get_version<H: SdioHost>(  
    transport: &mut IpcTransport<H>,  
) -> Result<MmVersionCfm, SdioError> {  
    let mut cfm_buf = [0u8; 27]; // sizeof(mm_version_cfm) = 6*4 + 2 + 1 = 27  
    transport.send_msg_raw(MM_VERSION_REQ, &[], true, &mut cfm_buf)?;  
    Ok(MmVersionCfm {  
        version_lmac:   u32::from_le_bytes([cfm_buf[0],  cfm_buf[1],  cfm_buf[2],  cfm_buf[3]]),  
        version_machw_1: u32::from_le_bytes([cfm_buf[4],  cfm_buf[5],  cfm_buf[6],  cfm_buf[7]]),  
        version_machw_2: u32::from_le_bytes([cfm_buf[8],  cfm_buf[9],  cfm_buf[10], cfm_buf[11]]),  
        version_phy_1:   u32::from_le_bytes([cfm_buf[12], cfm_buf[13], cfm_buf[14], cfm_buf[15]]),  
        version_phy_2:   u32::from_le_bytes([cfm_buf[16], cfm_buf[17], cfm_buf[18], cfm_buf[19]]),  
        features:        u32::from_le_bytes([cfm_buf[20], cfm_buf[21], cfm_buf[22], cfm_buf[23]]),  
        max_sta_nb:      u16::from_le_bytes([cfm_buf[24], cfm_buf[25]]),  
        max_vif_nb:      cfm_buf[26],  
    })  
}  
  
/// 发送 MM_SET_STACK_START_REQ, 启动 WiFi 协议栈并获取 5G/vendor 信息  
///  
/// 参考: rwnx_msg_tx.c rwnx_send_set_stack_start_req() (line 1393-1414)  
pub fn mm_set_stack_start<H: SdioHost>(  
    transport: &mut IpcTransport<H>,  
) -> Result<MmStackStartCfm, SdioError> {  
    // payload: is_stack_start(1) + efuse_valid(1) + set_vendor_info(1) + fwtrace_redir(1) = 4 bytes  
    let payload: [u8; 4] = [  
        1, // is_stack_start = 1 (start)  
        0, // efuse_valid = 0  
        0, // set_vendor_info = 0 (no 5G for AIC8801)  
        0, // fwtrace_redir = 0  
    ];  
    let mut cfm_buf = [0u8; 2];  
    transport.send_msg_raw(MM_SET_STACK_START_REQ, &payload, true, &mut cfm_buf)?;  
    Ok(MmStackStartCfm {  
        is_5g_support: cfm_buf[0],  
        vendor_info:   cfm_buf[1],  
    })  
}  
  
/// 发送 MM_GET_FW_VERSION_REQ, 获取固件版本字符串  
///  
/// 参考: rwnx_msg_tx.c rwnx_send_get_fw_version_req() (line 1797-1813)  
pub fn mm_get_fw_version<H: SdioHost>(  
    transport: &mut IpcTransport<H>,  
) -> Result<MmFwVersionCfm, SdioError> {  
    // payload: 1 byte (dummy)  
    let payload: [u8; 1] = [0];  
    let mut cfm_buf = [0u8; 64]; // fw_version_len(1) + fw_version(63) = 64  
    transport.send_msg_raw(MM_GET_FW_VERSION_REQ, &payload, true, &mut cfm_buf)?;  
    let mut fw_version = [0u8; 63];  
    fw_version.copy_from_slice(&cfm_buf[1..64]);  
    Ok(MmFwVersionCfm {  
        fw_version_len: cfm_buf[0],  
        fw_version,  
    })  
}  
  
/// 发送 MM_GET_MAC_ADDR_REQ, 获取 WiFi MAC 地址  
///  
/// 参考: rwnx_msg_tx.c rwnx_send_get_macaddr_req() (line 1334-1355)  
pub fn mm_get_mac_addr<H: SdioHost>(  
    transport: &mut IpcTransport<H>,  
) -> Result<[u8; 6], SdioError> {  
    // payload: get(1) = 1  
    let payload: [u8; 4] = [1, 0, 0, 0]; // get = 1 (u32 aligned)  
    let mut cfm_buf = [0u8; 8]; // MAC addr (6) + padding  
    transport.send_msg_raw(MM_GET_MAC_ADDR_REQ, &payload, true, &mut cfm_buf)?;  
    let mut mac = [0u8; 6];  
    mac.copy_from_slice(&cfm_buf[0..6]);  
    Ok(mac)  
}  
  
/// 发送 MM_RESET_REQ, 复位 MAC 层  
///  
/// 参考: rwnx_msg_tx.c rwnx_send_reset() (line 459-471)  
pub fn mm_reset<H: SdioHost>(  
    transport: &mut IpcTransport<H>,  
) -> Result<(), SdioError> {  
    let mut cfm_buf = [0u8; 0];  
    transport.send_msg_raw(MM_RESET_REQ, &[], true, &mut cfm_buf)?;  
    Ok(())  
}