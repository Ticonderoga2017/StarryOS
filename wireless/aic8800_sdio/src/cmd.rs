// ---- SD 命令号 ----  
pub const CMD0_GO_IDLE:          u8 = 0;  
pub const CMD3_SEND_REL_ADDR:    u8 = 3;  
pub const CMD5_IO_SEND_OP_COND:  u8 = 5;  
pub const CMD7_SELECT_CARD:      u8 = 7;  
pub const CMD52_IO_RW_DIRECT:    u8 = 52;  
pub const CMD53_IO_RW_EXTENDED:  u8 = 53;  
  
// ---- CMD5 OCR (IO Operation Conditions Register) 位定义 ----  
  
/// bit 31: Card Ready — 卡完成上电初始化后置 1  
pub const OCR_IORDY: u32           = 1 << 31;  
  
/// bits 30:28: Number of I/O Functions (1-7)  
pub const OCR_IO_FUNC_SHIFT: u32   = 28;  
pub const OCR_IO_FUNC_MASK: u32    = 0x7 << OCR_IO_FUNC_SHIFT;  
  
/// bit 27: Memory Present — 卡同时具有存储功能 (combo card)  
pub const OCR_MEM_PRESENT: u32     = 1 << 27;  
  
/// bits 23:0: 电压窗口 (Voltage Window)  
/// 每个 bit 对应一个 100mV 电压范围  
/// bit 15 = 2.7-2.8V, bit 16 = 2.8-2.9V, ..., bit 23 = 3.5-3.6V  
pub const OCR_VOLTAGE_MASK: u32    = 0x00FF_8000;  
  
/// 3.2-3.3V | 3.3-3.4V (bit 20 + bit 21)  
pub const OCR_3V2_3V4: u32         = 0x0030_0000;

// ---- R5 响应标志位 (CMD52/CMD53 response) ----  
pub const R5_COM_CRC_ERROR:    u32 = 1 << 15;  
pub const R5_ILLEGAL_COMMAND:  u32 = 1 << 14;  
pub const R5_IO_CURRENT_STATE: u32 = 0x3 << 12;  // bits 13:12  
pub const R5_ERROR:            u32 = 1 << 11;  
pub const R5_FUNCTION_NUMBER:  u32 = 1 << 9;  
pub const R5_OUT_OF_RANGE:     u32 = 1 << 8;  
pub const R5_ERROR_MASK: u32 = R5_COM_CRC_ERROR | R5_ILLEGAL_COMMAND  
    | R5_ERROR | R5_FUNCTION_NUMBER | R5_OUT_OF_RANGE;  
  
// ---- CMD52/CMD53 参数构造常量 ----  
pub const CMD52_RW_FLAG:     u32 = 1 << 31;  
pub const CMD52_RAW_FLAG:    u32 = 1 << 27;  // Read After Write  
pub const CMD53_RW_FLAG:     u32 = 1 << 31;  
pub const CMD53_BLOCK_MODE:  u32 = 1 << 27;  
pub const CMD53_OP_CODE_INC: u32 = 1 << 26;  // incrementing address  
pub const SDIO_ADDR_MASK:    u32 = 0x1FFFF;  // 17-bit address  
  
// ---- Transfer Mode Register 位定义 ----  
pub const TM_DMA_EN:         u16 = 1 << 0;  
pub const TM_BLK_CNT_EN:     u16 = 1 << 1;  
pub const TM_AUTO_CMD12:     u16 = 1 << 2;  
pub const TM_DATA_DIR_READ:  u16 = 1 << 4;  // 1=read, 0=write  
pub const TM_MULTI_BLOCK:    u16 = 1 << 5;  
  
// ---- Present State Register 位定义 (补充) ----  
pub const PSR_BUF_WR_EN:     u32 = 1 << 10;  // Buffer Write Enable  
pub const PSR_BUF_RD_EN:     u32 = 1 << 11;  // Buffer Read Enable  

pub const PIO_TIMEOUT: u32 = 1_000_000;  