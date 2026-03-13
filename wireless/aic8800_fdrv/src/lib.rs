#![no_std]

pub mod bus;
pub mod cmd_mgr;
pub mod rx_thread;
pub mod tx_thread;

extern crate alloc;
  
use alloc::sync::Arc;  
use alloc::vec;
use core::sync::atomic::Ordering;
use bus::{WifiBus, BusState, set_global_bus, sdio1_irq_handler};  
use sdhci_cv1800::{CviSdhci, regs::*};  
use aic8800_sdio::SdioHost;

use crate::cmd_mgr::{DRV_TASK_ID, DUMMY_WORD_LEN, MM_RESET_REQ, MM_SET_STACK_START_REQ, TAIL_LEN, TASK_MM};  

/// 轮询模式发送 LMAC 命令并等待 CFM  
///  
/// 用于 FDRV 初始化阶段（中断未使能），直接操作 SDIO 寄存器。  
/// 与 `aic8800_fw::ipc_msg::IpcTransport::send_msg` 类似，但使用  
/// TASK_MM 消息格式而非 TASK_DBG。  
fn polling_send_cmd(
    sdio: &CviSdhci,
    msg_id: u16,
    dest_id: u16,
    param: &[u8],
    wait_cfm: bool,
    cfm_buf: &mut [u8],
) -> Result<usize, &'static str> {
    // ---- 构造帧 ----  
    let lmac_len = 8 + param.len();  
    let sdio_payload_len = DUMMY_WORD_LEN + lmac_len;  
    let sdio_len = sdio_payload_len + 4;  
    let raw_len = 4 + DUMMY_WORD_LEN + lmac_len; 

    let aligned = (raw_len + 3) & !3;  
    let final_len = if aligned % SDIOWIFI_FUNC_BLOCKSIZE != 0 {  
        let with_tail = aligned + TAIL_LEN;  
        ((with_tail / SDIOWIFI_FUNC_BLOCKSIZE) + 1) * SDIOWIFI_FUNC_BLOCKSIZE  
    } else {  
        aligned  
    };

    let mut buf = vec![0u8; final_len];  
  
    // sdio_header [0..4]  
    buf[0] = (sdio_len & 0xFF) as u8;  
    buf[1] = ((sdio_len >> 8) & 0x0F) as u8;  
    buf[2] = SDIO_TYPE_CFG_CMD_RSP;  
    buf[3] = 0x00;  
  
    // lmac_msg header [8..16]  
    let off = 4 + DUMMY_WORD_LEN; // = 8  
    buf[off..off + 2].copy_from_slice(&msg_id.to_le_bytes());  
    buf[off + 2..off + 4].copy_from_slice(&dest_id.to_le_bytes());  
    buf[off + 4..off + 6].copy_from_slice(&DRV_TASK_ID.to_le_bytes());  
    buf[off + 6..off + 8].copy_from_slice(&(param.len() as u16).to_le_bytes());  
  
    if !param.is_empty() {  
        buf[off + 8..off + 8 + param.len()].copy_from_slice(param);  
    }
    // ---- 流控 ----  
    for retry in 0..100u32 {
        match sdio.read_byte(1, SDIOWIFI_FLOW_CTRL_REG) {
            Ok(fc) if fc & 0x7F != 0 => break,
            Ok(_) => {}
            Err(_) => return Err("flow_ctrl read error"),
        }
        if retry >= 99 {
            return Err("flow_ctrl timeout"); 
        }
        for _ in 0..5_000 { core::hint::spin_loop(); }
    }

    // ---- 写 FIFO ---- 
    sdio.write_fifo(1, SDIOWIFI_WR_FIFO_ADDR, &buf)
        .map_err(|_| "write_fifo error")?;

    if !wait_cfm {
        return Ok(0);
    }

    // ---- 轮询等待响应 ----  
    let expected_cfm = msg_id + 1;  
    for retry in 0..10_000u32 {
        let raw = sdio.read_byte(1, SDIOWIFI_BLOCK_CNT_REG)  
            .map_err(|_| "read block_cnt error")?;  

        if raw & SDIO_OTHER_INTERRUPT != 0 {  
            // SDIO_OTHER_INTERRUPT — 重试  
            for _ in 0..1_000 { core::hint::spin_loop(); }  
            continue;  
        }  

        let block_cnt = raw & 0x7F;  
        if block_cnt == 0 {  
            if retry > 9_999 {  
                return Err("response timeout");  
            }  
            for _ in 0..1_000 { core::hint::spin_loop(); }  
            continue;  
        } 

        let read_len = (block_cnt as usize) * SDIOWIFI_FUNC_BLOCKSIZE;  
        let mut rx_buf = vec![0u8; read_len];  
        if sdio.read_fifo(1, SDIOWIFI_RD_FIFO_ADDR, &mut rx_buf).is_err() {  
            // CRC 错误 — 重试  
            for _ in 0..100_000 { core::hint::spin_loop(); }  
            continue;  
        } 

        // 解析响应：E2A 方向无 dummy word  
        // rx_buf[0..4] = sdio_header  
        // rx_buf[4..12] = lmac_msg header (id, dest_id, src_id, param_len)  
        if read_len < 12 {  
            return Err("response too short");  
        }  
        let resp_id = u16::from_le_bytes([rx_buf[4], rx_buf[5]]);  
        if resp_id != expected_cfm {  
            log::warn!(  
                "[polling] unexpected resp_id=0x{:04x}, expected=0x{:04x}",  
                resp_id, expected_cfm  
            );  
            // 可能是固件启动通知，丢弃并继续等待  
            continue;  
        } 

        // 拷贝 param 部分到 cfm_buf  
        let param_offset = 12; // sdio_header(4) + lmac_msg_header(8)  
        let cfm_len = cfm_buf.len().min(read_len.saturating_sub(param_offset));  
        if cfm_len > 0 {  
            cfm_buf[..cfm_len].copy_from_slice(&rx_buf[param_offset..param_offset + cfm_len]);  
        }  
        return Ok(cfm_len);  
    }

    Err("response timeout") 
}

