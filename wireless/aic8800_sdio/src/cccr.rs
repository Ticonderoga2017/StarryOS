// ============================================================ 
// CCCR (Card Common Control Registers) 地址定义  
// ============================================================ 

pub const CCCR_SDIO_REVISION:    u32 = 0x00;  
pub const CCCR_SD_REVISION:      u32 = 0x01;  
pub const CCCR_IO_ENABLE:        u32 = 0x02;  
pub const CCCR_IO_READY:         u32 = 0x03;  
pub const CCCR_INT_ENABLE:       u32 = 0x04;  
pub const CCCR_INT_PENDING:      u32 = 0x05;  
pub const CCCR_IO_ABORT:         u32 = 0x06;  
pub const CCCR_BUS_INTERFACE:    u32 = 0x07;  
pub const CCCR_CARD_CAPABILITY:  u32 = 0x08;  
pub const CCCR_CIS_POINTER:      u32 = 0x09; // 3 bytes  
pub const CCCR_BUS_SUSPEND:      u32 = 0x0C;  
pub const CCCR_FUNCTION_SELECT:  u32 = 0x0D;  
pub const CCCR_EXEC_FLAGS:       u32 = 0x0E;  
pub const CCCR_READY_FLAGS:      u32 = 0x0F;  
pub const CCCR_FN0_BLOCK_SIZE:   u32 = 0x10; // 2 bytes  
pub const CCCR_POWER_CONTROL:    u32 = 0x12;  
pub const CCCR_HIGH_SPEED:       u32 = 0x13;  

// ============================================================ 
// FBR (Function Basic Registers) 偏移量  
// Function N 的 FBR 基地址 = 0x100 * N  
// ============================================================ 

pub const FBR_BLOCK_SIZE_OFFSET: u32 = 0x10; // 2 bytes  

// ============================================================ 
// Bus width 设置值  
// ============================================================ 

pub const BUS_WIDTH_1BIT: u8 = 0x00;  
pub const BUS_WIDTH_4BIT: u8 = 0x02;  

// ============================================================ 
// CCCR 寄存器
// ============================================================ 

pub const IO_ENABLE: u32             = 0x02;  // I/O Enable: 每个 bit 对应一个 function (bit1=func1, bit2=func2, ...) 
pub const IO_READY: u32              = 0x03;  // I/O Ready: 对应 function 就绪状态 (read-only)  
pub const FN1_BLOCK_SIZE_LO: u32     = 0x110; // FBR fn1 block size (byte 0)  
pub const FN1_BLOCK_SIZE_HI: u32     = 0x111; // FBR fn1 block size (byte 1) 
/// Bus Speed Select (CCCR v3.0+, SDIO 3.0)  
/// bit[0]: SHS — Support High-Speed (read-only)  
/// bit[1]: EHS — Enable High-Speed (read/write, 写 1 启用高速模式)  
/// bit[3:2]: BSS — Bus Speed Select for UHS  
pub const BUS_SPEED_SELECT: u32       = 0x13;
/// Exec Flags (read-only)  
pub const EXEC_FLAGS: u32             = 0x0E;  

// ============================================================ 
// CIS 相关常量
// ============================================================ 

/// CIS Tuple codes  
pub const CISTPL_NULL:    u8 = 0x00;  
pub const CISTPL_MANFID:  u8 = 0x20;  // Manufacturer Identification  
pub const CISTPL_FUNCID:  u8 = 0x21;  
pub const CISTPL_FUNCE:   u8 = 0x22;  
pub const CISTPL_END:     u8 = 0xFF;

/// CIS Pointer within FBR: offset 0x09-0x0B relative to FBR base  
pub const FBR_CIS_PTR_OFFSET: u32 = 0x09;  

/// CIS Pointer for Function 0: CCCR offset 0x09-0x0B (3 bytes, little-endian)  
pub const FN0_CIS_PTR: u32 = 0x09;  

/// FBR base address for Function N = 0x100 * N  
pub const fn fbr_base(func: u8) -> u32 { (func as u32) * 0x100 }  