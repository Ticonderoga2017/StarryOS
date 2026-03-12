use alloc::sync::Arc;
use alloc::vec;
use core::future::poll_fn;
use core::task::Poll;

use axtask::future::{block_on, register_irq_waker};
use log::{self, info};

use crate::bus::{BusState, WifiBus};
use sdhci_cv1800::{CviSdhci, regs::*};
use aic8800_sdio::SdioHost;

const SDIO1_IRQ: usize = 38;
const RX_HWHRD_LEN: usize = 60;
const RX_ALIGNMENT: usize = 4;

const SDIO_TYPE_DATA: u8         = 0x00;  
const SDIO_TYPE_CFG: u8          = 0x10;  // 用于类型判断的 mask  
const SDIO_TYPE_CFG_CMD_RSP: u8  = 0x11;  
const SDIO_TYPE_CFG_DATA_CFM: u8 = 0x12;  
const SDIO_TYPE_CFG_PRINT: u8    = 0x13;  
  
const SDIOWIFI_BLOCK_CNT_REG: u32 = 0x12;  
const SDIOWIFI_RD_FIFO_ADDR: u32  = 0x08;  
const MAX_PKT_LEN: u16 = 1600;  
const DUMMY_WORD_LEN: usize = 4;  

fn align_up(val: usize, align: usize) -> usize {
    (val + align - 1) & !(align - 1)
}

/// 启动 wifi-rx 线程
pub fn start(bus: Arc<WifiBus>) {
    axtask::spawn_with_name(
        move || {
            info!("[wifi-rx] thread started");
            block_on(poll_fn(|cx| {
                // 检查总线状态
                if *bus.state.lock() == BusState::Down {
                    return Poll::Ready(());
                }
                // 处理所有待读数据
                process_rx_frames(&bus);
                // 注册 waker：下次 CARD_INT 时唤醒
                // （双重检查：先处理 → 注册 → 再处理，避免 race）
                register_irq_waker(SDIO1_IRQ, cx.waker());

                // 二次检查（注册和中断之间可能有数据到达）
                process_rx_frames(&bus);

                Poll::Pending
            }))
        },
        "wifi-rx".into(),
    );
}

/// 读取 SDIO FIFO 中的所有帧并按类型分发
fn process_rx_frames(bus: &WifiBus) {
    loop {
        // 持锁读 SDIO
        let mut sdio = bus.sdio.lock();

        // 读 block_cnt_reg (AIC8801: 0x12)
        let block_cnt = match sdio.read_byte(1, SDIOWIFI_BLOCK_CNT_REG) {
            Ok(cnt) => cnt & 0x7F,
            Err(e) => {
                log::error!("[wifi-rx] read block_cnt failed: {:?}", e);
                CviSdhci::unmask_card_irq(sdio.mmio_base());
                break;
            }
        };

        if block_cnt == 0 {
            // 无数据，恢复 CARD_INT 信号
            CviSdhci::unmask_card_irq(sdio.mmio_base());
            break;
        }

        // 计算数据长度并读 FIFO
        let data_len = (block_cnt as usize) * SDIOWIFI_FUNC_BLOCKSIZE;
        let mut buf = vec![0u8; data_len];
        if let Err(e) = sdio.read_fifo(1, SDIOWIFI_RD_FIFO_ADDR, &mut buf) {
            log::error!("[wifi-rx] read_fifo failed: {:?}", e);
            CviSdhci::unmask_card_irq(sdio.mmio_base());
            break;
        }

        // 释放 SDIO 锁后再处理帧（减少持锁时间）
        drop(sdio);

        // 解析并分发帧（可能有聚合帧）
        dispatch_frames(bus, &buf);
    }
}

