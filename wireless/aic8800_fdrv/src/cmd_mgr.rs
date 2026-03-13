use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::future::poll_fn;
use core::sync::atomic::Ordering;
use core::task::Poll;

use axtask::future::block_on;
use log;

use crate::{bus::{BusState, WifiBus}, lmac_msg::*};
use sdhci_cv1800::regs::*; 

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
    log::info!("[cmd_mgr] send_cmd enter: msg_id=0x{:04x}", msg_id);

    // 检查总线状态  
    if *bus.state.lock() == BusState::Down {
        return Err(CmdError::BusDown);
    }

    log::info!("[cmd_mgr] state check passed");

    let timeout = if timeout_ms == 0 {
        CMD_TX_TIMEOUT_DEFAULT_MS
    } else {
        timeout_ms
    };

    // ---- 构造 SDIO 帧 ----
    let frame = build_cmd_frame(msg_id, dest_id, param);

    log::info!("[cmd_mgr] frame built, len={}", frame.len());

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

// ================================================================
// 1. TX Power Index (AIC8801 默认值)
// ================================================================

/// 发送 MM_SET_TXPWR_IDX_LVL_REQ
pub fn send_txpwr_idx_req(
    bus: &Arc<WifiBus>,
    timeout_ms: u64,
) -> Result<(), CmdError> {
    // txpwr_idx_conf_t: 10 bytes
    // [enable, dsss, ofdmlowrate_2g4, ofdm64qam_2g4, ofdm256qam_2g4,
    //  ofdm1024qam_2g4, ofdmlowrate_5g, ofdm64qam_5g, ofdm256qam_5g, ofdm1024qam_5g]
    let param: [u8; 10] = [
        1,   // enable
        9,   // dsss
        8,   // ofdmlowrate_2g4
        8,   // ofdm64qam_2g4
        8,   // ofdm256qam_2g4
        8,   // ofdm1024qam_2g4
        11,  // ofdmlowrate_5g
        10,  // ofdm64qam_5g
        9,   // ofdm256qam_5g
        9,   // ofdm1024qam_5g
    ];
    log::info!("[lmac] sending MM_SET_TXPWR_IDX_LVL_REQ");
    send_cmd(bus, MM_SET_TXPWR_IDX_LVL_REQ, TASK_MM, &param, timeout_ms)?;
    Ok(())
}

// ================================================================
// 2. TX Power Offset
// ================================================================

/// 发送 MM_SET_TXPWR_OFST_REQ
pub fn send_txpwr_ofst_req(
    bus: &Arc<WifiBus>,
    timeout_ms: u64,
) -> Result<(), CmdError> {
    // txpwr_ofst_conf_t: 8 bytes
    // [enable, chan_1_4, chan_5_9, chan_10_13, chan_36_64, chan_100_120, chan_122_140, chan_142_165]
    let param: [u8; 8] = [
        1,  // enable
        0,  // chan_1_4
        0,  // chan_5_9
        0,  // chan_10_13
        0,  // chan_36_64
        0,  // chan_100_120
        0,  // chan_122_140
        0,  // chan_142_165
    ];
    log::info!("[lmac] sending MM_SET_TXPWR_OFST_REQ");
    send_cmd(bus, MM_SET_TXPWR_OFST_REQ, TASK_MM, &param, timeout_ms)?;
    Ok(())
}

// ================================================================
// 3. RF Calibration
// ================================================================

/// 发送 MM_SET_RF_CALIB_REQ (AIC8801 版本，非 v2)
pub fn send_rf_calib_req(
    bus: &Arc<WifiBus>,
    timeout_ms: u64
) -> Result<Vec<u8>, CmdError> {
    // mm_set_rf_calib_req 结构体 (AIC8801):
    // [0..4]   cal_cfg_24g   (u32 LE) = 0xbf
    // [4..8]   cal_cfg_5g    (u32 LE) = 0x3f
    // [8..12]  param_alpha   (u32 LE) = 0x0c34c008
    // [12..16] bt_calib_en   (u32 LE) = 0
    // [16..20] bt_calib_param(u32 LE) = 0x264203
    // [20]     xtal_cap      (u8)     = 0
    // [21]     xtal_cap_fine (u8)     = 0
    // 总计 22 字节

    let mut param = [0u8; 22];
    param[0..4].copy_from_slice(&0x000000bfu32.to_le_bytes());   // cal_cfg_24g
    param[4..8].copy_from_slice(&0x0000003fu32.to_le_bytes());   // cal_cfg_5g
    param[8..12].copy_from_slice(&0x0c34c008u32.to_le_bytes());  // param_alpha
    param[12..16].copy_from_slice(&0u32.to_le_bytes());          // bt_calib_en
    param[16..20].copy_from_slice(&0x00264203u32.to_le_bytes()); // bt_calib_param
    param[20] = 0; // xtal_cap
    param[21] = 0; // xtal_cap_fine

    log::info!("[lmac] sending MM_SET_RF_CALIB_REQ");
    let rsp = send_cmd(bus, MM_SET_RF_CALIB_REQ, TASK_MM, &param, timeout_ms)?;
    // CFM: mm_set_rf_calib_cfm = 4 x u32 (rxgain_24g_addr, rxgain_5g_addr, txgain_24g_addr, txgain_5g_addr)
    if rsp.len() >= 16 {
        let rxgain_24g = u32::from_le_bytes([rsp[0], rsp[1], rsp[2], rsp[3]]);
        let txgain_24g = u32::from_le_bytes([rsp[8], rsp[9], rsp[10], rsp[11]]);
        log::info!("[lmac] RF calib OK: rxgain_24g=0x{:08x}, txgain_24g=0x{:08x}", rxgain_24g, txgain_24g);
    }
    Ok(rsp)
}

