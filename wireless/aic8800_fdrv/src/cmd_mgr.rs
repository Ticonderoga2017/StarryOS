use alloc::{collections::vec_deque::VecDeque, sync::Arc};
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
    send_cmd_with_cfm_id(bus, msg_id, dest_id, param, msg_id + 1, timeout_ms)
}

/// 发送 LMAC 命令并等待指定 CFM ID 的响应  
///  
/// 与 `send_cmd` 的区别：可以指定期望的 CFM ID。  
/// 用于扫描等命令（`SCANU_START_REQ` 等待 `SCANU_START_CFM_ADDTIONAL`）。  
///  
/// 不匹配的消息（indication）会被路由到 `bus.ind_queue`。 
pub fn send_cmd_with_cfm_id(
    bus: &Arc<WifiBus>,
    msg_id: u16,
    dest_id: u16,
    param: &[u8],
    expected_cfm_id: u16,
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
    bus.cmd_expected_cfm_id.store(expected_cfm_id, Ordering::Release);

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
                        bus.ind_queue.lock().push_back(rsp);
                        bus.ind_pollset.wake();
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
                    } else {
                        log::info!(  
                            "[cmd_mgr] IND routed (double-check): msg_id=0x{:04x}",  
                            msg.id  
                        );  
                        bus.ind_queue.lock().push_back(rsp);  
                        bus.ind_pollset.wake();  
                    }
                }                
            }
        }

        // 保持活跃以确保超时检查能触发  
        cx.waker().wake_by_ref(); 
        Poll::Pending
    }));

    // 清除期望的 CFM ID（无论成功、超时还是错误） 
    bus.cmd_expected_cfm_id.store(0, Ordering::Release);

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

// ================================================================  
// 扫描命令  
// ================================================================  
  
