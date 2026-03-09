//! SG2002 SDIO1 硬件初始化 (时钟 / 复位 / Pinmux / CardDetect)  
//!  
//! SDIO1 控制器位于 RTC 域, 基址 0x0500_0000。  
//!  
//! 时钟由两层控制:  
//!   1. 主系统 CRG (0x0300_2000) — clk_en_0, clk_byp_0, div_clk_sd1, div_clk_100k_sd1  
//!   2. RTC 子系统 (0x0502_5000) — rtcsys_clk_en, rtcsys_clkbyp, rtcsys_clkmux  
//!  
//! 复位:  
//!   - SOFT_RSTN_0 (0x0300_3000) bit17 在 TRM Reset 章节中标记为 Reserved  
//!     (bit16=SD0, bit17=Reserved, bit18=SDMA) — 不操作此位  
//!   - 仅使用 RTC 域 rtcsys_rst_ctrl (0x0502_5018) bit[2] 控制 SDIO1 复位  
//!  
//! Pinmux:  
//!   - SD1_D3/D2/D1/D0/CMD/CLK 与 VO[32..37] 复用, 由 0x0502_70E4 控制  
//!  
//! CardDetect:  
//!   - WiFi 模块无 CD 引脚  
//!   - 方式 A: SDHCI HOST_CTL1 bit7=CARD_DET_SEL=1, bit6=CARD_DET_TEST=1 (标准方式)  
//!   - 方式 B: sd_ctrl_opt (0x0300_0294) bit8/bit9 强制检测 (SoC 厂商扩展)  

use core::ptr::{read_volatile, write_volatile};  

// ============================================================  
// 虚拟地址偏移 (来自 axconfig.toml: phys-virt-offset)  
// ============================================================  
const PHYS_VIRT_OFFSET: usize = 0xffff_ffc0_0000_0000;  
  
// ============================================================  
// CRG 基址: 0x0300_2000 (CLKGEN/PLL)  
// ============================================================  
const CRG_BASE: usize = 0x0300_2000 + PHYS_VIRT_OFFSET; 

/// clk_en_0 (offset 0x000)  
///   bit21: clk_axi4_sd1 (默认 0x1)  
///   bit22: clk_sd1      (默认 0x1)  
///   bit23: clk_100k_sd1 (默认 0x1)  
const CLK_EN_0: usize = 0x000;  
const CLK_EN_0_SD1_ALL: u32 = (1 << 21) | (1 << 22) | (1 << 23);  

/// clk_byp_0 (offset 0x030)  
///   bit7: clk_sd1 bypass to xtal (默认 0x1; 清 0 使用 PLL)  
const CLK_BYP_0: usize = 0x030;  
const CLK_BYP_0_SD1: u32 = 1 << 7;  

/// div_clk_sd1 (offset 0x07c)  
///   bit[0]: Divider Reset Control (0=assert, 1=de-assert)  
const DIV_CLK_SD1: usize = 0x07c;  
  
/// div_clk_100k_sd1 (offset 0x084)  
///   bit[0]: Divider Reset Control (0=assert, 1=de-assert)  
const DIV_CLK_100K_SD1: usize = 0x084;  

// ============================================================  
// System Control 基址: 0x0300_0000 (TOP_MISC)  
// ============================================================  
const SYSCTRL_BASE: usize = 0x0300_0000 + PHYS_VIRT_OFFSET;  
  
/// sd_ctrl_opt (offset 0x294)  
///   bit8:  reg_sd1_carddet_ow — 使能覆写  
///   bit9:  reg_sd1_carddet_sw — 覆写值 (1=卡已插入)  
///   bit10: reg_sd1_phy_sel  
const SD_CTRL_OPT: usize = 0x294;  
const SD1_CARDDET_OW: u32 = 1 << 8;  
const SD1_CARDDET_SW: u32 = 1 << 9;  

// ============================================================  
// RTC 子系统 CTRL 基址: 0x0502_5000  
// ============================================================  
const RTCSYS_CTRL_BASE: usize = 0x0502_5000 + PHYS_VIRT_OFFSET;  
  
/// rtcsys_rst_ctrl (offset 0x018)  
///   bit2: reg_soft_rstn_sdio — 0=reset, 1=de-assert (默认 0x1)  
const RTCSYS_RST_CTRL: usize = 0x018;  
const RTCSYS_RST_SDIO: u32 = 1 << 2;  

/// rtcsys_clkmux (offset 0x01c)  
///   bits[3:0]: reg_sdio_clk_mux — 0=fpll/4, 1=osc_div (默认 0x0)  
const RTCSYS_CLKMUX: usize = 0x01c;  
  
/// rtcsys_clkbyp (offset 0x030)  
///   bit1: clk_sdio — 0=clk_sd1_pre(PLL), 1=xtal (默认 0xFFFFFFFF)  
const RTCSYS_CLKBYP: usize = 0x030;  
const RTCSYS_CLKBYP_SDIO: u32 = 1 << 1;  
  
