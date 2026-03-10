//! 固件二进制数据 — 编译时嵌入  
//!  
//! 所有固件文件通过 include_bytes!() 在编译时嵌入内核镜像。  
//! 路径相对于本文件 (wireless/aic8800/src/firmware_data.rs)，  
//! 即 ../../../firmware/ 指向 StarryOS/firmware/  

// ============================================================  
// AIC8801 固件  
// ============================================================  
  
/// AIC8801 主固件 (U02/U03/U04 通用)  
#[cfg(feature = "fw-aic8801")]  
pub static FW_8801_MAIN: &[u8] = include_bytes!("../../../firmware/fmacfw.bin");  
  
/// AIC8801 补丁固件  
#[cfg(feature = "fw-aic8801")]  
pub static FW_8801_PATCH: &[u8] = include_bytes!("../../../firmware/fmacfw_patch.bin");  
  
#[cfg(not(feature = "fw-aic8801"))]  
pub static FW_8801_MAIN: &[u8] = &[];  
#[cfg(not(feature = "fw-aic8801"))]  
pub static FW_8801_PATCH: &[u8] = &[];  

// ============================================================  
// AIC8800DC 固件  
// ============================================================  
  
/// AIC8800DC U01 固件  
#[cfg(feature = "fw-aic8800dc")]  
pub static FW_DC_U01: &[u8] = include_bytes!("../../../firmware/fmacfw_8800dc.bin");  
  
/// AIC8800DC U01 补丁  
#[cfg(feature = "fw-aic8800dc")]  
pub static FW_DC_PATCH_U01: &[u8] = include_bytes!("../../../firmware/fmacfw_patch_8800dc.bin");  
  
/// AIC8800DC U02 补丁 (U02 为 ROM 固件 + 补丁模式)  
#[cfg(feature = "fw-aic8800dc")]  
pub static FW_DC_PATCH_U02: &[u8] = include_bytes!("../../../firmware/fmacfw_patch_8800dc_u02.bin");  
  
/// AIC8800DC H_U02 补丁 (高性能变体)  
#[cfg(feature = "fw-aic8800dc")]  
pub static FW_DC_PATCH_H_U02: &[u8] = include_bytes!("../../../firmware/fmacfw_patch_8800dc_h_u02.bin");  
  
/// AIC8800DC U01 补丁表  
#[cfg(feature = "fw-aic8800dc")]  
pub static FW_DC_PATCH_TBL_U01: &[u8] = include_bytes!("../../../firmware/fmacfw_patch_tbl_8800dc.bin");  
  
/// AIC8800DC U02 补丁表  
#[cfg(feature = "fw-aic8800dc")]  
pub static FW_DC_PATCH_TBL_U02: &[u8] = include_bytes!("../../../firmware/fmacfw_patch_tbl_8800dc_u02.bin");  
  
/// AIC8800DC H_U02 补丁表  
#[cfg(feature = "fw-aic8800dc")]  
pub static FW_DC_PATCH_TBL_H_U02: &[u8] = include_bytes!("../../../firmware/fmacfw_patch_tbl_8800dc_h_u02.bin");  
  
// 未启用时的空占位  
#[cfg(not(feature = "fw-aic8800dc"))]  
pub static FW_DC_U01: &[u8] = &[];  
#[cfg(not(feature = "fw-aic8800dc"))]  
pub static FW_DC_PATCH_U01: &[u8] = &[];  
#[cfg(not(feature = "fw-aic8800dc"))]  
pub static FW_DC_PATCH_U02: &[u8] = &[];  
#[cfg(not(feature = "fw-aic8800dc"))]  
pub static FW_DC_PATCH_H_U02: &[u8] = &[];  
#[cfg(not(feature = "fw-aic8800dc"))]  
pub static FW_DC_PATCH_TBL_U01: &[u8] = &[];  
#[cfg(not(feature = "fw-aic8800dc"))]  
pub static FW_DC_PATCH_TBL_U02: &[u8] = &[];  
#[cfg(not(feature = "fw-aic8800dc"))]  
pub static FW_DC_PATCH_TBL_H_U02: &[u8] = &[];  

// ============================================================  
// AIC8800D80 固件  
// ============================================================  
  
#[cfg(feature = "fw-aic8800d80")]  
pub static FW_D80_U01: &[u8] = include_bytes!("../../../firmware/fmacfw_8800d80.bin");  
  
#[cfg(feature = "fw-aic8800d80")]  
pub static FW_D80_U02: &[u8] = include_bytes!("../../../firmware/fmacfw_8800d80_u02.bin");  
  
#[cfg(not(feature = "fw-aic8800d80"))]  
pub static FW_D80_U01: &[u8] = &[];  
#[cfg(not(feature = "fw-aic8800d80"))]  
pub static FW_D80_U02: &[u8] = &[];  
  
// ============================================================  
// AIC8800D80X2 固件  
// ============================================================  
  
#[cfg(feature = "fw-aic8800d80x2")]  
pub static FW_D80X2: &[u8] = include_bytes!("../../../firmware/fmacfw_8800d80x2.bin");  
  
#[cfg(not(feature = "fw-aic8800d80x2"))]  
pub static FW_D80X2: &[u8] = &[];  