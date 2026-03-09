#![no_std]

mod regs;
pub mod hw_init;

use core::ptr::{read_volatile, write_volatile};
use aic8800_sdio::{SdioHost, cccr::*, cmd::*, error::SdioError};
use crate::{hw_init::sdio1_hw_init, regs::*};

/// CVI SoC WiFi SDIO 控制器  
pub struct CviSdhci {  
    base: usize,  // MMIO 基地址  
    rca: u16,     // 相对卡地址  
    vendor_id: u16,  
    device_id: u16,  
}  

impl CviSdhci {
    pub fn new(base_addr: usize) -> Self {
        Self { 
            base: base_addr, 
            rca: 0, 
            vendor_id: 0, 
            device_id: 0,
         }
    }

    // ---- 底层 MMIO 操作 ---- 
    fn read32(&self, offset: u32) -> u32 {
        unsafe {
            read_volatile((self.base + offset as usize) as *const u32)
        }
    }
    
    fn write32(&self, offset: u32, val: u32) {
        unsafe {
            write_volatile((self.base + offset as usize) as *mut u32, val);
        }
    }

    fn read16(&self, offset: u32) -> u16 {
        unsafe {
            read_volatile((self.base + offset as usize) as *const u16)
        }
    }
    
    fn write16(&self, offset: u32, val: u16) {
        unsafe {
            write_volatile((self.base + offset as usize) as *mut u16, val);
        }
    }
    fn read8(&self, offset: u32) -> u8 {
        unsafe {
            read_volatile((self.base + offset as usize) as *const u8)
        }
    }
    
    fn write8(&self, offset: u32, val: u8) {
        unsafe {
            write_volatile((self.base + offset as usize) as *mut u8, val);
        }
    }

    // ---- SD 命令发送 ---
    fn wait_cmd_idle(&self) -> Result<(), SdioError> {
        for _ in 0..CMD_RESPONSE_TIMEOUT {
            if self.read32(SDHCI_PRESENT_STATE) & SDHCI_CMD_INHIBIT == 0 {
                return Ok(());
            }
            core::hint::spin_loop(); // 告知 CPU 这是自旋等待，降低功耗/释放流水线资源  
        }
        Err(SdioError::Timeout)
    }

    fn wait_cmd_complete(&self) -> Result<u32, SdioError> {
        for _ in 0..CMD_RESPONSE_TIMEOUT {
            let norm_status = self.read16(SDHCI_INT_STATUS_NORM);
            // 检查 Error 汇总位 (Normal Status bit 15)
            if norm_status & NORM_INT_ERROR != 0 {
                let err_status = self.read16(SDHCI_INT_STATUS_ERR);
                // 清除: 先清 error，再清 normal  
                self.write16(SDHCI_INT_STATUS_ERR, err_status);  
                self.write16(SDHCI_INT_STATUS_NORM, norm_status); 

                if err_status & (ERR_INT_CMD_CRC | ERR_INT_DAT_CRC) != 0 {
                    return Err(SdioError::CrcError);
                }
                if err_status & (ERR_INT_CMD_TIMEOUT | ERR_INT_DAT_TIMEOUT) != 0 {
                    return Err(SdioError::Timeout);
                }

                return Err(SdioError::IoError);
            }

            // 检查 Command Complete (Normal Status bit 0)  
            if norm_status & NORM_INT_CMD_COMPLETE != 0 {  
                self.write16(SDHCI_INT_STATUS_NORM, NORM_INT_CMD_COMPLETE);  
                return Ok(self.read32(SDHCI_RESPONSE));  
            }  
    
            core::hint::spin_loop();
        }
        Err(SdioError::Timeout)
    }

    fn send_cmd(&self, cmd_idx: u8, arg: u32) -> Result<u32, SdioError> {
        self.wait_cmd_idle()?; // 如果超时或 CMD 线忙，直接通过 ? 返回错误
        // 写入参数 
        self.write32(SDHCI_ARGUMENT, arg);

        // 发送命令: [15:8]=cmd_idx, [7:0]=response type  
        let cmd_reg = match cmd_idx {  
            0 => (0u16 << 8) | 0x00,           // CMD0: 无响应  
            3 => (3u16 << 8) | 0x1A,           // CMD3: R6  
            5 => (5u16 << 8) | 0x02,           // CMD5: R4  
            7 => (7u16 << 8) | 0x1B,           // CMD7: R1b  
            52 => (52u16 << 8) | 0x1A,         // CMD52: R5  
            53 => (53u16 << 8) | 0x3A,         // CMD53: R5 + data  
            _ => return Err(SdioError::Unsupported),  
        }; 
        self.write16(SDHCI_COMMAND, cmd_reg);

        // 等待响应  
        self.wait_cmd_complete()
    }

