#![no_std]

extern crate alloc;

pub mod error;
pub mod cccr;
pub mod cmd;

use error::SdioError;

/// SDIO 主机控制器抽象  
pub trait SdioHost: Send + Sync {  
    /// 初始化 SDHCI 控制器，执行 SDIO 卡枚举  
    /// (CMD5 → CMD3 → CMD7 → 设置 4-bit 模式)  
    fn init(&mut self) -> Result<(), SdioError>;  
  
    /// CMD52: 单字节读 (I/O read direct)  
    fn read_byte(&self, func: u8, addr: u32) -> Result<u8, SdioError>;  
  
    /// CMD52: 单字节写 (I/O write direct)  
    fn write_byte(&self, func: u8, addr: u32, val: u8) -> Result<(), SdioError>;  
  
    /// CMD53: 多字节/块读 (I/O read extended, fixed address / FIFO 模式)  
    fn read_fifo(&self, func: u8, addr: u32, buf: &mut [u8]) -> Result<(), SdioError>;  
  
    /// CMD53: 多字节/块写 (I/O write extended, fixed address / FIFO 模式)  
    fn write_fifo(&self, func: u8, addr: u32, buf: &[u8]) -> Result<(), SdioError>;  
  
    // /// CMD53: 多字节/块读 (incrementing address 模式)  
    // fn read_bytes(&self, func: u8, addr: u32, buf: &mut [u8]) -> Result<(), SdioError>;  
  
    // /// CMD53: 多字节/块写 (incrementing address 模式)  
    // fn write_bytes(&self, func: u8, addr: u32, buf: &[u8]) -> Result<(), SdioError>;  
  
    /// 设置指定 function 的 block size  
    fn set_block_size(&self, func: u8, size: u16) -> Result<(), SdioError>;  
  
    fn set_clock(&self, hz: u32) -> Result<(), SdioError> {  
        Ok(()) // 默认空实现  
    }
    /// 使能指定 SDIO function  
    fn enable_func(&self, func: u8) -> Result<(), SdioError>;  
  
    /// 注册 SDIO 中断回调 (可选, 轮询模式下为空实现)  
    fn claim_irq(&self, func: u8, handler: fn()) -> Result<(), SdioError> {  
        Ok(()) // 默认: 轮询模式, 不注册中断  
    }  
  
    /// 获取 SDIO 卡的 vendor/device ID  
    fn vendor_device_id(&self) -> (u16, u16);  
}  