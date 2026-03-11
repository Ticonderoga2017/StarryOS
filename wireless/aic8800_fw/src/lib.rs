#![no_std]

extern crate alloc;  

pub mod fw_data;
pub mod fw_upload;
pub mod ipc_msg;
pub mod chip_id;

use aic8800_sdio::{SdioHost, error::SdioError};  
use chip_id::*;
use ipc_msg::{IpcTransport, ipc_mem_read};
use fw_upload::{init_aic8801_firmware, init_aic8800dc_firmware, init_aic8800d80_firmware};
use fw_data::{FirmwareSet, get_firmware_set};

use crate::ipc_msg::{ipc_mem_write, mm_get_fw_version, mm_get_mac_addr, mm_get_version, mm_reset, mm_set_stack_start};
/// SDIO 功能寄存器初始化
/// 
/// `is_v3`: true = AIC8800D80/D80X2, false = AIC8801/DC/DW 
pub fn sdio_func_setup<H: SdioHost>(host: &mut H, is_v3: bool) -> Result<(), SdioError> {  
    if !is_v3 {
        // ---- AIC8801 / AIC8800DC / AIC8800DW ----  
  
        // 使能块模式 (block_bit0 = 0x1)  
        host.write_byte(1, SDIOWIFI_REGISTER_BLOCK, 0x01)?;

        // 禁用字节模式 (byte_mode_disable = 0x1, 即 "no byte mode")  
        host.write_byte(1, SDIOWIFI_BYTEMODE_ENABLE_REG, 0x01)?;  

        // 延时等待芯片内部状态稳定 (~10ms)  
        // 在 no_std 无精确 sleep 的环境下, 粗略等待  
        for _ in 0..500_000 {  
            core::hint::spin_loop();  
        }  
    } else {
        // ---- AIC8800D80 / AIC8800D80X2 (SDIO v3) ----  
  
        // 禁用字节模式  
        host.write_byte(1, SDIOWIFI_BYTEMODE_ENABLE_REG_V3, 0x01)?; 

        // 唤醒芯片
        host.write_byte(1, SDIOWIFI_WAKEUP_REG_V3, 0x01)?;

        // 等待 ~5ms  
        for _ in 0..250_000 {  
            core::hint::spin_loop();  
        }  

        // 检查唤醒状态 
        let sleep_val = host.read_byte(1, SDIOWIFI_SLEEP_REG_V3)?;  
        if sleep_val & 0x10 == 0 {  
            log::error!("[aic8800] V3 wakeup failed, sleep_reg=0x{:02x}", sleep_val);  
            return Err(SdioError::Timeout);  
        }  
        log::info!("[aic8800] V3 SDIO ready (sleep_reg=0x{:02x})", sleep_val);  
    }
    
    log::info!("[aic8800] SDIO func setup done (v3={})", is_v3);
    Ok(())
}

/// 从芯片读取版本信息 
fn read_chip_revision<H: SdioHost>(
    transport: &mut IpcTransport<H>,
     chip: ChipVariant,
    ) -> Result<ChipRevision, SdioError> {  
    let raw = ipc_mem_read(transport, CHIP_REV_ADDR)?;
    log::info!("[aic8800] chip info raw = 0x{:08x}", raw); 

    let (rev, is_chip_id_h) = match chip {
        ChipVariant::Aic8801 => {
            // AIC8801: 直接取高 16 位的低 8 位  
            let rev = (raw >> 16) as u8;  
            (rev, false)  
        }
        ChipVariant::Aic8800DC | ChipVariant::Aic8800DW | ChipVariant::Aic8800D80=> {
            // AIC8800DC: 低 6 位为版本号, 高 2 位为 chip_id_h 标志  
            let rev = ((raw >> 16) & 0x3F) as u8;  
            let is_h = ((raw >> 16) & 0xC0) == 0xC0;  
            (rev, is_h)  
        }
        ChipVariant::Aic8800D80X2 => {  
            let rev = ((raw >> 16) & 0x3F) as u8;  
            (rev, false)  
        }  
        ChipVariant::Unknown => return Err(SdioError::Unsupported),  
    };
    log::info!("[aic8800] chip_rev={}, is_chip_id_h={}", rev, is_chip_id_h);  
    Ok(ChipRevision { rev, is_chip_id_h })  
}

