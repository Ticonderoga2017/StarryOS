use alloc::sync::Arc;
use alloc::vec;
use core::{future::poll_fn, sync::atomic::AtomicU64};
use core::task::Poll;
use core::sync::atomic::Ordering;

use axtask::future::block_on;
use log;

use crate::bus::{BusState, WifiBus};
use sdhci_cv1800::{mask_unmask_card_irq_raw, regs::*};
use aic8800_sdio::SdioHost;

const RX_HWHRD_LEN: usize = 60;  
const RX_ALIGNMENT: usize = 4;  
const MAX_PKT_LEN: u16 = 1600;  
const SDIO_OTHER_INTERRUPT: u8 = 0x80; 

pub static RX_WAKE_COUNT: AtomicU64 = AtomicU64::new(0);  

fn align_up(val: usize, align: usize) -> usize {
    (val + align - 1) & !(align - 1)
}

/// 启动 wifi-rx 线程
pub fn start(bus: Arc<WifiBus>) {
    axtask::spawn_with_name(
        move || {
            log::info!("[wifi-rx] thread started");

            block_on(poll_fn(move |cx| {
                // 检查总线状态
                if *bus.state.lock() == BusState::Down {
                    return Poll::Ready(());
                }

                // 检查并清除 ISR 标志  
                if bus.rx_irq_pending.swap(false, Ordering::AcqRel) {
                    RX_WAKE_COUNT.fetch_add(1, Ordering::Relaxed);
                }

                let cnt = RX_WAKE_COUNT.load(Ordering::Relaxed);
                if cnt > 0 {  
                    log::info!("[wifi-rx] woke up (count={})", cnt);  
                } 

                // 处理所有待读数据
                process_rx_frames(&bus);

                bus.rx_irq_pollset.register(cx.waker());

                // 检查 rx_irq_pending 标志  
                //   如果 ISR 在 process_rx_frames 和 register 之间触发，  
                //   rx_irq_pending 会被设置，这里捕获它  
                if bus.rx_irq_pending.swap(false, Ordering::AcqRel) {
                    // ISR 已触发但没有调用 wake()，手动重新处理  
                    process_rx_frames(&bus);  
                    // 重新调度自己，继续检查  
                    cx.waker().wake_by_ref();  
                    return Poll::Pending;  
                }

                cx.waker().wake_by_ref();
                Poll::Pending
            }))
        },
        "wifi-rx".into(),
    );
}

/// 读取 SDIO FIFO 中的所有帧并按类型分发
fn process_rx_frames(bus: &WifiBus) {
    let mut read_any = false;

    loop {
        // 在轮询循环中也检查 rx_irq_pending  
        if bus.rx_irq_pending.swap(false, Ordering::AcqRel) {  
            // ISR 触发了，继续读取（不 break）  
        }

        let block_cnt = {
            let sdio = bus.sdio.lock();
            match sdio.read_byte(1, SDIOWIFI_BLOCK_CNT_REG) {
                Ok(v) => v,
                Err(e) => {
                    log::error!("[wifi-rx] read block_cnt failed: {:?}", e);
                    // mask_unmask_card_irq_raw(bus.sdio_mmio_base.load(Ordering::Acquire), false);
                    break;
                }
            }
        };

        if block_cnt & SDIO_OTHER_INTERRUPT != 0 {
            log::warn!("[wifi-rx] SDIO_OTHER_INTERRUPT (0x{:02x}), re-read", block_cnt);  
            continue;  
        }

        if block_cnt == 0 {
            let mut found = false;
            for poll_i in 0..500u32 {
                for _ in 0..100u32 { core::hint::spin_loop(); }

                 // 轮询期间也检查 rx_irq_pending  
                if bus.rx_irq_pending.swap(false, Ordering::AcqRel) {  
                    // ISR 触发了，重新读 block_cnt  
                } 

                let cnt = {
                    let sdio = bus.sdio.lock();
                    sdio.read_byte(1, SDIOWIFI_BLOCK_CNT_REG).unwrap_or(0)
                };

                if cnt & SDIO_OTHER_INTERRUPT != 0 {
                    continue;
                }

                if cnt > 0 {
                    log::info!(  
                        "[wifi-rx] block_cnt became {} after {}x100us wait",  
                        cnt, poll_i + 1  
                    );  
                    found = true;  
                    break; 
                }
            }

            if !found  {
                log::info!("[wifi-rx] block_cnt=0 after 50ms wait, CARD_INT stays masked");  
                break;
            }
            continue;
        }

        log::info!("[wifi-rx] block_cnt={}", block_cnt);  
        let data_len = (block_cnt as usize) * SDIOWIFI_FUNC_BLOCKSIZE;  
        let mut buf = vec![0u8; data_len]; 
        {
            let sdio = bus.sdio.lock();
            if let Err(e) = sdio.read_fifo(1, SDIOWIFI_RD_FIFO_ADDR, &mut buf) {  
                log::error!("[wifi-rx] read_fifo failed: {:?}", e);  
                break;  
            }  
        }

        read_any = true;
        dispatch_frames(bus, &buf);        
    }

    if read_any {
        let base = bus.sdio_mmio_base.load(Ordering::Acquire);
        if base != 0 {
            mask_unmask_card_irq_raw(base, false);
        }
    }
}