/// 发送 SCANU_START_REQ（WiFi 扫描）  
///  
/// 扫描流程：  
///   1. 发送 SCANU_START_REQ  
///   2. 固件返回 N 个 SCANU_RESULT_IND（每个 AP 一个）→ 路由到 ind_queue  
///   3. 固件返回 SCANU_START_CFM_ADDTIONAL（扫描完成）→ 作为 CFM 返回  
///  
/// 参数：  
///   - `vif_idx`: VIF 索引（从 MM_ADD_IF_REQ 获得）  
///   - `ssid`: 可选的目标 SSID（None = 被动扫描/广播扫描）  
///   - `timeout_ms`: 超时时间（建议 15000-20000ms）  
///  
/// 返回 SCANU_START_CFM 的 param（3 字节: vif_idx, status, result_cnt） 
pub fn send_scanu_start_req(
    bus: &Arc<WifiBus>,
    vif_idx: u8,
    ssid: Option<&[u8]>,
    timeout_ms: u64,
) -> Result<Vec<u8>, CmdError> {
    // 构造 scanu_start_req param  
    // 布局:  
    //   chan[SCAN_CHANNEL_MAX] * MAC_CHAN_DEF_SIZE  = 42 * 5 = 210  
    //   ssid[SCAN_SSID_MAX] * MAC_SSID_SIZE        = 3 * 33 = 99  
    //   bssid                                       = 6  
    //   add_ies (u32)                               = 4  
    //   add_ie_len (u16)                            = 2  
    //   vif_idx (u8)                                = 1  
    //   chan_cnt (u8)                                = 1  
    //   ssid_cnt (u8)                               = 1  
    //   no_cck (bool/u8)                            = 1  
    //   duration (u32)                              = 4  
    //   总计                                        = 329  
    let mut param = vec![0u8; SCANU_START_REQ_SIZE];

    // ---- 填充 chan[0..14]（2.4GHz 信道 1-14）---- 
    let chan_cnt = CHAN_2G4_FREQS.len(); //14
    for i in 0..chan_cnt {
        let off = i * MAC_CHAN_DEF_SIZE;
        param[off..off + 2].copy_from_slice(&CHAN_2G4_FREQS[i].to_le_bytes()); // freq  
        param[off + 2] = 0; // band = NL80211_BAND_2GHZ  
        param[off + 3] = 0; // flags = 0  
        param[off + 4] = 30; // tx_power = 30 dBm  
    }

    // ---- 填充 ssid[0]（如果指定）----  
    let ssid_offset = SCAN_CHANNEL_MAX * MAC_CHAN_DEF_SIZE; // 210
    let ssid_cnt = if let Some(s) = ssid {
        let len = s.len().min(MAC_SSID_LEN);
        param[ssid_offset] = len as u8; //ssid[0].length
        param[ssid_offset + 1..ssid_offset + 1 + len].copy_from_slice(&s[..len]); // ssid[0].array  
        1u8
    } else {
        0u8
    };

    // ---- 填充 bssid（广播地址 FF:FF:FF:FF:FF:FF）----  
    let bssid_offset = ssid_offset + SCAN_SSID_MAX * MAC_SSID_SIZE; // 210 + 99 = 309  
    param[bssid_offset..bssid_offset + 6].copy_from_slice(&[0xFF; 6]); 

    // ---- 填充尾部字段 ----  
    let tail_offset = bssid_offset + MAC_ADDR_SIZE; // 309 + 6 = 315  
    // add_ies (u32) = 0  
    // add_ie_len (u16) = 0  
    param[tail_offset + 6] = vif_idx;       // vif_idx  
    param[tail_offset + 7] = chan_cnt as u8; // chan_cnt  
    param[tail_offset + 8] = ssid_cnt;      // ssid_cnt  
    param[tail_offset + 9] = 0;             // no_cck = false  
    // duration (u32) = 0（使用固件默认值）  

    log::info!(  
        "[cmd_mgr] sending SCANU_START_REQ: vif_idx={}, chan_cnt={}, ssid_cnt={}, param_size={}",  
        vif_idx, chan_cnt, ssid_cnt, param.len()  
    );  

    // 等待 SCANU_START_CFM_ADDTIONAL (0x1009)，而非 SCANU_START_CFM (0x1001)  
    send_cmd_with_cfm_id(  
        bus,  
        SCANU_START_REQ,           // 0x1000  
        msg_t(TASK_SCANU, 0),      // dest_id = (TASK_SCANU << 8) | 0  
        &param,  
        SCANU_START_CFM_ADDTIONAL, // 0x1009  
        timeout_ms,  
    ) 
}

/// 从 ind_queue 中收集所有 SCANU_RESULT_IND 并解析为 ScanResult  
///  
/// 在 `send_scanu_start_req` 返回后调用。  
/// 扫描期间固件发送的 SCANU_RESULT_IND 已被路由到 ind_queue。  
pub fn collect_scan_results(bus: &Arc<WifiBus>) -> Vec<ScanResult> {
    let mut results = Vec::new();
    let mut queue = bus.ind_queue.lock();

    // 取出所有 SCANU_RESULT_IND，其他 indication 放回  
    let mut remaining = VecDeque::new();
    while let Some(msg_data) = queue.pop_front() {
        if msg_data.len() >= LmacMsg::SIZE {
            let msg = LmacMsg::from_le_bytes(&msg_data);
            if msg.id == SCANU_RESULT_IND {
                // 解析 SCANU_RESULT_IND  
                let param = &msg_data[LmacMsg::SIZE..];
                if let Some(result) = parse_scanu_result_ind(param) {
                    results.push(result);
                }
                continue;
            }
        }
        // 非 SCANU_RESULT_IND，放回队列  
        remaining.push_back(msg_data);
    }

    // 将非扫描 indication 放回  
    *queue = remaining;

    results
}

