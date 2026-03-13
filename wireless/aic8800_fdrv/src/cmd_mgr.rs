use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::future::poll_fn;
use core::sync::atomic::Ordering;
use core::task::Poll;

use axtask::future::block_on;
use log;

use crate::bus::{BusState, WifiBus};
use sdhci_cv1800::regs::*; 

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
  
/// CMD 超时（与 Linux RWNX_80211_CMD_TIMEOUT_MS 一致）  
const CMD_TIMEOUT_MS: u64 = 6000;  
  
// Frame construction constants  
pub const DUMMY_WORD_LEN: usize = 4;  
pub const TAIL_LEN: usize = 4;  
pub const CMD_TX_TIMEOUT_DEFAULT_MS: u64 = 5000;  

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

// ============================================================  
// LMAC 消息 ID（TASK_MM = 0, LMAC_FIRST_MSG(0) = 0）  
// ============================================================  
pub const MM_SET_STACK_START_REQ: u16 = 0x007B; // 枚举偏移 123  
pub const MM_SET_STACK_START_CFM: u16 = 0x007C;  

// MM 消息 (TASK_MM = 0)  
pub const MM_RESET_REQ: u16           = lmac_first_msg(TASK_MM);      // 0x0000  
pub const MM_RESET_CFM: u16           = lmac_first_msg(TASK_MM) + 1;  // 0x0001  
pub const MM_START_REQ: u16           = lmac_first_msg(TASK_MM) + 2;  // 0x0002  
pub const MM_START_CFM: u16           = lmac_first_msg(TASK_MM) + 3;  // 0x0003  
pub const MM_VERSION_REQ: u16         = lmac_first_msg(TASK_MM) + 4;  // 0x0004  
pub const MM_VERSION_CFM: u16         = lmac_first_msg(TASK_MM) + 5;  // 0x0005

#[derive(Debug)]
pub enum CmdError {
    Timeout,
    BusDown, 
    SdioError, 
    InvalidResponse,
    MismatchedCfm { expected: u16, got: u16 },
}

fn align_up(val: usize, align: usize) -> usize {
    (val + align - 1) & !(align - 1)
}

/// 构造 SDIO CMD 帧  
/// 格式：[4B sdio_header][4B dummy][8B lmac_msg][NB param]  
/// 对齐：TX_ALIGNMENT → TAIL_LEN → SDIOWIFI_FUNC_BLOCKSIZE  
fn build_cmd_frame(msg_id: u16, dest_task: u16, param: &[u8]) -> Vec<u8> {
    // lmac_msg header (8B) + param  
    let lmac_len = 8 + param.len();
    // sdio payload = dummy(4) + lmac_msg(8) + param  
    let sdio_payload_len = DUMMY_WORD_LEN + lmac_len;
    // sdio_header length field = sdio_payload_len + 4 (header itself) 
    let sdio_len = sdio_payload_len + 4;
    // total raw length = sdio_header(4) + dummy(4) + lmac_msg(8) + param  
    let raw_len = 4 + DUMMY_WORD_LEN + lmac_len;  

    // Step 1: TX_ALIGNMENT  
    let aligned_len = align_up(raw_len, TX_ALIGNMENT); 

    // Step 2: TAIL_LEN + BLOCK_SIZE alignment  
    let final_len = if aligned_len % SDIOWIFI_FUNC_BLOCKSIZE != 0 {
        align_up(aligned_len + TAIL_LEN, SDIOWIFI_FUNC_BLOCKSIZE)
    } else {
        aligned_len
    };

    let mut buf = vec![0u8; final_len];

    // sdio_header [0..4]  
    buf[0] = (sdio_len & 0xFF) as u8;  
    buf[1] = ((sdio_len >> 8) & 0x0F) as u8;  
    buf[2] = SDIO_TYPE_CFG_CMD_RSP; // 0x11  
    buf[3] = 0x00; // AIC8801: reserved; AIC8800D80: CRC8  
    // dummy word [4..8] = 0x00000000 (already zeroed)  

    // lmac_msg header [8..16]  
    let msg_offset = 4 + DUMMY_WORD_LEN; // = 8  
    buf[msg_offset..msg_offset + 2].copy_from_slice(&msg_id.to_le_bytes());  
    buf[msg_offset + 2..msg_offset + 4].copy_from_slice(&dest_task.to_le_bytes());  
    buf[msg_offset + 4..msg_offset + 6].copy_from_slice(&DRV_TASK_ID.to_le_bytes()); // src_id  
    buf[msg_offset + 6..msg_offset + 8].copy_from_slice(&(param.len() as u16).to_le_bytes()); 
    
    // param [16..16+param.len()]  
    if !param.is_empty() {
        buf[msg_offset + 8..msg_offset + 8 + param.len()].copy_from_slice(param);  
    }  
  
    buf  
}