    /// 检查 R5 响应的错误标志
    fn check_r5_response(&self, resp: u32) -> Result<u8, SdioError> {
        if resp & R5_COM_CRC_ERROR != 0 {
            return Err(SdioError::CrcError);
        }
        if resp & (R5_ILLEGAL_COMMAND | R5_FUNCTION_NUMBER | R5_OUT_OF_RANGE) != 0 {
            log::error!("R5 error flags: 0x{:04x}", (resp >> 8) & 0xFF);  
            return Err(SdioError::IoError); 
        }
        if resp & R5_ERROR != 0 {
            return Err(SdioError::IoError);
        }
        Ok((resp & 0xFF) as u8)  
    }

    // ---- CMD52 ----  

    fn cmd52_read(&self, func: u8, addr: u32) -> Result<u8, SdioError> {
        if addr > SDIO_ADDR_MASK {
            return Err(SdioError::Unsupported);
        }
        let arg = ((func as u32 & 0x07) << 28) | ((addr & SDIO_ADDR_MASK) << 9);
        let resp = self.send_cmd(52, arg)?;
        self.check_r5_response(resp)
    }

    fn cmd52_write(&self, func: u8, addr: u32, val: u8) -> Result<(), SdioError> {
        if addr > SDIO_ADDR_MASK {
            return Err(SdioError::Unsupported);
        }
        let arg = CMD52_RW_FLAG
                    | ((func as u32 & 0x07) << 28)
                    | ((addr & SDIO_ADDR_MASK) << 9)
                    | (val as u32);
        let resp = self.send_cmd(52, arg)?;
        self.check_r5_response(resp)?;
        Ok(())
    }

    /// CMD52 写入后读回 (Read After Write)  
    fn cmd52_write_read(&self, func: u8, addr: u32, val: u8) -> Result<u8, SdioError> {  
        if addr > SDIO_ADDR_MASK {  
            return Err(SdioError::Unsupported);  
        }  
        let arg = CMD52_RW_FLAG  
                | CMD52_RAW_FLAG  
                | ((func as u32 & 0x07) << 28)  
                | ((addr & SDIO_ADDR_MASK) << 9)  
                | (val as u32);  
        let resp = self.send_cmd(52, arg)?;  
        self.check_r5_response(resp)  
    }

    // ---- CMD53 ---- 

    fn cmd53_read(&self, func: u8, addr: u32, buf: &mut [u8], block_size: u16, use_block_mode: bool) -> Result<(), SdioError> {  
        if addr > SDIO_ADDR_MASK || buf.is_empty() {  
            return Err(SdioError::Unsupported);  
        }  
        let (block_mode, count, blk_sz) = if use_block_mode && block_size > 0  {
            let nblocks = buf.len() / block_size as usize;
            if nblocks == 0 || buf.len() % block_size as usize != 0 {
                return Err(SdioError::Unsupported);  
            }
            (true, nblocks, block_size)
        } else {
            // Byte mode: count = 字节数, 0 表示 512 
            let byte_count = if buf.len() == 512 { 0 } else { buf.len() };
            if buf.len() > 512 {
                return Err(SdioError::Unsupported); // byte 模式最大 512 
            }
            (false, byte_count, buf.len() as u16)
        };

        // 构造 CMD53 参数
        let arg = ((func as u32 & 0x07) << 28)                
                | if block_mode { CMD53_BLOCK_MODE } else { 0 }
                | CMD53_OP_CODE_INC // 大多数情况用递增地址 
                | ((addr & SDIO_ADDR_MASK) << 9)
                | (count as u32 & 0x1FF); // count 字段 9 位
        
        // 配置传输参数
        self.write16(SDHCI_BLOCK_SIZE, blk_sz);
        let xfer_blocks = if block_mode { count as u16 } else { 1 };
        self.write16(SDHCI_BLOCK_COUNT, xfer_blocks);

        // Transfer Mode: read + (multi-block if >1 block) + block count enable 
        let mut tm = TM_DATA_DIR_READ;
        if xfer_blocks > 1 {
            tm |= TM_MULTI_BLOCK | TM_BLK_CNT_EN;
        }
        self.write16(SDHCI_TRANSFER_MODE, tm);

        // 发送 CMD53  
        let resp = self.send_cmd(53, arg)?;  
        self.check_r5_response(resp)?; 

        // PIO 读取数据  
        self.pio_read(buf, blk_sz, xfer_blocks)?;

        // 等待 Transfer Complete  
        self.wait_transfer_complete()?;

        Ok(())
    }