/// 解析 SCANU_RESULT_IND 的 param 部分  
///  
/// param 布局（scanu_result_ind）:  
///   [0..2]  length (u16)     — 802.11 帧长度  
///   [2..4]  framectrl (u16)  — Frame Control  
///   [4..6]  center_freq (u16)  
///   [6]     band (u8)  
///   [7]     sta_idx (u8)  
///   [8]     inst_nbr (u8)  
///   [9]     rssi (i8)  
///   [10..]  payload — 802.11 管理帧（Beacon/ProbeResp）
fn parse_scanu_result_ind(param: &[u8]) -> Option<ScanResult> {
    if param.len() < 10 {
        return None;
    }

    let _frame_len = u16::from_le_bytes([param[0], param[1]]) as usize;  
    let center_freq = u16::from_le_bytes([param[4], param[5]]);  
    let rssi = param[9] as i8;  

    let payload = &param[10..];  
    if payload.len() < 24 {  
        return None; // 802.11 管理帧头至少 24 字节  
    }  

    // 802.11 管理帧头部:  
    //   [0..2]   Frame Control  
    //   [2..4]   Duration  
    //   [4..10]  DA  
    //   [10..16] SA  
    //   [16..22] BSSID  
    //   [22..24] Sequence Control  
    //   [24..]   Frame Body  
    let mut bssid = [0u8; 6];  
    bssid.copy_from_slice(&payload[16..22]);  

    // Beacon/ProbeResp Frame Body:  
    //   [0..8]   Timestamp (u64)  
    //   [8..10]  Beacon Interval (u16)  
    //   [10..12] Capability Info (u16)  
    //   [12..]   Information Elements  
    let body = &payload[24..];  
    if body.len() < 12 {  
        return None;  
    }  

    let beacon_interval = u16::from_le_bytes([body[8], body[9]]);  
    let capability = u16::from_le_bytes([body[10], body[11]]);  
  
    // 解析 IE 提取 SSID (IE ID = 0)  
    let mut ssid = [0u8; MAC_SSID_LEN];  
    let mut ssid_len = 0u8;  
    let mut ie_offset = 12;  
    while ie_offset + 2 <= body.len() {  
        let ie_id = body[ie_offset];  
        let ie_len = body[ie_offset + 1] as usize;  
        if ie_offset + 2 + ie_len > body.len() {  
            break;  
        }  
        if ie_id == 0 {  
            // SSID IE  
            let len = ie_len.min(MAC_SSID_LEN);  
            ssid[..len].copy_from_slice(&body[ie_offset + 2..ie_offset + 2 + len]);  
            ssid_len = len as u8;  
            break;  
        }  
        ie_offset += 2 + ie_len;  
    }  
  
    Some(ScanResult {  
        ssid,  
        ssid_len,  
        bssid,  
        center_freq,  
        rssi,  
        capability,  
        beacon_interval,  
        raw_payload: payload.to_vec(),  
    })  
}