// ================================================================
// 4. ME_CONFIG_REQ — 最小 HT 配置
// ================================================================

/// 发送 ME_CONFIG_REQ（最小配置：HT only, 20MHz, 1SS）
/// 对应 Linux: rwnx_send_me_config_req (line 2566-2688)
///
/// me_config_req 结构体布局:
///   struct mac_htcapability ht_cap;    // 32 bytes
///   struct mac_vhtcapability vht_cap;  // 12 bytes
///   struct mac_hecapability he_cap;    // 52 bytes (估算)
///   u16 tx_lft;
///   u8  phy_bw_max;
///   bool ht_supp;
///   bool vht_supp;
///   bool he_supp;
///   bool he_ul_on;
///   bool ps_on;
///   bool ant_div_on;
///   bool dpsm;
///
/// 最小移植策略：全部填零，只设置关键字段
pub fn send_me_config_req(
    bus: &Arc<WifiBus>,
    timeout_ms: u64,
) -> Result<Vec<u8>, CmdError> {
    // mac_htcapability:  26 bytes (2+1+16+2+4+1)  
    // mac_vhtcapability: 12 bytes (4+2+2+2+2)  
    // mac_hecapability:  54 bytes (6+11+12+25)  
    // tail fields:       10 bytes (2+1+1+1+1+1+1+1+1)  
    // 总计: 102 bytes  

    const HT_CAP_SIZE: usize = 26;  
    const VHT_CAP_SIZE: usize = 12;  
    const HE_CAP_SIZE: usize = 54;  
    const ME_CONFIG_SIZE: usize = HT_CAP_SIZE + VHT_CAP_SIZE + HE_CAP_SIZE + 10; // 102  
  
    let mut param = [0u8; ME_CONFIG_SIZE];  

    // ht_cap (offset 0)  
    param[0..2].copy_from_slice(&0x0001u16.to_le_bytes()); // ht_capa_info = LDPC  
    param[2] = 3 | (7 << 2); // a_mpdu_param  
    param[3] = 0xFF; // mcs_rate[0]  
  
    // vht_cap (offset 26) — 全零  
    // he_cap  (offset 38) — 全零  
  
    let tail = HT_CAP_SIZE + VHT_CAP_SIZE + HE_CAP_SIZE; // 92  
    // tx_lft (u16) = 0  
    // phy_bw_max (u8) = 0  
    param[tail + 2] = 0; // PHY_CHNL_BW_20  
    // ht_supp = 1  
    param[tail + 3] = 1;  
    // vht_supp..dpsm = 0  
  
    log::info!("[lmac] sending ME_CONFIG_REQ (HT only, 20MHz, 1SS)");  
    send_cmd(bus, ME_CONFIG_REQ, TASK_ME, &param, timeout_ms)  
}