/// rtcsys_clk_en (offset 0x034)  
///   bit1:  clk_sd1  
///   bit2:  clk_fab_sd1  
///   默认 0xFFFFFFFF (全使能)  
const RTCSYS_CLK_EN: usize = 0x034;  
const RTCSYS_CLK_EN_SD1_ALL: u32 = (1 << 1) | (1 << 2);  

// ============================================================  
// RTCSYS_IO 基址: 0x0502_7000  
// ============================================================  
const RTCSYS_IO_BASE: usize = 0x0502_7000 + PHYS_VIRT_OFFSET;  
  
/// SD1/VO 引脚功能选择 (物理地址 0x0502_70E4)  
/// 写 0 选择 SD1 功能  
const FMUX_SD1_VO: usize = 0x0E4;  

const SDIO1_PADDR: usize = 0x0432_0000;

// ============================================================  
// MMIO 辅助函数  
// ============================================================  
  
#[inline]  
unsafe fn mmio_read32(addr: usize) -> u32 {  
    read_volatile(addr as *const u32)  
}  
  
#[inline]  
unsafe fn mmio_write32(addr: usize, val: u32) {  
    write_volatile(addr as *mut u32, val);  
}  
  
#[inline]  
unsafe fn mmio_set_bits32(addr: usize, bits: u32) {  
    let val = mmio_read32(addr);  
    mmio_write32(addr, val | bits);  
}  
  
#[inline]  
unsafe fn mmio_clr_bits32(addr: usize, bits: u32) {  
    let val = mmio_read32(addr);  
    mmio_write32(addr, val & !bits);  
} 

// ============================================================  
// 公开接口  
// ============================================================  
  
/// SDIO1 硬件使能: pinmux → 时钟 → 复位 → 卡检测覆写  
///  
/// 在 CviSdhci::init() 开头调用, 仅对 SDIO1 执行。  
pub fn sdio1_hw_init() {
    log::info!("[sdio1_hw] === SDIO1 Hardware Init Start ==="); 
    unsafe {
        // ==== Step 1: Pinmux ====  
        let addr = RTCSYS_IO_BASE + FMUX_SD1_VO;  
        let old = mmio_read32(addr);  
        mmio_write32(addr, 0x0);  
        log::info!(  
            "[sdio1_hw] FMUX_SD1_VO   @0x050270E4: 0x{:08x} -> 0x{:08x}",  
            old, mmio_read32(addr)  
        );  

        // ==== Step 2: CRG 时钟使能 ====  
        // 2a. clk_en_0: bit21/22/23 使能  
        let addr = CRG_BASE + CLK_EN_0;  
        let old = mmio_read32(addr);  
        mmio_set_bits32(addr, CLK_EN_0_SD1_ALL);  
        log::info!(  
            "[sdio1_hw] clk_en_0      @0x03002000: 0x{:08x} -> 0x{:08x}",  
            old, mmio_read32(addr)  
        ); 

        // 2b. clk_byp_0: bit7 清 0 (PLL, 非 xtal)  
        let addr = CRG_BASE + CLK_BYP_0;  
        let old = mmio_read32(addr);  
        mmio_clr_bits32(addr, CLK_BYP_0_SD1);  
        log::info!(  
            "[sdio1_hw] clk_byp_0     @0x03002030: 0x{:08x} -> 0x{:08x}",  
            old, mmio_read32(addr)  
        );  

        // 2c. div_clk_sd1: bit0=1 de-assert divider reset  
        let addr = CRG_BASE + DIV_CLK_SD1;  
        let old = mmio_read32(addr);  
        mmio_set_bits32(addr, 0x1);  
        log::info!(  
            "[sdio1_hw] div_clk_sd1   @0x0300207C: 0x{:08x} -> 0x{:08x}",  
            old, mmio_read32(addr)  
        );  

        // 2d. div_clk_100k_sd1: bit0=1 de-assert divider reset  
        let addr = CRG_BASE + DIV_CLK_100K_SD1;  
        let old = mmio_read32(addr);  
        mmio_set_bits32(addr, 0x1);  
        log::info!(  
            "[sdio1_hw] div_clk_100k  @0x03002084: 0x{:08x} -> 0x{:08x}",  
            old, mmio_read32(addr)  
        );  

        // ==== Step 3: RTC 域时钟 ====  
        // 3a. rtcsys_clkmux: bits[3:0]=0 (fpll/4, 默认)  
        let addr = RTCSYS_CTRL_BASE + RTCSYS_CLKMUX;  
        let old = mmio_read32(addr);  
        let val = old & !0xF; // 清除 bits[3:0], 选 fpll/4  
        mmio_write32(addr, val);  
        log::info!(  
            "[sdio1_hw] rtcsys_clkmux @0x0502501C: 0x{:08x} -> 0x{:08x}",  
            old, mmio_read32(addr)  
        );  

        // 3b. rtcsys_clk_en: bit1/bit2 使能  
        let addr = RTCSYS_CTRL_BASE + RTCSYS_CLK_EN;  
        let old = mmio_read32(addr);  
        mmio_set_bits32(addr, RTCSYS_CLK_EN_SD1_ALL);  
        log::info!(  
            "[sdio1_hw] rtcsys_clk_en @0x05025034: 0x{:08x} -> 0x{:08x}",  
            old, mmio_read32(addr)  
        );  

        // 3c. rtcsys_clkbyp: bit1 清 0 (PLL, 非 xtal)  
        let addr = RTCSYS_CTRL_BASE + RTCSYS_CLKBYP;  
        let old = mmio_read32(addr);  
        mmio_clr_bits32(addr, RTCSYS_CLKBYP_SDIO);  
        log::info!(  
            "[sdio1_hw] rtcsys_clkbyp @0x05025030: 0x{:08x} -> 0x{:08x}",  
            old, mmio_read32(addr)  
        );  

        // ==== Step 4: RTC 域复位解除 ====  
        // 注意: 不操作 SOFT_RSTN_0 bit17 (TRM Reset 章节标记为 Reserved)  
        let addr = RTCSYS_CTRL_BASE + RTCSYS_RST_CTRL;  
        let old = mmio_read32(addr);  
        mmio_set_bits32(addr, RTCSYS_RST_SDIO);  
        log::info!(  
            "[sdio1_hw] rtcsys_rst    @0x05025018: 0x{:08x} -> 0x{:08x}",  
            old, mmio_read32(addr)  
        ); 

        // ==== Step 5: SoC 级卡检测覆写 ====  
        // WiFi 模块无 CD 引脚, 强制 CARD_INSERTED = 1  
        let addr = SYSCTRL_BASE + SD_CTRL_OPT;  
        let old = mmio_read32(addr);  
        mmio_set_bits32(addr, SD1_CARDDET_OW | SD1_CARDDET_SW);  
        log::info!(  
            "[sdio1_hw] sd_ctrl_opt   @0x03000294: 0x{:08x} -> 0x{:08x}",  
            old, mmio_read32(addr)  
        ); 

        // 短延迟, 等待时钟和复位稳定  
        for _ in 0..10_000u32 {  
            core::hint::spin_loop();  
        }    

        log::info!("[sdio1_hw] === SDIO1 Hardware Init Done ===");      
    }    
}