/// 从 ind_queue 中等待指定 msg_id 的 indication  
/// 超时返回 Err(CmdError::Timeout)  
pub fn wait_for_indication(
    bus: &Arc<WifiBus>,
    target_msg_id: u16,
    timeout_ms: u64,
) -> Result<Vec<u8>, CmdError> {
    let deadline = axhal::time::monotonic_time_nanos() + timeout_ms as u64 * 1_000_000; 

    let result = block_on(poll_fn(|cx| {
        // 超时检查  
        if axhal::time::monotonic_time_nanos() >= deadline {  
            return Poll::Ready(Err(CmdError::Timeout));  
        }  

        // 在 ind_queue 中查找目标 msg_id  
        {
            let mut queue = bus.ind_queue.lock();
            for i in 0..queue.len() {
                if queue[i].len() >= LmacMsg::SIZE {
                    let msg = LmacMsg::from_le_bytes(&queue[i]);
                    if msg.id == target_msg_id {
                        let raw = queue.remove(i).unwrap();
                        let param_start = LmacMsg::SIZE;
                        let param = if raw.len() > param_start {
                            raw[param_start..].to_vec()
                        } else {
                            Vec::new()
                        };
                        return Poll::Ready(Ok(param));
                    }
                }
            }
        }

        // 注册 waker，等待 ind_pollset 通知  
        bus.ind_pollset.register(cx.waker());  

        // 注册后再检查一次（防止 race）
        {  
            let mut queue = bus.ind_queue.lock();  
            for i in 0..queue.len() {  
                if queue[i].len() >= LmacMsg::SIZE {  
                    let msg = LmacMsg::from_le_bytes(&queue[i]);  
                    if msg.id == target_msg_id {  
                        let raw = queue.remove(i).unwrap();  
                        let param_start = LmacMsg::SIZE;  
                        let param = if raw.len() > param_start {  
                            raw[param_start..].to_vec()  
                        } else {  
                            Vec::new()  
                        };  
                        return Poll::Ready(Ok(param));  
                    }  
                }  
            }  
        }

        // 保持活跃（与 rx_thread 相同策略）  
        cx.waker().wake_by_ref();  
        Poll::Pending  
    }));

    result
}

/// 发送 SM_CONNECT_REQ（连接到 AP） 
/// sm_connect_req 结构体布局（含 C padding）:  
///   [0..33]   mac_ssid ssid        (1+32)  
///   [33]      padding              (1 byte, align mac_addr to u16)  
///   [34..40]  mac_addr bssid       (6)  
///   [40..45]  mac_chan_def chan     (5)  
///   [45..48]  padding              (3 bytes, align u32 flags)  
///   [48..52]  u32 flags  
///   [52..54]  u16 ctrl_port_ethertype  
///   [54..56]  u16 ie_len  
///   [56..58]  u16 listen_interval  
///   [58]      bool dont_wait_bcmc  
///   [59]      u8 auth_type  
///   [60]      u8 uapsd_queues  
///   [61]      u8 vif_idx  
///   [62..64]  padding              (2 bytes, align u32 ie_buf)  
///   [64..320] u32 ie_buf[64]       (256)  
///   总计: 320 bytes  
///  
/// 返回 SM_CONNECT_CFM 的 param（1 字节: status）  
pub fn send_sm_connect_req(  
    bus: &Arc<WifiBus>,  
    vif_idx: u8,  
    ssid: &[u8],  
    bssid: &[u8; 6],  
    channel_freq: u16,  
    flags: u32,  
    auth_type: u8,  
    ie: &[u8],         // RSN IE 等附加 IE  
    timeout_ms: u64,  
) -> Result<Vec<u8>, CmdError> {  
    const SM_CONNECT_REQ_SIZE: usize = 320;  
  
    let mut param = vec![0u8; SM_CONNECT_REQ_SIZE]; 

    // ssid [0..33]  
    let ssid_len = ssid.len().min(MAC_SSID_LEN);  
    param[0] = ssid_len as u8;  
    param[1..1 + ssid_len].copy_from_slice(&ssid[..ssid_len]);  
  
    // bssid [34..40] (1 byte padding after ssid)  
    param[34..40].copy_from_slice(bssid);  

    // chan [40..45]  
    if channel_freq != 0 && channel_freq != 0xFFFF {  
        param[40..42].copy_from_slice(&channel_freq.to_le_bytes()); // freq  
        param[42] = 0; // band = 2.4GHz  
        param[43] = 0; // flags  
        param[44] = 30; // tx_power  
    } else {  
        // 不指定信道：freq = 0xFFFF  
        param[40..42].copy_from_slice(&0xFFFFu16.to_le_bytes());  
    }  

    // flags [48..52]  
    param[48..52].copy_from_slice(&flags.to_le_bytes());  
  
    // ctrl_port_ethertype [52..54] = ETH_P_PAE (0x888E) in network byte order  
    param[52..54].copy_from_slice(&ETH_P_PAE.to_be_bytes());  
  
    // ie_len [54..56]  
    let ie_len = ie.len().min(256);  
    param[54..56].copy_from_slice(&(ie_len as u16).to_le_bytes());  
  
    // listen_interval [56..58] = 1  
    param[56..58].copy_from_slice(&1u16.to_le_bytes());  
  
    // dont_wait_bcmc [58] = 0 (wait for BC/MC)  
    param[58] = 0;  
  
    // auth_type [59]  
    param[59] = auth_type;  
  
    // uapsd_queues [60] = 0  
    param[60] = 0;  
  
    // vif_idx [61]  
    param[61] = vif_idx;  
  
    // ie_buf [64..64+ie_len]  
    if ie_len > 0 {  
        param[64..64 + ie_len].copy_from_slice(&ie[..ie_len]);  
    }  

    log::info!(  
        "[cmd_mgr] sending SM_CONNECT_REQ: vif={}, ssid_len={}, auth={}, flags=0x{:08x}, ie_len={}",  
        vif_idx, ssid_len, auth_type, flags, ie_len  
    );  
  
    // Linux 驱动等待 SM_CONNECT_CFM (msg_id + 1)  
    send_cmd(bus, SM_CONNECT_REQ, msg_t(TASK_SM, 0), &param, timeout_ms)     
}

