#![no_std]
#![no_main]
#![doc = include_str!("../README.md")]

#[macro_use]
extern crate axlog;

extern crate alloc;
extern crate axruntime;

use alloc::{borrow::ToOwned, vec::Vec};

use axfs::FS_CONTEXT;

mod entry;

pub const CMDLINE: &[&str] = &["/bin/sh", "-c", include_str!("init.sh")];

#[unsafe(no_mangle)]
fn main() {
    starry_api::init();

    #[cfg(feature = "sg2002")]
    sdio1_probe();

    let args = CMDLINE
        .iter()
        .copied()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let envs = [];
    let exit_code = entry::run_initproc(&args, &envs);
    info!("Init process exited with code: {exit_code:?}");

    let cx = FS_CONTEXT.lock();
    cx.root_dir()
        .unmount_all()
        .expect("Failed to unmount all filesystems");
    cx.root_dir()
        .filesystem()
        .flush()
        .expect("Failed to flush rootfs");
}

#[cfg(feature = "vf2")]
extern crate axplat_riscv64_visionfive2;

#[cfg(feature = "sg2002")]
extern crate axplat_riscv64_sg2002;
#[cfg(feature = "sg2002")]
use sdhci_cv1800::{CviSdhci, hw_init}; 
#[cfg(feature = "sg2002")] 
use aic8800_sdio::SdioHost;  
#[cfg(feature = "sg2002")]
use aic8800_fw::{chip_id::ChipVariant, firmware_init};  
#[cfg(feature = "sg2002")]
use alloc::sync::Arc;
#[cfg(feature = "sg2002")]
use axsync::Mutex;
#[cfg(feature = "sg2002")]
use aic8800_fdrv::{bus::{BusState, WifiBus}, cmd_mgr::*, wifi_mgr::*, wpa2::*, lmac_msg::CmdError};
#[cfg(feature = "sg2002")]
fn sdio1_probe() {   
    // 修正: SD1 主系统总线地址 (非 RTC 域)  
    // 内存映射: 0x04320000 - 0x0432FFFF = SD1  
    // Linux DTS: wifi-sd@4320000  
    const SDIO1_PADDR: usize = 0x0432_0000;  
    const SDIO1_VADDR: usize = SDIO1_PADDR + 0xFFFF_FFC0_0000_0000; 
  
    info!("========== SDIO1 Probe Start ==========");  
  
    let mut sdio1 = CviSdhci::new(SDIO1_VADDR);  
  
    match sdio1.init() {  
        Ok(()) => {  
            let (vid, did) = sdio1.vendor_device_id();  
            info!("SDIO1 probe OK: vendor=0x{:04x}, device=0x{:04x}", vid, did);  
            
            let chip = ChipVariant::from_vid_did(vid, did);  
            info!("Detected chip: {:?}", chip);  

            if chip == ChipVariant::Unknown {  
                warn!("Unknown AIC chip, skip firmware init");  
                return;  
            }  
  
            match aic8800_fw::firmware_init(&mut sdio1, chip) {  
                Ok(()) => {
                    info!("AIC8800 firmware init SUCCESS");
                    // ---- FDRV 初始化 ----  
                    match aic8800_fdrv::init(sdio1) {  
                        Ok(bus) => {  
                            info!("AIC8800 FDRV init SUCCESS");  
                            if let Err(e) = wifi_main(&bus) {  
                                // 测试固件是否还活着  
                                match send_cmd(&bus, 0x0004, 0x0000, &[], 3000) {  // MM_VERSION_REQ  
                                    Ok(rsp) => info!("[probe] firmware alive, MM_VERSION_CFM len={}", rsp.len()),  
                                    Err(_) => error!("[probe] firmware DEAD - no response to MM_VERSION_REQ"),  
                                }

                                // dump 所有队列  
                                let rsp_q = bus.cmd_rsp_queue.lock();  
                                info!("[debug] cmd_rsp_queue len: {}", rsp_q.len());  
                                drop(rsp_q);  
                                let data_q = bus.data_rx_queue.lock();  
                                info!("[debug] data_rx_queue len: {}", data_q.len());  
                                drop(data_q);  

                                error!("[wifi] FAILED: {:?}", e);  
                            }                            

                            bus.dump_status(); // 打印完整状态  
                            core::mem::forget(bus); // 临时 leak                              
                        }  
                        Err(e) => error!("FDRV init FAILED: {}", e),  
                    }  
                } 
                Err(e) => error!("AIC8800 firmware init FAILED: {:?}", e),  
            } 
        }  
        Err(e) => {  
            error!("SDIO1 init failed: {:?}", e);  
            error!(">>> Check clock/reset/pinmux — dumping registers <<<");  
            hw_init::sdio1_hw_dump();  
        }  
    }  
  
    info!("========== SDIO1 Probe End =========="); 
}