/// FDRV 初始化入口  
///  
/// 在 firmware_init 成功后调用。执行以下步骤：  
/// 1. 等待固件 SDIO 接口稳定  
/// 2. 排空残留数据  
/// 3. 轮询模式发送 MM_SET_STACK_START_REQ + MM_RESET_REQ（LMAC 初始化）  
/// 4. 注册 PLIC IRQ#38  
/// 5. 使能 CARD_INT 信号 + AIC8800 芯片端中断  
/// 6. 启动 RX/TX 线程   
pub fn init(sdio: CviSdhci) -> Result<Arc<WifiBus>, &'static str> {  
    // ---- Step 0: 等待固件 SDIO 接口稳定 ----  
    log::info!("[fdrv] waiting for firmware SDIO interface to stabilize...");  
    for _ in 0..20_000_000 { core::hint::spin_loop(); } // ~800ms  

    // ---- Step 1: 排空残留数据 ----  
    for i in 0..5u32 {  
        match sdio.read_byte(1, SDIOWIFI_BLOCK_CNT_REG) {  
            Ok(raw) => {  
                let cnt = raw & 0x7F;  
                if cnt == 0 {  
                    log::info!("[fdrv] drain: no stale data (iteration {})", i);  
                    break;  
                }  
                let len = (cnt as usize) * SDIOWIFI_FUNC_BLOCKSIZE;  
                let mut discard = vec![0u8; len];  
                let _ = sdio.read_fifo(1, SDIOWIFI_RD_FIFO_ADDR, &mut discard);  
                log::info!("[fdrv] drain: discarded {} bytes (block_cnt={})", len, cnt);  
            }  
            Err(e) => {  
                log::warn!("[fdrv] drain: read_byte error: {:?}", e);  
                break;  
            }  
        }  
    }

    // ---- Step 2: 轮询模式 LMAC 初始化 ----  
    // 2a. MM_SET_STACK_START_REQ (is_stack_start=1, efuse_valid=0, set_vendor_info=0, fwtrace_redir=0)  
    let stack_start_param: [u8; 4] = [0x01, 0x00, 0x00, 0x00];  
    let mut cfm = [0u8; 2]; // mm_set_stack_start_cfm: is_5g_support(1) + vendor_info(1)  
    match polling_send_cmd(&sdio, MM_SET_STACK_START_REQ, TASK_MM, &stack_start_param, true, &mut cfm) {  
        Ok(len) => {  
            log::info!(  
                "[fdrv] MM_SET_STACK_START_CFM OK, len={}, is_5g={}, vendor=0x{:02x}",  
                len,  
                if len > 0 { cfm[0] } else { 0 },  
                if len > 1 { cfm[1] } else { 0 }  
            );  
        }  
        Err(e) => {  
            log::error!("[fdrv] MM_SET_STACK_START_REQ failed: {}", e);  
            return Err("MM_SET_STACK_START_REQ failed");  
        }  
    }
    
    // 2b. MM_RESET_REQ (无参数)  
    let mut reset_cfm = [0u8; 0];  
    match polling_send_cmd(&sdio, MM_RESET_REQ, TASK_MM, &[], true, &mut reset_cfm) {
        Ok(_) =>  log::info!("[fdrv] MM_RESET_CFM OK"), 
        Err(e) => {  
            log::error!("[fdrv] MM_RESET_REQ failed: {}", e);  
            return Err("MM_RESET_REQ failed");  
        } 
    }

    // ---- Step 3: 排空 LMAC 初始化产生的残留数据 ----  
    for _ in 0..5_000_000 { core::hint::spin_loop(); } // ~200ms  
    for i in 0..10u32 {
        match sdio.read_byte(1, SDIOWIFI_BLOCK_CNT_REG) {
            Ok(raw) => {
                let cnt = raw & 0x7F;
                if cnt == 0 { break; }
                let len = (cnt as usize) * SDIOWIFI_FUNC_BLOCKSIZE;
                let mut discard = vec![0u8; len];
                sdio.read_fifo(1, SDIOWIFI_RD_FIFO_ADDR, &mut discard);
                log::info!("[fdrv] post-init drain: discarded {} bytes", len); 
            }
            Err(_) => break,
        }
    }

    // ---- Step 4: 创建 WifiBus ----  
    let bus = WifiBus::new(sdio);  
    set_global_bus(&bus);  

    // ---- Step 5: 注册 PLIC IRQ#38 ----  
    let registered = axplat::irq::register(38, sdio1_irq_handler);  
    if !registered {  
        return Err("IRQ#38 registration failed");  
    }  
    log::info!("[fdrv] PLIC IRQ#38 registered");  

    // ---- Step 6: 使能中断 ----  
    {  
        let sdio = bus.sdio.lock();  
        sdio.enable_irq(); // SDHCI CARD_INT 信号  
    }  
    {  
        let sdio = bus.sdio.lock();  
        // AIC8800 芯片端 SDIO 中断使能  
        let _ = sdio.write_byte(1, SDIOWIFI_INTR_CONFIG_REG, 0x07);  
    }  
  
    // 验证 IRQ 触发  
    for _ in 0..100_000 { core::hint::spin_loop(); }  
    let irq_cnt = bus::IRQ_COUNT.load(Ordering::Relaxed);  
    log::info!("[VERIFY-1] IRQ#38 triggered {} times", irq_cnt);  
  
    // ---- Step 7: 启动线程 ----  
    *bus.state.lock() = BusState::Up;  
    rx_thread::start(Arc::clone(&bus));  
    tx_thread::start(Arc::clone(&bus));  
  
    log::info!("[fdrv] AIC8800 FDRV initialized");  
    Ok(bus)      
}