/// 发送 SM_DISCONNECT_REQ  
/// 对应 Linux: rwnx_send_sm_disconnect_req (rwnx_msg_tx.c:3239-3258)  
///  
/// sm_disconnect_req:  
///   u16 reason_code;  [0..2]  
///   u8  vif_idx;      [2]  
///   总计: 3 bytes  
pub fn send_sm_disconnect_req(  
    bus: &Arc<WifiBus>,  
    vif_idx: u8,  
    reason_code: u16,  
    timeout_ms: u64,  
) -> Result<Vec<u8>, CmdError> {  
    let mut param = [0u8; 3];  
    param[0..2].copy_from_slice(&reason_code.to_le_bytes());  
    param[2] = vif_idx;  

    log::info!(  
        "[cmd_mgr] sending SM_DISCONNECT_REQ: vif={}, reason={}",  
        vif_idx, reason_code  
    );  
  
    send_cmd(bus, SM_DISCONNECT_REQ, msg_t(TASK_SM, 0), &param, timeout_ms)  
}

/// 发送 MM_KEY_ADD_REQ（安装加密密钥）  
/// 对应 Linux: rwnx_send_key_add (rwnx_msg_tx.c:641-679)  
///  
/// mm_key_add_req 结构体:  
///   u8  key_idx;       [0]  
///   u8  sta_idx;       [1]  
///   mac_sec_key key;   [2..35]  (u8 length + u32 array[8] → 1+3padding+32=36? 或 1+32=33?)  
///   u8  cipher_suite;  
///   u8  inst_nbr;  
///   u8  spp;  
///   bool pairwise;  
///  
/// mac_sec_key: { u8 length; u32 array[8]; }  
///   C layout: length at [0], padding [1..4], array at [4..36] → 总 36 bytes  
///   或者 length at [0], array at [4..36] (u32 alignment) → 总 36 bytes  
///  
/// mm_key_add_req 总大小:  
///   key_idx(1) + sta_idx(1) + padding(2) + mac_sec_key(36) + cipher(1) + inst_nbr(1) + spp(1) + pairwise(1)  
///   = 44 bytes  
pub fn send_key_add_req(  
    bus: &Arc<WifiBus>,  
    vif_idx: u8,  
    sta_idx: u8,  
    pairwise: bool,  
    key: &[u8],  
    key_idx: u8,  
    cipher_suite: u8,  
    timeout_ms: u64,  
) -> Result<u8, CmdError> { 
    const MM_KEY_ADD_REQ_SIZE: usize = 44;  
  
    let mut param = [0u8; MM_KEY_ADD_REQ_SIZE];  
  
    // key_idx [0]  
    param[0] = key_idx;  
    // sta_idx [1]  
    param[1] = sta_idx;  
    // padding [2..4]  

    // mac_sec_key [4..40]:  
    //   length [4]  
    //   padding [5..8]  
    //   array [8..40] (u32 array[8], 32 bytes)  
    let key_len = key.len().min(MAC_SEC_KEY_LEN);  
    param[4] = key_len as u8; 
    // 密钥数据写入 array 字段（offset 8）  
    // 注意：Linux 驱动用 memcpy(&key.array[0], key, key_len)  
    // array 是 u32[]，但 memcpy 按字节拷贝，所以直接拷贝即可  
    param[8..8 + key_len].copy_from_slice(&key[..key_len]);  

    // cipher_suite [40]  
    param[40] = cipher_suite;  
    // inst_nbr [41]  
    param[41] = vif_idx;  
    // spp [42]  
    param[42] = 0;  
    // pairwise [43]  
    param[43] = if pairwise { 1 } else { 0 };  
  
    log::info!(  
        "[cmd_mgr] sending MM_KEY_ADD_REQ: sta={}, key_idx={}, cipher={}, pairwise={}, key_len={}",  
        sta_idx, key_idx, cipher_suite, pairwise, key_len  
    );  

    let rsp = send_cmd(bus, MM_KEY_ADD_REQ, TASK_MM, &param, timeout_ms)?;

    // mm_key_add_cfm: status(u8) + hw_key_idx(u8) + aligned[2]  
    if rsp.len() >= 2 {
        let status = rsp[0];
        let hw_key_idx = rsp[1];
        if status != 0 {  
            log::error!("[cmd_mgr] MM_KEY_ADD_CFM status={} (error)", status);  
            return Err(CmdError::FirmwareError);  
        }  
        log::info!("[cmd_mgr] MM_KEY_ADD_CFM OK: hw_key_idx={}", hw_key_idx);  
        Ok(hw_key_idx)   
    } else {
        log::error!("[cmd_mgr] MM_KEY_ADD_CFM too short: {} bytes", rsp.len());  
        Err(CmdError::InvalidResponse)  
    }
}

