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
    // {
    //     // wireless 完整初始化：aicbsp_init → aicbsp_set_subsys(Wifi, On) → FDRV platform_init
    //     let mut bsp_info = wireless::bsp::AicBspInfo::default();
    //     wireless::bsp::aicbsp_init(&mut bsp_info, wireless::bsp::AicBspCpMode::Work).expect("aicbsp_init failed");
    //     wireless::bsp::aicbsp_set_subsys(wireless::bsp::AicBspSubsys::Wifi, wireless::bsp::AicBspPwrState::On)
    //         .expect("aicbsp_set_subsys Wifi On failed");
    //     // 验证完整流程：上电 → SDIO 初始化 → 时钟/4-bit 切换 → 最小 IPC (DBG_MEM_READ) 并等待 CFM
    //     // 这将验证 MEM_WRITE_CFM 超时是否已通过 寄存器顺序、DMA 边界、4-bit 模式及 RD_FIFO 排空 解决。
    //     // wireless::bsp::dma_write_minimal_verify().expect("dma_write_minimal_verify failed");
    // }

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
fn sdio1_probe() {
    use sdhci_cv1800::{CviSdhci, hw_init};  
    use aic8800_sdio::SdioHost;  
    use aic8800_fw::{chip_id::ChipVariant, firmware_init};  
    use alloc::sync::Arc;
    use axsync::Mutex;
    use aic8800_fdrv::bus::{BusState, WifiBus};
  
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
                            // 阶段 4 验证: 发送 MM_VERSION_REQ  
                            match aic8800_fdrv::cmd_mgr::send_cmd(  
                                &bus, 0x0004, 0x0000, &[], 6000  
                            ) {  
                                Ok(rsp) => info!("[VERIFY-4] MM_VERSION_CFM OK, len={}", rsp.len()),  
                                Err(e) => error!("[VERIFY-4] MM_VERSION_REQ FAILED: {:?}", e),  
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