/// 发送 ME_CHAN_CONFIG_REQ（2.4GHz 信道 1-14）
///
/// me_chan_config_req 结构体:
///   mac_chan_def chan2G4[14];  // 14 * 4 bytes = 56 bytes
///   mac_chan_def chan5G[28];   // 28 * 4 bytes = 112 bytes
///   u8 chan2G4_cnt;
///   u8 chan5G_cnt;
///
/// mac_chan_def:
///   u16 freq;      // MHz
///   u8  band;      // 0 = 2.4GHz
///   u8  flags;     // 0 = enabled
///   s8  tx_power;  // dBm (Linux 用 30 dBm)
///   → 实际 5 bytes，但可能有 padding → 按 Linux 对齐
///
/// 注意：mac_chan_def 在 Linux 中是 5 字节 + padding
/// 实际布局需要与固件匹配
pub fn send_me_chan_config_req(
    bus: &Arc<WifiBus>,
    timeout_ms: u64
) -> Result<Vec<u8>, CmdError> {
    // mac_chan_def 大小: freq(2) + band(1) + flags(1) + tx_power(1) = 5 bytes
    // 假设无 padding = 5 bytes per entry
    const CHAN_DEF_SIZE: usize = 5; // mac_chan_def 大小
    const MAX_2G4: usize = 14;
    const MAX_5G: usize = 28;

    // 总大小 = 14*5 + 28*5 + 1 + 1 = 70 + 140 + 2 = 212
    let total_size = MAX_2G4 * CHAN_DEF_SIZE + MAX_5G * CHAN_DEF_SIZE + 2;
    let mut param = alloc::vec![0u8; total_size];

    // 填充 chan2G4[0..14]
    let chan_cnt = CHAN_2G4_FREQS.len().min(MAX_2G4);
    for i in 0..chan_cnt {
        let off = i * CHAN_DEF_SIZE;
        param[off..off + 2].copy_from_slice(&CHAN_2G4_FREQS[i].to_le_bytes()); // freq
        param[off + 2] = 0; // band = NL80211_BAND_2GHZ
        param[off + 3] = 0; // flags = 0 (enabled)
        param[off + 4] = 30; // tx_power = 30 dBm (与 Linux 一致)
    }
    
    // chan5G[0..28] 全零（不使用 5GHz）

    // chan2G4_cnt 和 chan5G_cnt 在尾部
    let cnt_offset = MAX_2G4 * CHAN_DEF_SIZE + MAX_5G * CHAN_DEF_SIZE;
    param[cnt_offset] = chan_cnt as u8;     // chan2G4_cnt
    param[cnt_offset + 1] = 0;             // chan5G_cnt

    log::info!("[lmac] sending ME_CHAN_CONFIG_REQ ({} 2.4GHz channels)", chan_cnt);
    send_cmd(bus, ME_CHAN_CONFIG_REQ, TASK_ME, &param, timeout_ms)
}

// ================================================================
// 6. MM_START_REQ — 启动 MAC
// ================================================================

/// 发送 MM_START_REQ
/// 对应 Linux: rwnx_send_start (line 473-492)
///
/// mm_start_req 结构体:
///   phy_cfg_tag phy_cfg;     // 16 * u32 = 64 bytes (全零即可)
///   u32 uapsd_timeout;       // 300 (Linux 默认)
///   u16 lp_clk_accuracy;     // 20 ppm (Linux 默认)
pub fn send_mm_start_req(
    bus: &Arc<WifiBus>,
    timeout_ms: u64
) -> Result<Vec<u8>, CmdError> {
    // phy_cfg: 16 * 4 = 64 bytes (全零 — AIC8801 不需要 PHY 配置)
    // uapsd_timeout: 4 bytes
    // lp_clk_accuracy: 2 bytes
    // 总计 70 bytes

    let mut param = [0u8; 70];
    // phy_cfg[0..64] = 全零
    // uapsd_timeout = 300 (Linux 默认值)
    param[64..68].copy_from_slice(&300u32.to_le_bytes());
    // lp_clk_accuracy = 20 ppm
    param[68..70].copy_from_slice(&20u16.to_le_bytes());

    log::info!("[lmac] sending MM_START_REQ");
    send_cmd(bus, MM_START_REQ, TASK_MM, &param, timeout_ms)
}

/// 发送 MM_ADD_IF_REQ（创建 STA 接口）
/// 对应 Linux: rwnx_send_add_if (line 508-562)
///
/// mm_add_if_req 结构体:
///   u8 type;           // MM_STA = 0
///   mac_addr addr;     // 6 bytes (struct mac_addr { u16 array[3]; })
///   bool p2p;          // false
///
/// 返回 CFM 中的 vif_index (mm_add_if_cfm.inst_nbr)
pub fn send_mm_add_if_req(
    bus: &Arc<WifiBus>,
    mac_addr: &[u8; 6],
    timeout_ms: u64
) -> Result<u8, CmdError> {
    // mm_add_if_req: type(1) + addr(6) + p2p(1) = 8 bytes
    let mut param = [0u8; 8];
    param[0] = MM_STA;                    // type = STA
    param[1..7].copy_from_slice(mac_addr); // MAC address
    param[7] = 0;                          // p2p = false

    log::info!(
        "[lmac] sending MM_ADD_IF_REQ (STA, mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})",
        mac_addr[0], mac_addr[1], mac_addr[2],
        mac_addr[3], mac_addr[4], mac_addr[5]
    );

    let rsp = send_cmd(bus, MM_ADD_IF_REQ, TASK_MM, &param, timeout_ms)?;

    // mm_add_if_cfm:
    //   u8 status;     // 0 = success
    //   u8 inst_nbr;   // VIF index
    if rsp.len() >= 2 {
        let status = rsp[0];
        let vif_idx = rsp[1];
        if status != 0 {
            log::error!("[lmac] MM_ADD_IF_CFM status={} (error)", status);
            return Err(CmdError::FirmwareError);
        }
        log::info!("[lmac] MM_ADD_IF_CFM OK: vif_index={}", vif_idx);
        Ok(vif_idx)
    } else {
        log::error!("[lmac] MM_ADD_IF_CFM too short: {} bytes", rsp.len());
        Err(CmdError::InvalidResponse)
    }
}