    fn cmd53_read_fixed(&self, func: u8, addr: u32, buf: &mut [u8], block_size: u16, use_block_mode: bool) -> Result<(), SdioError> {  
        // 与 cmd53_read 相同，但不设置 OP_CODE_INC (fixed address)  
        // SDIO WiFi 芯片的 FIFO 读写通常使用 fixed address  
        if addr > SDIO_ADDR_MASK || buf.is_empty() {  
            return Err(SdioError::Unsupported);  
        }  
  
        let (block_mode, count, blk_sz) = if use_block_mode && block_size > 0 {  
            let nblocks = buf.len() / block_size as usize;  
            if nblocks == 0 || buf.len() % block_size as usize != 0 {  
                return Err(SdioError::Unsupported);  
            }  
            (true, nblocks, block_size)  
        } else {  
            let byte_count = if buf.len() == 512 { 0usize } else { buf.len() };  
            if buf.len() > 512 {  
                return Err(SdioError::Unsupported);  
            }  
            (false, byte_count, buf.len() as u16)  
        };

        let arg = ((func as u32 & 0x07) << 28)  
                | if block_mode { CMD53_BLOCK_MODE } else { 0 }  
                | ((addr & SDIO_ADDR_MASK) << 9)  
                | (count as u32 & 0x1FF); // count 字段 9 位
        self.write16(SDHCI_BLOCK_SIZE, blk_sz);
        let xfer_blocks = if block_mode { count as u16 } else { 1 };
        self.write16(SDHCI_BLOCK_COUNT, xfer_blocks);

        let mut tm = TM_DATA_DIR_READ;
        if xfer_blocks > 1 {
            tm |= TM_MULTI_BLOCK | TM_BLK_CNT_EN;
        }
        self.write16(SDHCI_TRANSFER_MODE, tm);

        let resp = self.send_cmd(53, arg)?;
        self.check_r5_response(resp)?;

        self.pio_read(buf, blk_sz, xfer_blocks)?;
        self.wait_transfer_complete()?;

        Ok(())
    }

    fn cmd53_write(&self, func: u8, addr: u32, buf: &[u8], block_size: u16, use_block_mode: bool) -> Result<(), SdioError> {  
        if addr > SDIO_ADDR_MASK || buf.is_empty() {  
            return Err(SdioError::Unsupported);  
        }  
  
        let (block_mode, count, blk_sz) = if use_block_mode && block_size > 0 {  
            let nblocks = buf.len() / block_size as usize;  
            if nblocks == 0 || buf.len() % block_size as usize != 0 {  
                return Err(SdioError::Unsupported);  
            }  
            (true, nblocks, block_size)  
        } else {  
            let byte_count = if buf.len() == 512 { 0usize } else { buf.len() };  
            if buf.len() > 512 {  
                return Err(SdioError::Unsupported);  
            }  
            (false, byte_count, buf.len() as u16)  
        };  

        let arg = CMD53_RW_FLAG  
                | ((func as u32 & 0x07) << 28)  
                | if block_mode { CMD53_BLOCK_MODE } else { 0 }  
                | CMD53_OP_CODE_INC  
                | ((addr & SDIO_ADDR_MASK) << 9)  
                | (count as u32 & 0x1FF);  
  
        self.write16(SDHCI_BLOCK_SIZE, blk_sz);  
        let xfer_blocks = if block_mode { count as u16 } else { 1 };  
        self.write16(SDHCI_BLOCK_COUNT, xfer_blocks);  

        // Transfer Mode: write (bit4=0) + multi-block flags  
        let mut tm: u16 = 0; // direction = write  
        if xfer_blocks > 1 {  
            tm |= TM_MULTI_BLOCK | TM_BLK_CNT_EN;  
        }  
        self.write16(SDHCI_TRANSFER_MODE, tm);  
  
        let resp = self.send_cmd(53, arg)?;  
        self.check_r5_response(resp)?;  

        // PIO 写入数据  
        self.pio_write(buf, blk_sz, xfer_blocks)?;
        self.wait_transfer_complete()?;

        Ok(())
    }

