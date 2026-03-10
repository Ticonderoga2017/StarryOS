//! 固件上传核心逻辑 

use aic8800_sdio::{SdioHost, error::SdioError};  
use crate::fw_data::FirmwareSet;
use crate::chip_id::*;  
use crate::ipc_msg::{IpcTransport, ipc_mem_block_write, ipc_start_app};  

/// 将固件二进制数据上传到芯片 RAM  
pub fn upload_firmware<H: SdioHost>(
    transport: &mut IpcTransport<H>, 
    fw_data: &[u8], 
    fw_addr: u32
) -> Result<(), SdioError> {  
    let size = fw_data.len();
    log::info!("[aic8800] Uploading firmware: addr=0x{:08x}, size={} bytes", fw_addr, size);  
    if size == 0 {
        log::warn!("[aic8800] Firmware data is empty, skipping upload");
        return Err(SdioError::Unsupported);
    }

    let mut offset = 0;  
    // 上传完整的 1024 字节块  
    while offset + FW_UPLOAD_CHUNK_SIZE <= size {
        ipc_mem_block_write(transport, fw_addr.wrapping_add(offset as u32), &fw_data[offset..offset + FW_UPLOAD_CHUNK_SIZE])?;
        offset += FW_UPLOAD_CHUNK_SIZE;

        // 每 64KB 打印进度 
        if offset % 65536 == 0 {
            log::info!("[aic8800]   progress: {}/{} ({:.1}%)", offset, size, (offset as f64 / size as f64) * 100.0);
        }
    }
    // 上传剩余的不足 1024 字节的部分
    if offset < size {
        ipc_mem_block_write(transport, fw_addr + offset as u32, &fw_data[offset..])?;        
    }
    log::info!("[aic8800] Firmware upload complete ({} bytes)", size); 
    Ok(())
}

/// AIC8801 固件初始化流程  
pub fn init_aic8801_firmware<H: SdioHost>(
    transport: &mut IpcTransport<H>, 
    fw_set: &FirmwareSet
) -> Result<(), SdioError> {  
    log::info!("[aic8800] === AIC8801 init start ===");  

    // 1. 上传 FMAC 固件到 RAM_FMAC_FW_ADDR  
    upload_firmware(transport, fw_set.wl_fw, RAM_FMAC_FW_ADDR)?;  

    // 2. 上传补丁固件 (如果有)  
    if !fw_set.wl_patch.is_empty() {
        upload_firmware(transport, fw_set.wl_patch, RAM_FMAC_FW_PATCH_ADDR)?;
    } else {
        log::warn!("[aic8800] No patch firmware provided, skipping patch upload");
    }

    // TODO: aicwifi_patch_config  (LDPC/AGC/TxGain/JumpTable)  
    // TODO: aicwifi_sys_config    (RF PLL 配置)  

    // 3. (MVP 阶段省略) aicwifi_patch_config — 补丁表写入  
    // TODO: patch_config_aic8801(transport)?;  
  
    // 4. (MVP 阶段省略) aicwifi_sys_config — 系统配置  
    // TODO: sys_config_aic8801(transport)?;  

    // 5. 启动固件(start_from_bootrom) 
    let status = ipc_start_app(
        transport, 
        RAM_FMAC_FW_ADDR, // bootaddr = 0x00120000
        HOST_START_APP_AUTO, // boottype = 1  
    )?;
    log::info!("[aic8800] start_app status = 0x{:08x}", status);  
  
    log::info!("[aic8800] === AIC8801 init done ===");  
    Ok(())
}