/// 解析 SDIO FIFO 中的聚合帧并按类型分发  
///  
/// buf 布局：一个或多个 SDIO 帧紧密排列（4 字节对齐）  
/// 每帧：[SDIO_HDR(4)] [payload(pkt_len)] [padding]  
///  
/// DATA 帧：pkt_len 包含 SDIO header 4 字节  
///   advance = roundup(pkt_len + RX_HWHRD_LEN, RX_ALIGNMENT)  
///  
/// CFG 帧：pkt_len 不包含 SDIO header 4 字节  
///   advance = roundup(pkt_len, RX_ALIGNMENT) + 4  
fn dispatch_frames(bus: &WifiBus, buf: &[u8]) {
    let mut offset = 0;

    while offset + 4 <= buf.len() {
        let pkt_len = u16::from_le_bytes([buf[offset], buf[offset + 1]]) as usize;
        if pkt_len == 0 || pkt_len > MAX_PKT_LEN as usize {
            break;
        }

        let pkt_type = buf[offset + 2] & 0x7F; // bit6..0 = type, bit7 reserved  
        let is_cfg = (pkt_type & SDIO_TYPE_CFG) == SDIO_TYPE_CFG;
        if !is_cfg {
            // ========== DATA 帧 ==========  
            // pkt_len 包含 SDIO header 的 4 字节（Linux: aggr_len = pkt_len + RX_HWHRD_LEN）  
            // 实际数据从 offset 开始，长度 = pkt_len + RX_HWHRD_LEN  
            let aggr_len = pkt_len + RX_HWHRD_LEN;  
            let advance = align_up(aggr_len, RX_ALIGNMENT);  
  
            if offset + aggr_len > buf.len() {  
                log::warn!("[wifi-rx] DATA frame truncated at offset={}", offset);  
                break;  
            }

            // 802.11 payload 从 offset+4 开始，长度 = pkt_len - 4（去掉 SDIO header）  
            // hw_rxhdr 从 offset+pkt_len 开始，长度 = RX_HWHRD_LEN  
            let data_payload = &buf[offset..offset + aggr_len];  
            log::info!(  
                "[wifi-rx] DATA frame, pkt_len={}, aggr_len={}",  
                pkt_len, aggr_len  
            ); 

            // TODO: 将 data_payload 送入网络栈  
            // 当前阶段只记录日志  
            let mut queue = bus.data_rx_queue.lock();  
            queue.push_back(data_payload.to_vec());  
            drop(queue);  
            offset += advance; 
        } else {
            // ========== CFG 帧 ==========  
            // pkt_len 不包含 SDIO header 的 4 字节  
            // ipc_e2a_msg 从 offset+4 开始，长度 = pkt_len  
            let msg_start = offset + 4;  
            let msg_end = msg_start + pkt_len;  
  
            if msg_end > buf.len() {  
                log::warn!("[wifi-rx] CFG frame truncated at offset={}", offset);  
                break;  
            }  

            let msg_data = &buf[msg_start..msg_end];
            let cfg_subtype = pkt_type & 0x7F;

            // 帧间偏移 = roundup(pkt_len, RX_ALIGNMENT) + 4  
            let advance = align_up(pkt_len, RX_ALIGNMENT) + 4;  
            match cfg_subtype {
                SDIO_TYPE_CFG_CMD_RSP => {
                    // ipc_e2a_msg: [id(2)][dummy_dest(2)][dummy_src(2)][param_len(2)][pattern(4)][param...]  
                    if msg_data.len() >= 8 {
                        let msg_id = u16::from_le_bytes([msg_data[0], msg_data[1]]); 
                        let param_len = u16::from_le_bytes([msg_data[6], msg_data[7]]);  
                        log::info!(  
                            "[wifi-rx] CFG_CMD_RSP: msg_id=0x{:04x}, param_len={}, total={}",  
                            msg_id, param_len, msg_data.len()  
                        );

                        // 判断是 CFM 还是 IND
                        let expected_cfm = bus.cmd_expected_cfm_id.load(Ordering::Acquire);
                        if expected_cfm != 0 && msg_id == expected_cfm {
                            let mut queue = bus.cmd_rsp_queue.lock();
                            queue.push_back(msg_data.to_vec());
                            drop(queue);
                            bus.cmd_rsp_pollset.wake();
                        } else {
                            log::info!(  
                                "[wifi-rx] IND routed: msg_id=0x{:04x} (expected=0x{:04x})",  
                                msg_id, expected_cfm  
                            );
                            let mut queue = bus.ind_queue.lock();
                            queue.push_back(msg_data.to_vec());
                            drop(queue);
                            bus.ind_pollset.wake();
                        }
                    }
                }
                SDIO_TYPE_DATA => {
                    log::info!("[wifi-rx] DATA frame, len={}", pkt_len);  
  
                    // RX DATA 帧格式：  
                    //   [0..60]  hw_rxhdr (60 bytes)  
                    //   [60..62] padding (2 bytes, SDIO alignment)  
                    //   [62..]   802.11 data frame 或 Ethernet frame  
                    //  
                    // FullMAC 模式下，固件已将 802.11 帧转换为 Ethernet 帧：  
                    //   [62..68]  dst_mac (6)  
                    //   [68..74]  src_mac (6)  
                    //   [74..76]  ethertype (2, big-endian)  
                    //   [76..]    payload  

                    const RX_HWHRD_LEN: usize = 60;  
                    const ETH_HDR_OFFSET: usize = RX_HWHRD_LEN + 2; // 62  
                    const ETH_P_PAE: u16 = 0x888E;  
  
                    let data = &buf[offset..offset + pkt_len as usize];  

                    if data.len() >= ETH_HDR_OFFSET + 14 {  
                        let ethertype = u16::from_be_bytes([  
                            data[ETH_HDR_OFFSET + 12],  
                            data[ETH_HDR_OFFSET + 13],  
                        ]);  
  
                        if ethertype == ETH_P_PAE {  
                            // EAPOL 帧：提取 EAPOL payload（跳过 Ethernet 头）  
                            let eapol_start = ETH_HDR_OFFSET + 14; // 76  
                            if data.len() > eapol_start {  
                                let eapol = data[eapol_start..].to_vec();  
                                log::info!(  
                                    "[wifi-rx] EAPOL frame detected, eapol_len={}",  
                                    eapol.len()  
                                );  
                                let mut queue = bus.eapol_queue.lock();  
                                queue.push_back(eapol);  
                                drop(queue);  
                                bus.eapol_pollset.wake();  
                            }  
                        } else {  
                            // 普通 DATA 帧，放入 data_rx_queue  
                            let mut queue = bus.data_rx_queue.lock();  
                            queue.push_back(data.to_vec());  
                            drop(queue);  
                        }  
                    }                  
                }
                SDIO_TYPE_CFG_DATA_CFM => {  
                    log::info!("[wifi-rx] DATA_CFM frame, len={}", pkt_len);  
                    // TX 确认 → 释放 flow control credits  
                } 
                SDIO_TYPE_CFG_PRINT => {  
                    // 固件调试输出  
                    if let Ok(s) = core::str::from_utf8(msg_data) {  
                        log::info!("[fw-print] {}", s.trim_end_matches('\0'));  
                    }  
                }  
                _ => {  
                    log::warn!("[wifi-rx] unknown frame type=0x{:02x}, len={}", cfg_subtype, pkt_len);  
                }
            }
            offset += advance; 
        }     
    }
}