     fn cmd53_write_fixed(  
        &self, func: u8, addr: u32, buf: &[u8], block_size: u16, use_block_mode: bool,  
    ) -> Result<(), SdioError> {  
        // 与 cmd53_write 相同，但 fixed address (不设 OP_CODE_INC)  
        if addr > SDIO_ADDR_MASK || buf.is_empty() {  
            return Err(SdioError::Unsupported);  
        }  
  
        let (block_mode, count, blk_sz) = if use_block_mode && block_size > 0 {  
            let nblocks = buf.len() / block_size as usize;  
            if nblocks == 0 || buf.len() % block_size as usize != 0 {  
                return Err(SdioError::Unsupported);  
            }  
            (true, nblocks, block_size)  
        } else {  
            let byte_count = if buf.len() == 512 { 0usize } else { buf.len() };  
            if buf.len() > 512 {  
                return Err(SdioError::Unsupported);  
            }  
            (false, byte_count, buf.len() as u16)  
        };  
  
        let arg = CMD53_RW_FLAG  
                | ((func as u32 & 0x07) << 28)  
                | if block_mode { CMD53_BLOCK_MODE } else { 0 }  
                // 不设置 CMD53_OP_CODE_INC  
                | ((addr & SDIO_ADDR_MASK) << 9)  
                | (count as u32 & 0x1FF);  
  
        self.write16(SDHCI_BLOCK_SIZE, blk_sz);  
        let xfer_blocks = if block_mode { count as u16 } else { 1 };  
        self.write16(SDHCI_BLOCK_COUNT, xfer_blocks);  
  
        let mut tm: u16 = 0;  
        if xfer_blocks > 1 {  
            tm |= TM_MULTI_BLOCK | TM_BLK_CNT_EN;  
        }  
        self.write16(SDHCI_TRANSFER_MODE, tm);  
  
        let resp = self.send_cmd(53, arg)?;  
        self.check_r5_response(resp)?;  
  
        self.pio_write(buf, blk_sz, xfer_blocks)?;  
        self.wait_transfer_complete()?;  
  
        Ok(())  
    }  

    // ---- PIO 数据传输实现 ---- 

    /// PIO 读取: 逐块等待 Buffer Read Ready → 读取 Buffer Data Port 
    fn pio_read(&self, buf: &mut [u8], block_size: u16, nblocks: u16) -> Result<(), SdioError> {  
        let mut offset = 0;

        for _ in 0..nblocks {
            self.wait_buffer_read_ready()?; // 等待 Buffer Read Ready 中断状态位

            // 每次从 Buffer Data Port 读 4 字节
            let words = (block_size as usize + 3) / 4; // 向上取整
            for _ in 0..words {
                let data = self.read32(SDHCI_BUFFER);
                let byte_offset = data.to_le_bytes(); // 转换为字节数组，处理未对齐的最后一个 word
                let remaining = buf.len() - offset;
                let copy_len = core::cmp::min(4, remaining);
                buf[offset..offset + copy_len].copy_from_slice(&byte_offset[..copy_len]);
                offset += copy_len;
            }
        }

        Ok(())
    }

    /// PIO 写入: 逐块等待 Buffer Write Ready → 写入 Buffer Data Port 
    fn pio_write(&self, buf: &[u8], block_size: u16, nblocks: u16) -> Result<(), SdioError> {  
        let mut offset = 0;

        for _ in 0..nblocks {
            self.wait_buffer_write_ready()?; // 等待 Buffer Write Ready 中断状态位

            let words = (block_size as usize + 3) / 4; // 向上取整
            for _ in 0..words {
                let mut data: [u8; 4] = [0; 4];
                let remaining = buf.len() - offset;
                let copy_len = core::cmp::min(4, remaining);
                data[..copy_len].copy_from_slice(&buf[offset..offset + copy_len]);
                let word = u32::from_le_bytes(data);
                self.write32(SDHCI_BUFFER, word);
                offset += copy_len;
            }
        }

        Ok(())
    }

    /// 等待 Buffer Read Ready 中断状态位
    fn wait_buffer_read_ready(&self) -> Result<(), SdioError> {  
        for _ in 0..PIO_TIMEOUT {  
            let norm_status = self.read16(SDHCI_INT_STATUS_NORM);  
            if norm_status & NORM_INT_ERROR != 0 {  
                let err = self.read16(SDHCI_INT_STATUS_ERR);
                self.write16(SDHCI_INT_STATUS_ERR, err); // 清除错误状态位
                self.write16(SDHCI_INT_STATUS_NORM, NORM_INT_ERROR); // 清除错误汇总位;
                log::error!("PIO read error: norm_status=0x{:04x}, err_status=0x{:04x}", norm_status, err);
                return Err(SdioError::IoError);
            }  
            // 检查 Buffer Read Ready (bit 5)
            if norm_status & NORM_INT_BUF_RD_READY != 0 {  
                // 清除该中断位 (W1C) 
                self.write16(SDHCI_INT_STATUS_NORM, NORM_INT_BUF_RD_READY); // 清除状态位  
                return Ok(());  
            }
            core::hint::spin_loop();  
        }  
        Err(SdioError::Timeout)  
    }