fn drain_stale_data(sdio: &CviSdhci) {
    for i in 0..10 {
        match sdio.read_byte(1, SDIOWIFI_BLOCK_CNT_REG) {
            Ok(raw) => {
                // 跳过 SDIO_OTHER_INTERRUPT (bit 7)  
                if raw & SDIO_OTHER_INTERRUPT != 0 {  
                    log::debug!("[fdrv] drain: SDIO_OTHER_INTERRUPT, raw=0x{:02x}", raw);  
                    continue;  
                }  
                let block_cnt = raw & SDIOWIFI_FLOWCTRL_MASK;  
                if block_cnt == 0 {  
                    log::info!("[fdrv] drain: no stale data (iteration {})", i);  
                    break;  
                } 
                log::info!("[fdrv] drain: block_cnt={}, reading and discarding", block_cnt);  
                let data_len = (block_cnt as usize) * SDIOWIFI_FUNC_BLOCKSIZE;  
                let mut buf = vec![0u8; data_len];  
                match sdio.read_fifo(1, SDIOWIFI_RD_FIFO_ADDR, &mut buf) {
                    Ok(()) => log::info!("[fdrv] drain: discarded {} bytes", data_len),  
                    Err(e) => {  
                        log::warn!("[fdrv] drain: read_fifo failed: {:?} (CRC error expected)", e);  
                        // CRC 错误是预期的（固件刚启动），继续排空  
                    }
                }
            }            
            Err(e) => {
                log::warn!("[fdrv] drain: read_byte failed: {:?}", e);  
                break; 
            }
        }
    }
}