/// 验证芯片版本是否受支持  
fn validate_chip_revision(chip: ChipVariant, rev: &ChipRevision) -> Result<(), SdioError> {  
    let supported = match chip {
        ChipVariant::Aic8801 => {
            // AIC8801: 支持 U02(3), U03(7), U04(7)  
            // 由于 U03 == U04 == 7, 只需检查 U02 和 U03  
            rev.rev == CHIP_REV_U02 || rev.rev == CHIP_REV_U03 || rev.rev == CHIP_REV_U04
        }
        ChipVariant::Aic8800DC | ChipVariant::Aic8800DW | ChipVariant::Aic8800D80=> {
            // AIC8800DC/DW/D80: 支持 U01(1), U02(3), U03(7), U04(7)  
            rev.rev == CHIP_REV_U01 || rev.rev == CHIP_REV_U02 || rev.rev == CHIP_REV_U03 // CHIP_REV_U04 == CHIP_REV_U03 == 7, 已隐式覆盖
        }
        ChipVariant::Aic8800D80X2 => {
            // AIC8800D80X2: 需要 >= CHIP_REV_U04 + 8 = 15 
            rev.rev >= CHIP_REV_U04 + 8
        } 
        ChipVariant::Unknown => false,
    };
    if !supported {  
        log::error!(  
            "[aic8800] Unsupported chip revision: chip={:?}, rev={}",  
            chip,  
            rev.rev  
        );  
        return Err(SdioError::Unsupported);  
    } 

    log::info!("[aic8800] Chip revision validated: {:?}, rev={}", chip, rev.rev);  
    Ok(())  
}

/// BSP 系统配置 — 在固件上传前调用
///
/// 写入 10 个关键寄存器:
///   - 时钟/PLL 配置 (0x40500014, 0x40500018, 0x40500004)
///   - panic 修复 (0x40040000)
///   - BBPLL 配置 (0x40040084, 0x40040080, 0x40100058)
///   - PMIC 接口初始化 (0x50000000)
///   - 26MHz 晶振分频 (0x50019150)
///   - ★ 停止看门狗 (0x50017008) — 不停止会导致芯片在 ~1s 后复位
fn aicbsp_system_config<H: SdioHost> (ipc: &mut IpcTransport<H>) -> Result<(), SdioError> {
    for &(addr, data) in SYSCFG_TBL {  
        ipc_mem_write(ipc, addr, data)?;  
    }  
    log::info!("[aic8800] aicbsp_system_config done (watchdog stopped)");
    Ok(())  
}

/// 完整的固件初始化入口  
///  
/// fw_data: 固件二进制数据 (fmacfw.bin 或 fw_patch.bin)  
pub fn firmware_init<H: SdioHost>(
    host: &mut H, 
    chip: ChipVariant,
) -> Result<(), SdioError> {  
    log::info!("[aic8800] firmware_init: chip={:?}", chip);
    
    // 1. SDIO 功能寄存器初始化 (区分 v3 芯片)  
    let is_v3 = matches!(chip, ChipVariant::Aic8800D80 | ChipVariant::Aic8800D80X2);
    sdio_func_setup(host, is_v3)?;

    // 2. 创建 IPC 传输层  
    let mut ipc = IpcTransport::new(host, chip);

    // 3. 读取芯片版本信息
    let chip_rev = read_chip_revision(&mut ipc, chip)?;

    // 4. 验证芯片版本是否受支持
    validate_chip_revision(chip, &chip_rev)?;

    // 4.5 BSP 系统配置 (停止看门狗, 配置 PMIC/时钟) — 必须在固件上传前执行
    if matches!(chip, ChipVariant::Aic8801) {
        aicbsp_system_config(&mut ipc)?;
    }

    // 5. 选择固件 
    let fw_set = get_firmware_set(chip, &chip_rev).unwrap();
    log::info!(  
        "[aic8800] Selected firmware: {} (fw={} bytes, patch={} bytes)",  
        fw_set.desc,  
        fw_set.wl_fw.len(),  
        fw_set.wl_patch.len(),  
    ); 

    if fw_set.wl_fw.is_empty() {  
        log::error!("[aic8800] WiFi firmware data is empty");  
        return Err(SdioError::Unsupported);  
    }

    // 6. 根据芯片类型执行固件初始化  
    match chip {
        ChipVariant::Aic8801 => init_aic8801_firmware(&mut ipc, &fw_set)?,
        ChipVariant::Aic8800DC | ChipVariant::Aic8800DW => init_aic8800dc_firmware(&mut ipc, &fw_set, &chip_rev)?,
        ChipVariant::Aic8800D80 | ChipVariant::Aic8800D80X2 => init_aic8800d80_firmware(&mut ipc, &fw_set)?,
        ChipVariant::Unknown => {
            log::error!("[aic8800] Unknown chip, cannot init firmware");
            return Err(SdioError::Unsupported);
        }
    }

    // 固件验证 (MM_RESET + MM_VERSION + STACK_START + FW_VERSION + MAC_ADDR)  
    verify_firmware(&mut ipc)?;  

    log::info!("[aic8800] firmware_init complete"); 

    Ok(())
}