    /// 等待 Buffer Write Ready 中断状态位 
    fn wait_buffer_write_ready(&self) -> Result<(), SdioError> {  
        for _ in 0..PIO_TIMEOUT {  
            let norm_status = self.read16(SDHCI_INT_STATUS_NORM);  
            if norm_status & NORM_INT_ERROR != 0 {  
                let err = self.read16(SDHCI_INT_STATUS_ERR);
                self.write16(SDHCI_INT_STATUS_ERR, err); // 清除错误状态位
                self.write16(SDHCI_INT_STATUS_NORM, NORM_INT_ERROR); // 清除错误汇总位;
                log::error!("PIO write error: norm_status=0x{:04x}, err_status=0x{:04x}", norm_status, err);
                return Err(SdioError::IoError);
            }  
            // 检查 Buffer Write Ready (bit 4)
            if norm_status & NORM_INT_BUF_WR_READY != 0 {  
                // 清除该中断位 (W1C) 
                self.write16(SDHCI_INT_STATUS_NORM, NORM_INT_BUF_WR_READY); // 清除状态位  
                return Ok(());  
            }
            core::hint::spin_loop();  
        }  
        Err(SdioError::Timeout)  
    }

    /// 等待 Transfer Complete 中断状态位  
    fn wait_transfer_complete(&self) -> Result<(), SdioError> {  
        for _ in 0..PIO_TIMEOUT {  
            let norm_status = self.read16(SDHCI_INT_STATUS_NORM);  
            if norm_status & NORM_INT_ERROR != 0 {  
                let err = self.read16(SDHCI_INT_STATUS_ERR);
                self.write16(SDHCI_INT_STATUS_ERR, err); // 清除错误状态位
                self.write16(SDHCI_INT_STATUS_NORM, NORM_INT_ERROR); // 清除错误汇总位;
                log::error!("Transfer error: norm_status=0x{:04x}, err_status=0x{:04x}", norm_status, err);
                return Err(SdioError::IoError);
            }  
            // 检查 Transfer Complete (bit 1)
            if norm_status & NORM_INT_XFER_COMPLETE != 0 {  
                // 清除该中断位 (W1C) 
                self.write16(SDHCI_INT_STATUS_NORM, NORM_INT_XFER_COMPLETE); // 清除状态位  
                return Ok(());  
            }
            core::hint::spin_loop();  
        }  
        Err(SdioError::Timeout)  
    }

    /// 等待 Internal Clock Stable，带超时和 spin_loop hint  
    fn wait_clock_stable(&self) -> Result<(), SdioError> {  
        for _ in 0..CLOCK_STABLE_TIMEOUT {  
            if self.read16(SDHCI_CLOCK_CONTROL) & CC_INT_CLK_STABLE != 0 {  
                return Ok(());  
            }  
            core::hint::spin_loop();  
        }  
        Err(SdioError::Timeout)  
    }

    fn set_clock(&self, hz: u32) -> Result<(), SdioError> {
        // 1. 从 Capabilities Register 读取 base clock
        let caps = self.read32(SDHCI_CAPABILITIES);
        let mut base_clock = ((caps >> 8) & 0xFF) as u32 * 1_000_000; // bits[15:8] = MHz
        if base_clock == 0 {
            // fallback: CVI SoC 默认 50MHz  
            base_clock = 50_000_000u32;
            log::warn!("Capabilities base_clock=0, fallback to 50MHz");  
        }

        // 2. 计算分频值 (确保实际频率 <= 目标频率) 
        let divisor = if hz >= base_clock {
            0u16 // 不分频
        } else {
            // 向上取整保证 base_clock / (2 * divisor) <= hz  
            // 即 divisor >= base_clock / (2 * hz)  
            let div = (base_clock + 2 * hz - 1) / (2 * hz);  
            div.min(0x3FF) as u16  
        };

        // 3. 停止 SD clock output (保留 internal clock 状态，只清除 SD_CLK_EN) 
        let mut clk_reg = self.read16(SDHCI_CLOCK_CONTROL);
        clk_reg &= !(CC_SD_CLK_EN | CC_INT_CLK_EN);
        self.write16(SDHCI_CLOCK_CONTROL, clk_reg);

        // 4. 写入分频值 (10-bit: 高 2 位写入 bits[7:6], 低 8 位写入 bits[15:8])  
        clk_reg &= !(CC_FREQ_SEL_MASK | CC_FREQ_SEL_EXT_MASK);  
        let freq_sel = ((divisor & 0xFF) << CC_DIV_SHIFT as u16) as u16; // 低 8 位  
        let ext_sel = (((divisor >> 8) & 0x03) << CC_EXT_DIV_SHIFT as u16) as u16; // 高 2 位  
        clk_reg |= freq_sel | ext_sel | CC_INT_CLK_EN;  
        self.write16(SDHCI_CLOCK_CONTROL, clk_reg);  

        // 5. 等待 Internal Clock Stable (带超时) 
        self.wait_clock_stable()?;

        // 6. 启用 SD clock output  
        clk_reg = self.read16(SDHCI_CLOCK_CONTROL);  
        self.write16(SDHCI_CLOCK_CONTROL, clk_reg | CC_SD_CLK_EN); 

        let actual_freq = if divisor == 0 {
            base_clock
        } else {
            base_clock / (2 * divisor as u32) 
        };
        log::debug!("SDHCI clock: target={}Hz, divisor={}, actual={}Hz", hz, divisor, actual_freq);  
  
        Ok(()) 
    }