/// AIC8800DC 固件初始化流程  
pub fn init_aic8800dc_firmware<H: SdioHost>(
    transport: &mut IpcTransport<H>, 
    fw_set: &FirmwareSet,
    chip_rev: &ChipRevision,
) -> Result<(), SdioError> {  
    log::info!("[aic8800] === AIC8800DC init start ===");  

    // TODO Phase 1b: system_config_8800dc(transport)?;  
    //   配置 PMIC 电压, BBPLL, 时钟等  
    //   需要实现 DBG_MEM_MASK_WRITE_REQ  

    // U01: 上传完整固件到 RAM; U02+: 补丁上传到 ROM_FMAC_PATCH_ADDR  
    let upload_addr = if chip_rev.rev == CHIP_REV_U01 {
        RAM_FMAC_FW_ADDR
    } else {
        ROM_FMAC_FW_ADDR
    };
    upload_firmware(transport, fw_set.wl_fw, upload_addr)?;

    // U01 还需要上传补丁  
    if chip_rev.rev == CHIP_REV_U01 && !fw_set.wl_patch.is_empty() {  
        upload_firmware(transport, fw_set.wl_patch, ROM_FMAC_PATCH_ADDR)?;  
    }

    // TODO: aicwf_patch_config_8800dc (LDPC/AGC/TxGain/JumpTable)  

    // // 1. 上传补丁固件到 ROM_FMAC_PATCH_ADDR  
    // if !fw_set.wl_patch.is_empty() {  
    //     upload_firmware(transport, fw_set.wl_patch, ROM_FMAC_PATCH_ADDR)?;  
    // } else {  
    //     log::warn!("[aic8800] No patch firmware for DC, skipping patch upload");  
    // }  

    // TODO Phase 1c: aicwf_patch_config_8800dc(transport, chip_rev)?;  
    //   LDPC/AGC/TxGain/跳转表 配置  

    // // 1. 读取芯片版本     
    // let (chip_rev, _raw) = read_chip_revision(transport)?;  
    // log::info!("[aic8800] Chip revision = {}", chip_rev); 

    // // 2. (MVP 阶段省略) system_config_8800dc — 写系统配置寄存器  
    // // TODO: system_config_8800dc(transport)?; 

    // // 3. 上传补丁固件
    // if !fw_patch_data.is_empty() {
    //     upload_firmware(transport, fw_patch_data, ROM_FMAC_PATCH_ADDR)?;
    // } else {
    //     log::warn!("[aic8800] No patch firmware provided, skipping patch upload");
    // }

    // 4. (MVP 阶段省略) aicwf_patch_config_8800dc — LDPC/AGC/TxGain/JumpTable 配置  
    // TODO: patch_config_8800dc(transport, chip_rev)?;  
  
    // 5. 启动固件  
    let boot_addr = if chip_rev.rev == CHIP_REV_U01 {
        RAM_FMAC_FW_ADDR  
    } else {  
        ROM_FMAC_FW_ADDR  
    }; 
    let status = ipc_start_app(  
        transport,  
        boot_addr,      
        HOST_START_APP_DUMMY,    // boottype = 5 (AIC8800DC normal mode)  
    )?; 
    log::info!("[aic8800] start_app status = 0x{:08x}", status);     

    log::info!("[aic8800] === AIC8800DC init done ===");  
    Ok(())
}

/// AIC8800D80 固件初始化流程 
pub fn init_aic8800d80_firmware<H: SdioHost>(
    transport: &mut IpcTransport<H>, 
    fw_set: &FirmwareSet, 
) -> Result<(), SdioError> {  
    log::info!("[aic8800] === AIC8800D80 init start ===");  

    // 1. 上传 FMAC 固件到 RAM_FMAC_FW_ADDR  
    upload_firmware(transport, fw_set.wl_fw, RAM_FMAC_FW_ADDR)?;  

    // TODO Phase 1b: aicbsp_system_config_8800d80(transport)?;  
    // TODO Phase 1c: aicwifi_patch_config_8800d80(transport)?;  
    // TODO Phase 1c: aicwifi_sys_config_8800d80(transport)?; 
    
    // 2. (MVP 阶段省略) aicwifi_patch_config_8800d80  
    // TODO: patch_config_8800d80(transport)?;  
  
    // 3. (MVP 阶段省略) aicwifi_sys_config_8800d80  
    // TODO: sys_config_8800d80(transport)?;  

    // 4. 启动固件(start_from_bootrom) 
    let status = ipc_start_app(  
        transport,  
        RAM_FMAC_FW_ADDR,       // bootaddr = 0x00120000  
        HOST_START_APP_AUTO,     // boottype = 1  
    )?;  
    log::info!("[aic8800] start_app status = 0x{:08x}", status);  
  
    log::info!("[aic8800] === AIC8800D80 init done ===");  
    Ok(())
}