/// 固件验证 — 在 start_app 成功后调用  
///  
/// 通过 4 步验证固件是否正确加载并能响应 IPC 消息:  
///   1. MM_RESET — 复位 MAC 层  
///   2. MM_VERSION — 获取 LMAC/HW/PHY 版本  
///   3. MM_SET_STACK_START — 启动协议栈, 获取 5G/vendor 信息  
///   4. MM_GET_FW_VERSION — 获取固件版本字符串  
///   5. MM_GET_MAC_ADDR — 获取 WiFi MAC 地址  
///  
/// 参考: rwnx_main.c rwnx_cfg80211_init() (line 5803-5835)  
pub fn verify_firmware<H: SdioHost>(  
    ipc: &mut IpcTransport<H>,  
) -> Result<(), SdioError> {  
    log::info!("[aic8800] === Firmware verification start ===");  
  
    // Step 1: MAC 层复位  
    log::info!("[aic8800] Step 1/5: MM_RESET");  
    mm_reset(ipc)?;  
    log::info!("[aic8800] MM_RESET OK");  
  
    // Step 2: 获取版本信息  
    log::info!("[aic8800] Step 2/5: MM_VERSION");  
    let ver = mm_get_version(ipc)?;  
    log::info!(  
        "[aic8800] LMAC version : 0x{:08x}",  
        ver.version_lmac  
    );  
    log::info!(  
        "[aic8800] MAC HW       : 0x{:08x} / 0x{:08x}",  
        ver.version_machw_1, ver.version_machw_2  
    );  
    log::info!(  
        "[aic8800] PHY          : 0x{:08x} / 0x{:08x}",  
        ver.version_phy_1, ver.version_phy_2  
    );  
    log::info!(  
        "[aic8800] features=0x{:08x}, max_sta={}, max_vif={}",  
        ver.features, ver.max_sta_nb, ver.max_vif_nb  
    );  
  
    // Step 3: 启动协议栈  
    log::info!("[aic8800] Step 3/5: MM_SET_STACK_START");  
    let stack = mm_set_stack_start(ipc)?;  
    log::info!(  
        "[aic8800] is_5g_support={}, vendor_info=0x{:02x}",  
        stack.is_5g_support, stack.vendor_info  
    );  
  
    // Step 4: 获取固件版本字符串  
    log::info!("[aic8800] Step 4/5: MM_GET_FW_VERSION");  
    let fw_ver = mm_get_fw_version(ipc)?;  
    let ver_len = (fw_ver.fw_version_len as usize).min(63);  
    if let Ok(ver_str) = core::str::from_utf8(&fw_ver.fw_version[..ver_len]) {  
        log::info!("[aic8800] Firmware version: {}", ver_str);  
    } else {  
        log::info!(  
            "[aic8800] Firmware version (raw): {:?}",  
            &fw_ver.fw_version[..ver_len]  
        );  
    }  
  
    // Step 5: 获取 MAC 地址  
    log::info!("[aic8800] Step 5/5: MM_GET_MAC_ADDR");  
    let mac = mm_get_mac_addr(ipc)?;  
    log::info!(  
        "[aic8800] MAC address: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",  
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]  
    );  
  
    log::info!("[aic8800] === Firmware verification PASSED ===");  
    Ok(())  
}