    /// 等待软件复位完成 
    fn wait_reset_complete(&self) -> Result<(), SdioError> {
        for _ in 0..RESET_TIMEOUT {
            if self.read8(SDHCI_SOFTWARE_RESET) == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(SdioError::Timeout)
    }

    /// 读取 CIS 指针 (3 字节, little-endian)  
    /// func=0 从 CCCR 读, func=1..7 从 FBR 读 
    fn read_cis_ptr(&self, func: u8) -> Result<u32, SdioError> {  
        let base = if func == 0 {
            FN0_CIS_PTR
        } else {
            fbr_base(func) + FBR_CIS_PTR_OFFSET
        };
        let b0 = self.cmd52_read(0, base)? as u32;
        let b1 = self.cmd52_read(0, base + 1)? as u32;
        let b2 = self.cmd52_read(0, base + 2)? as u32;
        Ok(b0 | (b1 << 8) | (b2 << 16))
    }

    /// 遍历 CIS tuple 链，查找 CISTPL_MANFID，返回 (vendor_id, device_id) 
    fn read_manfid_from_cis(&self, func: u8) -> Result<(u16, u16), SdioError> {  
        let mut addr = self.read_cis_ptr(func)?;
        for _ in 0..256 {
            let tuple_code = self.cmd52_read(0, addr)?;
            if tuple_code == CISTPL_END {
                break; // 遍历结束
            }
            if tuple_code == CISTPL_NULL {                  
                // NULL tuple 没有 link 字段，直接跳过 1 字节  
                addr += 1;  
                continue;
            }
            let tuple_link = self.cmd52_read(0, addr + 1)? as u32; // link 字段: 后续 tuple 的偏移量
            if tuple_code == CISTPL_MANFID && tuple_link >= 4 {
                let v0 = self.cmd52_read(0, addr + 2)? as u16;
                let v1 = self.cmd52_read(0, addr + 3)? as u16;
                let v2 = self.cmd52_read(0, addr + 4)? as u16;
                let v3 = self.cmd52_read(0, addr + 5)? as u16;
                return Ok((v0 | (v1 << 8), v2 | (v3 << 8))); // 返回 vendor_id 和 device_id
            }
            addr += 2 + tuple_link; // 移动到下一个 tuple
        }

        Err(SdioError::Unsupported) // 没有找到 Manfid
    }
}

impl SdioHost for CviSdhci {
    fn init(&mut self) -> Result<(), SdioError> {
        // ---- SDIO1 SoC 级硬件初始化 ---- 
        // 仅当 base 为 SDIO1 地址时执行 (SDIO0 不需要)  
        const SDIO1_VADDR: usize = 0x0432_0000 + 0xffff_ffc0_0000_0000;
        if self.base == SDIO1_VADDR {
            sdio1_hw_init();
        }

        // 1. 软件复位 (Reset All: CMD + DAT + 全部逻辑)  
        self.write8(SDHCI_SOFTWARE_RESET, SWRST_ALL);
        self.wait_reset_complete()?;

        // ---- SDHCI 标准卡检测覆写 ----  
        // WiFi 模块无物理 CD 引脚, 通过 HOST_CTL1 强制 CARD_INSERTED  
        // bit7: CARD_DET_SEL = 1 (使用 CARD_DET_TEST 而非 SD_CD 引脚)  
        // bit6: CARD_DET_TEST = 1 (卡已插入)  
        if self.base == SDIO1_VADDR {
            let hc1 = self.read8(SDHCI_HOST_CONTROL);
            self.write8(SDHCI_HOST_CONTROL, hc1 | 0xc0); // bit7 + bit6
            log::debug!("SDHCI: Forced card detect via HOST_CTL1"); 
        }

        // 2. 供电 3.3V (必须在启动时钟之前)  
        self.write8(SDHCI_POWER_CONTROL, POWER_330V_ON);

        // 3. 低速时钟 400KHz (SD 规范: 初始化阶段 ≤ 400KHz)  
        self.set_clock(400_000)?;  

        // 4. 使能中断状态位 (轮询模式, 不产生硬件中断信号)  
        self.write16(SDHCI_NORM_INT_STS_EN, NORM_INT_ENABLE_MASK);  
        self.write16(SDHCI_ERR_INT_STS_EN, ERR_INT_ENABLE_MASK); 
        self.write16(SDHCI_NORM_INT_SIG_EN, 0); // 轮询模式: 不触发 IRQ
        self.write16(SDHCI_ERR_INT_SIG_EN, 0);

        // 5. SDIO 卡探测: CMD5 (IO_SEND_OP_COND)  
        //    第一次 CMD5(arg=0): 查询卡支持的 OCR  
        let ocr_query = self.send_cmd(5, 0x0000_0000).map_err(|e| {  
            log::warn!("CMD5 failed: no SDIO card detected or IO error");  
            e  
        })?;   

        let num_io_funcs = ((ocr_query & OCR_IO_FUNC_MASK) >> OCR_IO_FUNC_SHIFT) as u8;  
        let mem_present = (ocr_query & OCR_MEM_PRESENT) != 0;  
        log::info!(  
            "SDIO card: {} IO function(s), memory_present={}",  
            num_io_funcs, mem_present  
        );

        // 选择电压交集: 卡支持的 OCR & 我们想要的电压 (3.2-3.4V)  
        let voltage = ocr_query & OCR_VOLTAGE_MASK & OCR_3V2_3V4;  
        if voltage == 0 {  
            log::error!("No common voltage range with SDIO card");  
            return Err(SdioError::Unsupported);  
        }

        //    第二次 CMD5(arg=voltage): 设置电压，轮询直到 IORDY=1  
        let mut ready = false;  
        for _ in 0..CMD5_READY_TIMEOUT {  
            let ocr_resp = self.send_cmd(5, voltage)?;  
            if ocr_resp & OCR_IORDY != 0 {  
                ready = true;  
                break;  
            }  
            // 卡还在上电，短暂等待后重试  
            for _ in 0..1000 { core::hint::spin_loop(); }  
        }  
        if !ready {
            log::error!("SDIO card not ready after CMD5 polling");  
            return Err(SdioError::Timeout);
        }

        // 6. 获取 RCA: CMD3 
        let resp = self.send_cmd(3, 0)?;
        self.rca = (resp >> 16) as u16;
        log::debug!("SDIO card RCA = 0x{:04x}", self.rca);  

        // 7. 选择卡: CMD7 → Transfer State  
        self.send_cmd(7, (self.rca as u32) << 16)?; 

        // 8. 高速模式切换 (卡侧 + 主机侧) 
        let bus_speed = self.cmd52_read(0, BUS_SPEED_SELECT)?;
        let supports_high_speed = (bus_speed & 0x01) != 0;
        if supports_high_speed {
            // 启用卡侧高速模式 
            self.cmd52_write(0, BUS_SPEED_SELECT, bus_speed | 0x02)?;  
            // 主机侧: Host Control 1 bit2=High Speed Enable 
            let hc1 = self.read8(SDHCI_HOST_CONTROL);
            self.write8(SDHCI_HOST_CONTROL, hc1 | HC_HIGH_SPEED);
            self.set_clock(50_000_000)?;
            log::info!("SDIO: High-Speed 50MHz enabled"); 
        } else {
            self.set_clock(25_000_000)?;  
            log::info!("SDIO: Default Speed 25MHz");
        }

        // 9. 4-bit 总线模式  
        //    卡侧: CCCR Bus Interface Control bits[1:0] = 0b10  
        let bus_if = self.cmd52_read(0, CCCR_BUS_INTERFACE)?;  
        self.cmd52_write(0, CCCR_BUS_INTERFACE, (bus_if & 0xFC) | 0x02)?;  
        // 主机侧: Host Control 1 bit1=4-bit mode 
        let hc1 = self.read8(SDHCI_HOST_CONTROL);
        self.write8(SDHCI_HOST_CONTROL, hc1 | HC_BUS_WIDTH_4);
        log::info!("SDIO: 4-bit bus mode enabled");

        // 10. 使能 SDIO function 1
        self.enable_func(1)?;
        // 等待 IO Ready  
        for _ in 0..1000u32 {  
            let io_ready = self.cmd52_read(0, CCCR_IO_READY)?;  
            if io_ready & 0x02 != 0 { break; }  
            for _ in 0..100 { core::hint::spin_loop(); }  
        } 
        log::info!("SDIO: Function 1 enabled");

        // 11. 设置 function 1 block size = 512 
        self.set_block_size(1, 512)?;
        log::debug!("SDIO: function 1 block size = 512"); 

        // 12. 从 CIS 读取 Vendor/Device ID (CISTPL_MANFID tuple)  
        //     Function 1 的 CIS 包含 aic8800 芯片的 MANFID  
        let (vid, did) = self.read_manfid_from_cis(1).or_else(|_| {
            self.read_manfid_from_cis(0) // 如果 function 1 的 CIS 读取失败，尝试 function 0 的 CIS
        })?;
        self.vendor_id = vid;
        self.device_id = did;
        log::info!(  
            "SDIO card: vendor=0x{:04x}, device=0x{:04x}",  
            self.vendor_id, self.device_id  
        ); 

        Ok(())
    }

