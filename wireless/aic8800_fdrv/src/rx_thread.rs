use alloc::sync::Arc;
use alloc::vec::Vec;
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

                // 打印后重置 RX_WAKE_COUNT，使其反映"自上次处理以来的中断次数"  
                let cnt = RX_WAKE_COUNT.swap(0, Ordering::Relaxed);  
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

                Poll::Pending
            }))
        },
        "wifi-rx".into(),
    );
}

/// 读取 SDIO FIFO 中的所有帧并按类型分发
fn process_rx_frames(bus: &WifiBus) {
    // SDIO_OTHER_INTERRUPT 重试计数器  
    let mut other_int_retries = 0u32; 

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
                    break;
                }
            }
        };

        if block_cnt & SDIO_OTHER_INTERRUPT != 0 {
            other_int_retries += 1; 
            if other_int_retries > 3 {  
                log::warn!(  
                    "[wifi-rx] SDIO_OTHER_INTERRUPT persists after {} retries, giving up",  
                    other_int_retries  
                );  
                break;  
            }  
            log::warn!("[wifi-rx] SDIO_OTHER_INTERRUPT (0x{:02x}), re-read", block_cnt);  
            continue;  
        }
        other_int_retries = 0; // 成功读取后重置

        if block_cnt == 0 {
            break;
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

        dispatch_frames(bus, &buf);        
    }

    let base = bus.sdio_mmio_base.load(Ordering::Acquire);
    if base != 0 {
        mask_unmask_card_irq_raw(base, false);
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
    const DATA_RX_QUEUE_MAX: usize = 64;
    
    // ---- hw_rxhdr 字段偏移 ----  
    // hw_rxhdr 从 buf[offset] 开始（前 4 字节与 SDIO header 重叠）  
    //   hw_vect (40B) + phy_info (8B) + flags (4B) + pattern (4B) = 56B  
    //   padding 4B → 总计 60B = RX_HWHRD_LEN  
    //  
    // hw_vect.status u32 在 hw_vect 偏移 36:  
    //   bit 0:   rx_vect2_valid  
    //   bit 1:   resp_frame  
    //   bit 2-4: decr_status (3 bits)  
    //   ...  
    const HWVECT_STATUS_OFFSET: usize = 36;  
    //  
    // hw_rxhdr.flags u32 在偏移 48:  
    //   bit 0: flags_is_amsdu  
    //   bit 1: flags_is_80211_mpdu  
    //   bit 2: flags_is_4addr  
    //   ...  
    const FLAGS_OFFSET: usize = 48;  
  
    // 802.11 MPDU 起始偏移 = RX_HWHRD_LEN = 60  
    const MPDU_OFFSET: usize = 60;  
  
    const ETH_P_PAE: u16 = 0x888E;  

    // 解密状态常量 (hw_vect.decr_status)  
    const DECR_UNENC:   u8 = 0;  
    const DECR_WEP:     u8 = 1;  
    const DECR_TKIP:    u8 = 2;  
    const DECR_CCMP128: u8 = 3;  
    const DECR_CCMP256: u8 = 4;  
    const DECR_GCMP128: u8 = 5;  
    const DECR_GCMP256: u8 = 6;  
    const DECR_WAPI:    u8 = 7;  

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
            // 内存布局（从 buf[offset] 开始）：  
            //   [0..56]   hw_rxhdr (56 bytes, 前4字节 = SDIO header)  
            //   [56..60]  padding  (4 bytes, msdu_offset 对齐)  
            //   [60..]    Ethernet 帧 (dst[6] + src[6] + ethertype[2] + payload)  
            //  
            // pkt_len = Ethernet 帧长度（hw_rxhdr.hwvect.len 字段）  
            // aggr_len = pkt_len + RX_HWHRD_LEN (60)  
            // advance  = roundup(aggr_len, RX_ALIGNMENT)  
            //  
            // 注意：SDIO header 的 4 字节已包含在 hw_rxhdr 中，  
            //       所以 advance 不需要额外 +4（与 CFG 帧不同）。 
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

            // --- 从 hw_rxhdr 提取 decr_status ---  
            let decr_status = if data_payload.len() > HWVECT_STATUS_OFFSET {
                (data_payload[HWVECT_STATUS_OFFSET] >> 2) & 0x07  
            } else {  
                DECR_UNENC  
            };  

            // --- 从 hw_rxhdr 提取 flags --- 
            let flags_byte0 = if data_payload.len() > FLAGS_OFFSET {
                data_payload[FLAGS_OFFSET]
            } else {
                0
            };
            let is_80211_npdu = (flags_byte0 >> 1) & 0x01 != 0;

            // 管理帧 (flags_is_80211_mpdu=1) 跳过 
            if is_80211_npdu {
                offset += advance;
                continue;
            }

            // --- 解析 802.11 MPDU ---  
            if pkt_len < 24 || data_payload.len() < MPDU_OFFSET + pkt_len {  
                log::warn!("[wifi-rx] DATA frame too short for 802.11 header");  
                offset += advance;  
                continue;  
            }
            let mpdu = &data_payload[MPDU_OFFSET..MPDU_OFFSET + pkt_len];  
            let fc0 = mpdu[0]; // Frame Control byte 0  
            let fc1 = mpdu[1]; // Frame Control byte 1  

            // 检查是否为 Data 帧 (Type = 2, 即 bits[3:2] = 10)  
            if (fc0 & 0x0C) != 0x08 {  
                offset += advance;  
                continue;  
            } 

            // 确定 802.11 头部长度  
            let is_qos = (fc0 & 0x80) != 0; // QoS Data (subtype bit 3)  
            let mut hdr_len: usize = if is_qos { 26 } else { 24 };  
            if (fc1 & 0x80) != 0 {  
                hdr_len += 4; // +HTC  
            }  

            // 提取 DA (Destination Address) 和 SA (Source Address)  
            let to_ds   = fc1 & 0x01;  
            let from_ds = (fc1 >> 1) & 0x01; 

            let (da, sa): (&[u8], &[u8]) = match (to_ds, from_ds) {  
                (0, 0) => {  
                    // IBSS: DA = Addr1, SA = Addr2  
                    (&mpdu[4..10], &mpdu[10..16])  
                }  
                (1, 0) => {  
                    // To DS: DA = Addr3, SA = Addr2  
                    (&mpdu[16..22], &mpdu[10..16])  
                }  
                (0, 1) => {  
                    // From DS: DA = Addr1, SA = Addr3  
                    (&mpdu[4..10], &mpdu[16..22])  
                }  
                _ => {  
                    // WDS (4-addr): DA = Addr3, SA = Addr4  
                    if pkt_len < 30 {  
                        offset += advance;  
                        continue;  
                    }  
                    (&mpdu[16..22], &mpdu[24..30])  
                }  
            }; 

            // 加密头长度（固件已解密，但加密头仍在 MPDU 中）  
            let crypto_hdr_len: usize = match decr_status {  
                DECR_CCMP128 | DECR_CCMP256 |  
                DECR_GCMP128 | DECR_GCMP256 => 8,  
                DECR_TKIP => 8,  
                DECR_WEP  => 4,  
                DECR_WAPI => 18,  
                _         => 0, // DECR_UNENC  
            };  
  
            // LLC/SNAP 头 (8 bytes): AA AA 03 00 00 00 [ethertype 2B]  
            // ethertype 在 LLC/SNAP 的第 6-7 字节  
            let llc_offset = hdr_len + crypto_hdr_len;  
            let ether_type_offset = llc_offset + 6;  
  
            if pkt_len < ether_type_offset + 2 {  
                // MPDU 太短，无法提取 ethertype  
                log::warn!(  
                    "[wifi-rx] MPDU too short for LLC/SNAP: pkt_len={}, need={}",  
                    pkt_len, ether_type_offset + 2  
                );  
                offset += advance;  
                continue;  
            }

            // 可选：验证 LLC/SNAP 头 (AA AA 03)  
            // if mpdu[llc_offset] != 0xAA || mpdu[llc_offset+1] != 0xAA || mpdu[llc_offset+2] != 0x03 {  
            //     // 非 LLC/SNAP 封装，跳过  
            //     offset += advance;  
            //     continue;  
            // }  
  
            let ethertype = u16::from_be_bytes([  
                mpdu[ether_type_offset],  
                mpdu[ether_type_offset + 1],  
            ]);

            // payload 起始 = 802.11 header + crypto header + LLC/SNAP (8 bytes)  
            let payload_start = llc_offset + 8;  

            if ethertype == ETH_P_PAE {
                // ===== EAPOL 帧 =====  
                if pkt_len > payload_start {  
                    let eapol = mpdu[payload_start..].to_vec();  
                    log::info!(  
                        "[wifi-rx] EAPOL frame detected, eapol_len={}, decr={}",  
                        eapol.len(), decr_status  
                    );  
                    let mut queue = bus.eapol_queue.lock();  
                    queue.push_back(eapol);  
                    drop(queue);  
                    bus.eapol_pollset.wake();  
                }
            } else {
                // ===== 普通 DATA 帧：构造 Ethernet 帧 =====  
                // Ethernet 帧 = DA(6) + SA(6) + ethertype(2) + payload  
                if pkt_len > payload_start {  
                    let payload = &mpdu[payload_start..];  
                    let mut eth_frame = Vec::with_capacity(14 + payload.len());  
                    eth_frame.extend_from_slice(da);          // DA (6B)  
                    eth_frame.extend_from_slice(sa);          // SA (6B)  
                    eth_frame.extend_from_slice(&mpdu[ether_type_offset..ether_type_offset + 2]); // ethertype (2B)  
                    eth_frame.extend_from_slice(payload);     // payload  
  
                    let mut queue = bus.data_rx_queue.lock();  
                    if queue.len() >= DATA_RX_QUEUE_MAX {  
                        queue.pop_front(); // 丢弃最旧的帧  
                    }  
                    queue.push_back(eth_frame);  
                    drop(queue);  
                } 
            }
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
            // let cfg_subtype = pkt_type & 0x7F;  
            let cfg_subtype = pkt_type;
  
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
                SDIO_TYPE_CFG_DATA_CFM => {  
                    if msg_data.len() >= 8 {  
                        let status = u32::from_le_bytes([msg_data[0], msg_data[1], msg_data[2], msg_data[3]]);  
                        let used_idx = u32::from_le_bytes([msg_data[4], msg_data[5], msg_data[6], msg_data[7]]);  
                        let tx_done = status & 1;  
                        let retry_req = (status >> 1) & 1;  
                        let sw_retry_req = (status >> 2) & 1;  
                        let acknowledged = (status >> 3) & 1;  
                        log::info!(  
                            "[wifi-rx] DATA_CFM: status=0x{:08x} (done={}, retry={}, sw_retry={}, ack={}), used_idx={}",  
                            status, tx_done, retry_req, sw_retry_req, acknowledged, used_idx  
                        );  
                    }  
                }  
                SDIO_TYPE_CFG_PRINT => {  
                    // 固件调试输出  
                    if let Ok(s) = core::str::from_utf8(msg_data) {  
                        log::info!("[fw-print] {}", s.trim_end_matches('\0'));  
                    }  
                }  
                _ => {  
                    log::warn!(  
                        "[wifi-rx] unknown frame type=0x{:02x}, len={}",  
                        cfg_subtype, pkt_len  
                    );  
                }  
            } 
            offset += advance; 
        }     
    }
}
