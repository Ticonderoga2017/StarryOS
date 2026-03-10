//! 固件选择逻辑 — 根据芯片型号和版本选择正确的固件数据 

use crate::chip_id::*;
use crate::fw_data::*;

/// 选中的固件集合  
pub struct FirmwareSet {  
    /// WiFi 主固件 (AIC8801/D80/D80X2) 或 补丁固件 (AIC8800DC)  
    pub wl_fw: &'static [u8],  
    /// 补丁表 (仅 AIC8800DC 使用, 其他芯片为空)  
    pub patch_tbl: &'static [u8],  
    /// AIC8801 的额外补丁固件  
    pub wl_patch: &'static [u8],  
    /// 描述信息  
    pub desc: &'static str,  
}  

/// 根据芯片型号和版本信息选择固件  
pub fn select_firmware(chip: ChipVariant, rev: &ChipRevision) -> Option<FirmwareSet> {  
    match chip {
        ChipVariant::Aic8801 => {
            // AIC8801: 主固件 + 补丁  
            // U02 和 U03/U04 使用相同的 fmacfw.bin,  
            // 但 U03/U04 使用不同的 BT 补丁 (此处仅关注 WiFi) 
            if FW_8801_MAIN.is_empty() {
                log::error!("[fw_select] AIC8801 firmware not embedded (enable 'fw-aic8801' feature)");  
                return None;  
            }
            Some(FirmwareSet {
                wl_fw: FW_8801_MAIN,
                patch_tbl: &[], // AIC8801 不使用补丁表
                wl_patch: FW_8801_PATCH,
                desc: "AIC8801",
            })
        }
        ChipVariant::Aic8800DC | ChipVariant::Aic8800DW => { 
            // AIC8800DC: ROM 固件 + 补丁模式  
            // 根据 chip_rev 和 is_chip_id_h 选择不同的补丁 
            if FW_DC_U01.is_empty()  && FW_DC_PATCH_U02.is_empty() && FW_DC_PATCH_H_U02.is_empty() {  
                log::error!("[fw_select] AIC8800DC firmware not embedded (enable 'fw-aic8800dc' feature)");  
                return None;  
            } 
            if rev.is_chip_id_h {  
                // 高性能变体  
                log::info!("[fw_select] AIC8800DC H_U02 selected");  
                Some(FirmwareSet {  
                    wl_fw: FW_DC_PATCH_H_U02,  
                    patch_tbl: FW_DC_PATCH_TBL_H_U02,  
                    wl_patch: &[],  
                    desc: "AIC8800DC H_U02",  
                })  
            } else if rev.rev == CHIP_REV_U01 {  
                // U01: 使用完整固件 (非补丁模式)  
                log::info!("[fw_select] AIC8800DC U01 selected");  
                Some(FirmwareSet {  
                    wl_fw: FW_DC_U01,  
                    patch_tbl: FW_DC_PATCH_TBL_U01,  
                    wl_patch: &[],  
                    desc: "AIC8800DC U01",  
                })  
            } else {  
                // U02/U03/U04: 补丁模式  
                log::info!("[fw_select] AIC8800DC U02 selected");  
                Some(FirmwareSet {  
                    wl_fw: FW_DC_PATCH_U02,  
                    patch_tbl: FW_DC_PATCH_TBL_U02,  
                    wl_patch: &[],  
                    desc: "AIC8800DC U02",  
                })  
            } 
        }
        ChipVariant::Aic8800D80 => {
            // D80: 根据版本和 is_chip_id_h 选择  
            // is_chip_id_h → fw_8800d80_h_u02  
            // U01          → fw_8800d80_u01  
            // U02/U03      → fw_8800d80_u02
            if FW_D80_U01.is_empty() && FW_D80_U02.is_empty() {  
                log::error!("[fw_select] AIC8800D80 firmware not embedded (enable 'fw-aic8800d80' feature)");  
                return None;  
            }  
  
            if rev.rev == CHIP_REV_U01 && !rev.is_chip_id_h {  
                log::info!("[fw_select] AIC8800D80 U01 selected");  
                Some(FirmwareSet {  
                    wl_fw: FW_D80_U01,  
                    patch_tbl: &[],  
                    wl_patch: &[],  
                    desc: "AIC8800D80 U01",  
                })  
            } else {  
                // U02/U03 或 H 变体都使用 U02 固件  
                log::info!("[fw_select] AIC8800D80 U02 selected");  
                Some(FirmwareSet {  
                    wl_fw: FW_D80_U02,  
                    patch_tbl: &[],  
                    wl_patch: &[],  
                    desc: "AIC8800D80 U02",  
                })  
            }
        }
        ChipVariant::Aic8800D80X2 => {  
            if FW_D80X2.is_empty() {  
                log::error!("[fw_select] AIC8800D80X2 firmware not embedded (enable 'fw-aic8800d80x2' feature)");  
                return None;  
            }  
            log::info!("[fw_select] AIC8800D80X2 selected");  
            Some(FirmwareSet {  
                wl_fw: FW_D80X2,  
                patch_tbl: &[],  
                wl_patch: &[],  
                desc: "AIC8800D80X2",  
            })  
        }  
  
        ChipVariant::Unknown => {  
            log::error!("[fw_select] Unknown chip variant");  
            None  
        } 
    }
}