    fn read_byte(&self, func: u8, addr: u32) -> Result<u8, SdioError> {
        self.cmd52_read(func, addr)
    }

    fn write_byte(&self, func: u8, addr: u32, val: u8) -> Result<(), SdioError> {
        self.cmd52_write(func, addr, val)
    }

    fn read_fifo(&self, func: u8, addr: u32, buf: &mut [u8]) -> Result<(), SdioError> {
        self.cmd53_read_fixed(func, addr, buf, 512, true)
    }

    fn write_fifo(&self, func: u8, addr: u32, buf: &[u8]) -> Result<(), SdioError> {
        self.cmd53_write_fixed(func, addr, buf, 512, true)
    }

    /// 设置指定 SDIO function 的 block size  
    ///  
    /// Block size 寄存器位置:  
    ///   - Function 0: CCCR 0x10-0x11  
    ///   - Function N (1-7): FBR 0x100*N + 0x10-0x11  
    /// 始终通过 function 0 的 CMD52 访问 (CCCR/FBR 地址空间) 
    fn set_block_size(&self, func: u8, size: u16) -> Result<(), SdioError> {
        if func > 7 {
            return Err(SdioError::Unsupported);
        }
        
        // SDIO block size 合法范围: 1-2048, 推荐 2 的幂  
        if size == 0 || size > 2048 {  
            return Err(SdioError::Unsupported);  
        } 

        let base = 0x100 * (func as u32);  
        // 写低字节  
        self.cmd52_write(0, base + 0x10, (size & 0xFF) as u8)?;  
        // 写高字节  
        self.cmd52_write(0, base + 0x11, ((size >> 8) & 0xFF) as u8)?;  
        // 回读验证  
        let lo = self.cmd52_read(0, base + 0x10)? as u16;  
        let hi = self.cmd52_read(0, base + 0x11)? as u16;  
        let readback = (hi << 8) | lo;  
        if readback != size {  
            log::warn!(  
                "set_block_size: func{} wrote {} but read back {}",  
                func, size, readback  
            );  
            return Err(SdioError::IoError);  
        }  
    
        log::debug!("SDIO func{} block size set to {}", func, size);  
        Ok(())  
    }