/// 发送 MM_KEY_DEL_REQ  
/// 对应 Linux: rwnx_send_key_del (rwnx_msg_tx.c:682-698)  
///  
/// mm_key_del_req: { u8 hw_key_idx; } → 1 byte  
pub fn send_key_del_req(  
    bus: &Arc<WifiBus>,  
    hw_key_idx: u8,  
    timeout_ms: u64,  
) -> Result<Vec<u8>, CmdError> {
    let param = [hw_key_idx];  
  
    log::info!("[cmd_mgr] sending MM_KEY_DEL_REQ: hw_key_idx={}", hw_key_idx);  
  
    send_cmd(bus, MM_KEY_DEL_REQ, TASK_MM, &param, timeout_ms)  
}

/// 发送 ME_SET_CONTROL_PORT_REQ  
/// 对应 Linux: rwnx_send_me_set_control_port_req (rwnx_msg_tx.c:2779-2797)  
///  
/// me_set_control_port_req:  
///   u8   sta_idx;            [0]  
///   bool control_port_open;  [1]  
///   总计: 2 bytes  
pub fn send_set_control_port_req(  
    bus: &Arc<WifiBus>,  
    sta_idx: u8,  
    open: bool,  
    timeout_ms: u64,  
) -> Result<Vec<u8>, CmdError> {  
    let param = [sta_idx, if open { 1 } else { 0 }];  
  
    log::info!(  
        "[cmd_mgr] sending ME_SET_CONTROL_PORT_REQ: sta_idx={}, open={}",  
        sta_idx, open  
    );  
  
    send_cmd(bus, ME_SET_CONTROL_PORT_REQ, TASK_ME, &param, timeout_ms)  
}