/// 发送 LMAC 命令并等待 CFM
///
/// # 参数
/// - `bus`: 共享总线
/// - `msg_id`: 消息 ID (如 MM_RESET_REQ)
/// - `dest_id`: 目标任务 ID (如 TASK_MM)
/// - `param`: 命令参数（结构体的字节表示）
/// - `timeout_ms`: 超时时间
///
/// # 返回
/// - `Ok(Vec<u8>)`: CFM 的 param 部分
/// - `Err(CmdError)`: 超时或错误
pub fn send_cmd(
    bus: &Arc<WifiBus>,
    msg_id: u16,
    dest_id: u16,
    param: &[u8],
    timeout_ms: u64
) -> Result<Vec<u8>, CmdError> {
    // 检查总线状态  
    if *bus.state.lock() == BusState::Down {
        return Err(CmdError::BusDown);
    }

    let timeout = if timeout_ms == 0 {
        CMD_TX_TIMEOUT_DEFAULT_MS
    } else {
        timeout_ms
    };

    // ---- 构造 SDIO 帧 ----
    let frame = build_cmd_frame(msg_id, dest_id, param);

    // 期望的 CFM ID = REQ ID + 1（LMAC 约定） 
    let expected_cfm_id = msg_id + 1;

    log::info!(  
        "[cmd_mgr] TX msg_id=0x{:04x}, dest=0x{:04x}, param_len={}, frame_len={}, timeout={}ms",  
        msg_id, dest_id, param.len(), frame.len(), timeout  
    ); 

    // 清空残留的 CMD 响应队列（避免旧响应干扰）  
    {
        let mut queue = bus.cmd_rsp_queue.lock();
        if !queue.is_empty() {
            log::warn!(  
                "[cmd_mgr] discarding {} stale CMD responses",  
                queue.len()  
            );  
            queue.clear(); 
        }
    }

    // 清除错误标志  
    bus.cmd_rsp_error.store(false, Ordering::Release);  

    // ---- 通过 TX 线程发送（CMD 优先级）----
    {
        let mut cmd_slot = bus.cmd_pending.lock();
        *cmd_slot = Some(frame);
        bus.cmd_pending_flag.store(true, Ordering::Release);
    }
    bus.tx_wake_pollset.wake(); // 唤醒 TX 线程

    // ---- 等待 CFM（RX 线程放入 cmd_rsp_queue）----
    let start = axhal::time::monotonic_time_nanos();
    let timeout_ns = timeout * 1_000_000;

    let result = block_on(poll_fn(|cx| {
        // 检查总线关闭 
        if bus.cmd_rsp_error.load(Ordering::Acquire) || *bus.state.lock() == BusState::Down {
            return Poll::Ready(Err(CmdError::BusDown)); 
        }

        // 检查超时
        let elapsed = axhal::time::monotonic_time_nanos() - start;
        if elapsed > timeout_ns {
            log::error!(  
                "[cmd_mgr] TIMEOUT waiting for cfm 0x{:04x} (elapsed={}ms)",  
                expected_cfm_id, elapsed / 1_000_000  
            );  
            return Poll::Ready(Err(CmdError::Timeout));
        }

        // 尝试从队列取响应  
        {
            let mut queue = bus.cmd_rsp_queue.lock();
            if let Some(rsp) = queue.pop_front() {
                // 验证 CFM ID 
                if rsp.len() >= LmacMsg::SIZE {
                    let msg = LmacMsg::from_le_bytes(&rsp);
                    if msg.id == expected_cfm_id {
                        log::info!(  
                            "[cmd_mgr] RX cfm_id=0x{:04x}, param_len={}, pattern=0x{:08x}",  
                            msg.id, msg.param_len, msg.pattern  
                        );
                        let param_start = LmacMsg::SIZE;
                        let param_end = param_start + msg.param_len as usize;
                        if rsp.len() >= param_end {
                            return Poll::Ready(Ok(rsp[param_start..param_end].to_vec()));
                        } else {
                            return Poll::Ready(Ok(rsp[param_start..].to_vec()));  
                        }
                    } else {
                        log::warn!(  
                            "[cmd_mgr] CFM id mismatch: expected 0x{:04x}, got 0x{:04x}",  
                            expected_cfm_id,  
                            msg.id  
                        );  
                    }
                } else {
                    // 响应格式无效，丢弃  
                    log::warn!("[cmd_mgr] invalid CFM response, len={}", rsp.len());  
                }                
            }
        }        

        // 注册 waker，等待 RX 线程唤醒
        bus.cmd_rsp_pollset.register(cx.waker());

        // 双重检查
        {
            let mut queue = bus.cmd_rsp_queue.lock();
            if let Some(rsp) = queue.pop_front() {
                if rsp.len() >= LmacMsg::SIZE {
                    let msg = LmacMsg::from_le_bytes(&rsp);
                    if msg.id == expected_cfm_id {
                        log::info!(  
                            "[cmd_mgr] RX cfm_id=0x{:04x} (double-check), param_len={}",  
                            msg.id, msg.param_len  
                        ); 
                        let param_start = LmacMsg::SIZE;  
                        let param_end = param_start + msg.param_len as usize; 
                        if rsp.len() >= param_end {  
                            return Poll::Ready(Ok(rsp[param_start..param_end].to_vec()));  
                        } else {  
                            return Poll::Ready(Ok(rsp[param_start..].to_vec()));  
                        } 
                    }
                }                
            }
        }

        Poll::Pending
    }));

    match &result {  
        Ok(rsp) => log::info!("[cmd_mgr] send_cmd 0x{:04x} OK, rsp_len={}", msg_id, rsp.len()),  
        Err(e) => log::error!("[cmd_mgr] send_cmd 0x{:04x} FAILED: {:?}", msg_id, e),  
    }  

    result
}

/// 发送命令不等待 CFM（用于 IND 类通知或不需要回复的消息）
pub fn send_cmd_no_cfm(
    bus: &Arc<WifiBus>,
    msg_id: u16,
    dest_id: u16,
    param: &[u8],
) -> Result<(), CmdError> {
    if *bus.state.lock() == BusState::Down {  
        return Err(CmdError::BusDown);  
    }  

    log::info!("[cmd_mgr] TX (no_cfm) msg_id=0x{:04x}, dest=0x{:04x}", msg_id, dest_id);  

    let frame = build_cmd_frame(msg_id, dest_id, param);
    {
        let mut cmd_slot = bus.cmd_pending.lock();
        *cmd_slot = Some(frame);
        bus.cmd_pending_flag.store(true, Ordering::Release);
    }
    bus.tx_wake_pollset.wake();

    Ok(())
}