/// Dump SDIO1 相关寄存器 (调试)  
pub fn sdio1_hw_dump() {  
    unsafe {  
        log::info!("===== SDIO1 HW Register Dump =====");  
        log::info!("  clk_en_0      @0x03002000 = 0x{:08x}", mmio_read32(CRG_BASE + CLK_EN_0));  
        log::info!("  clk_byp_0     @0x03002030 = 0x{:08x}", mmio_read32(CRG_BASE + CLK_BYP_0));  
        log::info!("  div_clk_sd1   @0x0300207C = 0x{:08x}", mmio_read32(CRG_BASE + DIV_CLK_SD1));  
        log::info!("  div_clk_100k  @0x03002084 = 0x{:08x}", mmio_read32(CRG_BASE + DIV_CLK_100K_SD1));  
        log::info!("  sd_ctrl_opt   @0x03000294 = 0x{:08x}", mmio_read32(SYSCTRL_BASE + SD_CTRL_OPT));  
        log::info!("  rtcsys_rst    @0x05025018 = 0x{:08x}", mmio_read32(RTCSYS_CTRL_BASE + RTCSYS_RST_CTRL));  
        log::info!("  rtcsys_clkmux @0x0502501C = 0x{:08x}", mmio_read32(RTCSYS_CTRL_BASE + RTCSYS_CLKMUX));  
        log::info!("  rtcsys_clkbyp @0x05025030 = 0x{:08x}", mmio_read32(RTCSYS_CTRL_BASE + RTCSYS_CLKBYP));  
        log::info!("  rtcsys_clk_en @0x05025034 = 0x{:08x}", mmio_read32(RTCSYS_CTRL_BASE + RTCSYS_CLK_EN));  
        log::info!("  FMUX_SD1_VO   @0x050270E4 = 0x{:08x}", mmio_read32(RTCSYS_IO_BASE + FMUX_SD1_VO));  
        // 读 SDIO1 HOST_VERSION 验证 MMIO 可达  
        let sdio1_va = SDIO1_PADDR + PHYS_VIRT_OFFSET;  
        let ver = read_volatile((sdio1_va + 0xFE) as *const u16);  
        log::info!("  SDIO1_HOST_VER @0x050000FE = 0x{:04x}", ver);  
        log::info!("======================================");  
    }
}