/// 解析 SDIO 帧并按类型分发到对应队列
fn dispatch_frames(bus: &WifiBus, data: &[u8]) {
    let mut offset = 0;
    while offset + 4 <= data.len() {
        // sdio_header: [len_lo, len_hi, type, ...]
        let pkt_len = (data[offset] as u16) | ((data[offset + 1] as u16 & 0x0F) << 8);

        if pkt_len == 0{
            break;
        }
        let frame_type = data[offset + 2] & 0x7F;

        // 合法性检查  
        if pkt_len > MAX_PKT_LEN {
            log::warn!("[wifi-rx] pkt_len {} > {}, skip rest", pkt_len, MAX_PKT_LEN);  
            break;  
        }

        // 判断帧类型：bit4 为 1 → CFG 类帧，否则为数据帧  
        if (frame_type & SDIO_TYPE_CFG) != SDIO_TYPE_CFG {
            // ---- 数据帧 ----  
            // Linux: aggr_len = pkt_len + RX_HWHRD_LEN  
            // RX_HWHRD_LEN(60) 包含 sdio_header(4) + hw_rxhdr(56)  
            // 所以 aggr_len 是从 data[offset] 开始的完整帧长度 
            let aggr_len = pkt_len as usize + RX_HWHRD_LEN;
            let adjust_len = align_up(aggr_len, RX_ALIGNMENT);

            if offset + aggr_len > data.len() {  
                log::warn!("[wifi-rx] data frame truncated: need {}, have {}",  
                    aggr_len, data.len() - offset);  
                break;  
            } 

            // 拷贝完整帧（含 sdio_header + hw_rxhdr + 802.11 payload）  
            let frame = data[offset..offset + aggr_len].to_vec();
            bus.data_rx_queue.lock().push_back(frame);
            bus.data_rx_pollset.wake();

            // 数据帧：前进 adjust_len（不加 4，因为 RX_HWHRD_LEN 已含 sdio_header）  
            offset += adjust_len; 
        } else {
            // ---- CFG 类帧 (CMD_RSP / DATA_CFM / PRINT) ----  
            // Linux: aggr_len = pkt_len（不含 sdio_header）  
            // 完整帧 = sdio_header(4) + pkt_len  
            let aggr_len = pkt_len as usize;  
            let adjust_len = align_up(aggr_len, RX_ALIGNMENT);  
  
            if offset + adjust_len + 4 > data.len() {  
                log::warn!("[wifi-rx] cfg frame truncated");  
                break;  
            }  

            let sub_type = frame_type & 0x7F;  
            match sub_type {
                SDIO_TYPE_CFG_CMD_RSP => {
                    // SDIO_TYPE_CFG_CMD_RSP  
                    // Linux: rwnx_rx_handle_msg(hw, (ipc_e2a_msg *)(msg + 4))  
                    // msg+4 = 跳过 sdio_header 后再跳过 dummy word  
                    // 即 data[offset + 4 + DUMMY_WORD_LEN .. offset + aggr_len + 4]  
                    if DUMMY_WORD_LEN <= aggr_len {
                        let msg = data[offset + 4 + DUMMY_WORD_LEN..offset + aggr_len + 4].to_vec();
                        bus.cmd_rsp_queue.lock().push_back(msg);
                        bus.cmd_rsp_pollset.wake();
                    }
                }
                SDIO_TYPE_CFG_DATA_CFM => {
                    if DUMMY_WORD_LEN <= aggr_len {
                        let cfm = data[offset + 4 + DUMMY_WORD_LEN..offset + aggr_len + 4].to_vec();
                        bus.tx_cfm_queue.lock().push_back(cfm);
                        bus.tx_cfm_pollset.wake();
                    }
                }
                SDIO_TYPE_CFG_PRINT => {
                    // SDIO_TYPE_CFG_PRINT  
                    // Linux: rwnx_rx_handle_print(hw, msg + 4, aggr_len)  
                    // msg+4 = 跳过 sdio_header，aggr_len 包含 dummy word  
                    // 实际字符串从 dummy word 之后开始  
                    if DUMMY_WORD_LEN <= aggr_len {
                        let print_data = &data[offset + 4 + DUMMY_WORD_LEN..offset + aggr_len + 4];
                        if let Ok(s) = core::str::from_utf8(print_data) {
                            log::info!("[AIC8800] {}", s.trim_end_matches('\0').trim()); 
                        }
                    }
                }
                _ => {
                    log::warn!("[wifi-rx] unknown cfg sub_type: 0x{:02x}", sub_type); 
                }
            }
            // CFG 帧：前进 adjust_len + 4（sdio_header 不计入 pkt_len）  
            offset += adjust_len + 4;  
        }       
    }
}