    /// 使能指定 SDIO function (1-7)  
    ///  
    /// 写 CCCR IO_ENABLE (0x02) 对应位，然后轮询 IO_READY (0x03) 等待就绪  
    fn enable_func(&self, func: u8) -> Result<(), SdioError> {
        if func == 0 || func > 7 {
            return Err(SdioError::Unsupported);
        }

        // 使能对应 function 位  
        let io_en = self.cmd52_read(0, IO_ENABLE)?;
        self.cmd52_write(0, IO_ENABLE, io_en | (1 << func))?;

        // 轮询等待 IO_READY 位被设置 
        for _ in 0..1000u32 {
            let io_ready = self.cmd52_read(0, CCCR_IO_READY)?;
            if io_ready & (1 << func) != 0 {
                log::info!("SDIO: Function {} enabled and ready", func);
                return Ok(());
            }
            for _ in 0..100_000 { core::hint::spin_loop(); }
        }

        log::error!("SDIO: Function {} not ready after enabling", func);
        Err(SdioError::Timeout)
    }
    
    fn claim_irq(&self, _func: u8, _handler: fn()) -> Result<(), SdioError> {  
        Ok(()) // 轮询模式，暂不实现  
    }  

    fn vendor_device_id(&self) -> (u16, u16) {  
        (self.vendor_id, self.device_id)  
    } 
}