#[cfg(feature = "sg2002")]
fn wifi_main(bus: &Arc<WifiBus>) -> Result<(), CmdError> {  
    // ========== Phase 1: LMAC Configuration ==========  
    info!("========== LMAC Config Start ==========");  
  
    // MM_VERSION_REQ  
    let rsp = send_cmd(bus, 0x0004, 0x0000, &[], 6000)?;  
    info!("[VERIFY] MM_VERSION_CFM OK, len={}", rsp.len());  
  
    // TX Power Index  
    send_txpwr_idx_req(bus, 6000)?;  
    info!("[LMAC] txpwr_idx OK");  
  
    // TX Power Offset  
    send_txpwr_ofst_req(bus, 6000)?;  
    info!("[LMAC] txpwr_ofst OK");  
  
    // RF Calibration  
    send_rf_calib_req(bus, 10000)?;  
    info!("[LMAC] rf_calib OK");  
  
    // ME Config  
    send_me_config_req(bus, 6000)?;  
    info!("[LMAC] me_config OK");  
  
    // ME Channel Config  
    send_me_chan_config_req(bus, 6000)?;  
    info!("[LMAC] me_chan_config OK");  
  
    // MM_START  
    send_mm_start_req(bus, 6000)?;  
    info!("[LMAC] mm_start OK");  
  
    // 获取 MAC 地址（需要在 cmd_mgr.rs 中实现 send_get_mac_addr_req）  
    let sta_mac = send_get_mac_addr_req(bus, 5000)?;  
    info!(  
        "[LMAC] MAC addr: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",  
        sta_mac[0], sta_mac[1], sta_mac[2],  
        sta_mac[3], sta_mac[4], sta_mac[5]  
    );  
  
    // 检查 MAC 是否全零，fallback 到默认值  
    let sta_mac = if sta_mac == [0u8; 6] {  
        warn!("[LMAC] MAC is all zeros, using default");  
        [0x00, 0x6F, 0x6F, 0x6F, 0x6F, 0x00]  
    } else {  
        sta_mac  
    };  
  
    // MM_ADD_IF（使用真实 MAC）  
    let vif_idx = send_mm_add_if_req(bus, &sta_mac, 6000)?;  
    info!("[LMAC] add_if OK: vif_index={}", vif_idx);  
  
    info!("========== LMAC Config End ==========");  
  
    // ========== Crypto Self-Test ==========  
    info!("========== Crypto Self-Test ==========");  
    let crypto_ok = aic8800_fdrv::wpa2::run_crypto_self_test();  
    if !crypto_ok {  
        error!("[FATAL] Crypto self-test FAILED! Handshake will not work.");  
        return Err(CmdError::FirmwareError);  
    }  
    info!("========== Crypto Self-Test PASSED ==========");
    aic8800_fdrv::wpa2::run_ptk_test();

    // ========== Phase 2: Scan ==========  
    let results = scan(bus, vif_idx, None, 20000)?;  
    info!("[SCAN] Found {} APs", results.len());  
    for (i, ap) in results.iter().enumerate() {  
        let ssid = core::str::from_utf8(&ap.ssid[..ap.ssid_len as usize])  
            .unwrap_or("<non-utf8>");  
        info!("  [{}] \"{}\" rssi={} freq={}", i, ssid, ap.rssi, ap.center_freq);  
    }  
  
    // ========== Phase 3: Connect ==========  
    let target_ssid = b"CU_Q2aa";  
    let password = b"uuux5cfj";  
  
    let ap = find_ap_by_ssid(&results, target_ssid)  
        .ok_or(CmdError::Timeout)?;  // 用 Timeout 代替，或添加新的 CmdError 变体  
    info!(  
        "[CONNECT] Target AP: freq={}, bssid={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",  
        ap.center_freq,  
        ap.bssid[0], ap.bssid[1], ap.bssid[2],  
        ap.bssid[3], ap.bssid[4], ap.bssid[5]  
    );  
  
    let rsn_ie = if !ap.rsn_ie.is_empty() {  
        info!("[CONNECT] AP RSN IE: {:02x?}", &ap.rsn_ie);  
        let sta_rsn = build_wpa2_rsn_ie_from_ap(&ap.rsn_ie);  
        info!("[CONNECT] STA RSN IE: {:02x?}", &sta_rsn);  
        sta_rsn  
    } else if ap.capability & 0x0010 != 0 {  
        // Privacy 位已设置但未捕获到 RSN IE —— 构建默认 WPA2-PSK RSN IE  
        info!("[CONNECT] Privacy bit set but no RSN IE, using default WPA2-PSK RSN IE");  
        build_wpa2_rsn_ie() 
    } else {  
        info!("[CONNECT] Open network (no RSN IE, no Privacy bit)");  
        Vec::new()  
    };
    
    let mut handshake = Wpa2Handshake::new(    
        password,    
        target_ssid,    
        &ap.bssid,     // AA (Authenticator Address)    
        &sta_mac,      // SPA (Supplicant Address)    
        // &handshake_rsn_ie,  // 使用固件实际发送的 RSN IE  
        &rsn_ie,
    );
    info!("[WPA2] PMK ready, now connecting...");  

    let connect_result = connect(  
        bus, vif_idx, target_ssid, &ap.bssid,  
        ap.center_freq, &rsn_ie, 15000, 
    )?;  
    info!("[CONNECT] SM_CONNECT_IND: ap_idx={}", connect_result.ap_idx);  
  
    // ========== Phase 4: WPA2 Handshake ==========  
  
    loop {  
        let eapol = wait_for_eapol(bus, 10000)?;  
        info!("[WPA2] Received EAPOL frame: {} bytes", eapol.len());  
  
        match handshake.process_eapol(&eapol) {  
            Ok(HandshakeAction::SendM2(m2)) => {  
                info!("[WPA2] Sending M2: {} bytes", m2.len());  
                send_eapol_data_frame(bus, &ap.bssid, &sta_mac, &m2, vif_idx, connect_result.ap_idx)?;  
            }  
            Ok(HandshakeAction::Completed(result)) => {  
                // 发送 M4  
                info!("[WPA2] Sending M4: {} bytes", result.m4_frame.len());  
                send_eapol_data_frame(bus, &ap.bssid, &sta_mac, &result.m4_frame, vif_idx, connect_result.ap_idx)?;  
  
                // 安装 PTK  
                send_key_add_req(  
                        bus,  
                        vif_idx,                    // vif_idx: u8  
                        connect_result.ap_idx,      // sta_idx: u8  
                        true,                       // pairwise: bool  
                        &result.tk,                 // key: &[u8]  
                        0,                          // key_idx: u8  
                        3,                          // cipher_suite: u8 (MAC_CIPHER_CCMP)  
                        5000,                       // timeout_ms: u64 
                )?;  
                info!("[WPA2] PTK installed");  
  
                // 安装 GTK  
                send_key_add_req(  
                    bus,  
                    vif_idx,                    // vif_idx: u8  
                    0xFF,                       // sta_idx: u8 (0xFF = group key)  
                    false,                      // pairwise: bool  
                    &result.gtk,                // key: &[u8]  
                    result.gtk_key_idx,         // key_idx: u8  
                    3,                          // cipher_suite: u8 (MAC_CIPHER_CCMP)  
                    5000,                       // timeout_ms: u64  
                )?;  
                info!("[WPA2] GTK installed");  
  
                // 打开控制端口  
                send_set_control_port_req(  
                    bus, connect_result.ap_idx, true, 5000,  
                )?;  
                info!("[WPA2] Control port opened, connected!");  
                break;  
            }  
            Err(e) => { 
                error!("[WPA2] Handshake error: {:?}", e);  
                break;  
            }  
        }  
    }  
  
    Ok(())  
}