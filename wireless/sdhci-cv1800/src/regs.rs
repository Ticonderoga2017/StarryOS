// ============================================================  
// SDHCI 标准寄存器偏移量 (SD Host Controller Spec v3.0)  
// ============================================================  
  
pub const SDHCI_DMA_ADDRESS:     u32 = 0x00;  
pub const SDHCI_BLOCK_SIZE:      u32 = 0x04;  
pub const SDHCI_BLOCK_COUNT:     u32 = 0x06;  
pub const SDHCI_ARGUMENT:        u32 = 0x08;  
pub const SDHCI_TRANSFER_MODE:   u32 = 0x0C;  
pub const SDHCI_COMMAND:         u32 = 0x0E;  
pub const SDHCI_RESPONSE:        u32 = 0x10;  // 0x10-0x1F (4 x 32-bit)  
pub const SDHCI_BUFFER:          u32 = 0x20;  
pub const SDHCI_PRESENT_STATE:   u32 = 0x24;  
pub const SDHCI_HOST_CONTROL:    u32 = 0x28;  
pub const SDHCI_POWER_CONTROL:   u32 = 0x29;  
pub const SDHCI_BLOCK_GAP_CTRL:  u32 = 0x2A;  
pub const SDHCI_WAKEUP_CTRL:     u32 = 0x2B;  
pub const SDHCI_CLOCK_CONTROL:   u32 = 0x2C;  
pub const SDHCI_TIMEOUT_CONTROL: u32 = 0x2E;  
pub const SDHCI_SOFTWARE_RESET:  u32 = 0x2F;  
  
// ---- 中断寄存器 (16-bit 分离访问) ----  
pub const SDHCI_INT_STATUS_NORM: u32 = 0x30;  // Normal Interrupt Status  
pub const SDHCI_INT_STATUS_ERR:  u32 = 0x32;  // Error Interrupt Status  
pub const SDHCI_NORM_INT_STS_EN: u32 = 0x34;  // Normal Interrupt Status Enable  
pub const SDHCI_ERR_INT_STS_EN:  u32 = 0x36;  // Error Interrupt Status Enable  
pub const SDHCI_NORM_INT_SIG_EN: u32 = 0x38;  // Normal Interrupt Signal Enable  
pub const SDHCI_ERR_INT_SIG_EN:  u32 = 0x3A;  // Error Interrupt Signal Enable  
  
// ---- 中断寄存器 (32-bit 合并访问别名) ----  
// 32-bit 写入 0x034 同时设置 Normal(低16) + Error(高16) Status Enable  
// 32-bit 写入 0x038 同时设置 Normal(低16) + Error(高16) Signal Enable  
pub const SDHCI_INT_STATUS_EN:   u32 = 0x34;  
pub const SDHCI_INT_SIGNAL_EN:   u32 = 0x38;  
  
pub const SDHCI_AUTO_CMD_ERR:    u32 = 0x3C;  // Auto CMD12 Error Status  
pub const SDHCI_HOST_CTRL2:      u32 = 0x3E;  // Host Control 2  
pub const SDHCI_CAPABILITIES:    u32 = 0x40;  
pub const SDHCI_CAPABILITIES_1:  u32 = 0x44;  // Capabilities Extended  
pub const SDHCI_MAX_CURRENT:     u32 = 0x48;  
pub const SDHCI_HOST_VERSION:    u32 = 0xFE;  // Host Controller Version  
  
// ============================================================  
// Present State Register (0x24) 位定义 (32-bit)  
// ============================================================  
  
pub const SDHCI_CMD_INHIBIT:     u32 = 1 << 0;  
pub const SDHCI_DATA_INHIBIT:    u32 = 1 << 1;  
pub const SDHCI_DAT_ACTIVE:      u32 = 1 << 2;  
pub const SDHCI_WR_ACTIVE:       u32 = 1 << 8;  
pub const SDHCI_RD_ACTIVE:       u32 = 1 << 9;  
pub const SDHCI_BUF_WR_EN:       u32 = 1 << 10;  
pub const SDHCI_BUF_RD_EN:       u32 = 1 << 11;  
pub const SDHCI_CARD_INSERTED:   u32 = 1 << 16;  
pub const SDHCI_CARD_STABLE:     u32 = 1 << 17;  
  
// ============================================================  
// Normal Interrupt Status (0x30) 位定义 (16-bit)  
// ============================================================  
  
pub const NORM_INT_CMD_COMPLETE:   u16 = 1 << 0;  
pub const NORM_INT_XFER_COMPLETE:  u16 = 1 << 1;  
pub const NORM_INT_BLK_GAP:        u16 = 1 << 2;  
pub const NORM_INT_DMA:            u16 = 1 << 3;  
pub const NORM_INT_BUF_WR_READY:   u16 = 1 << 4;  
pub const NORM_INT_BUF_RD_READY:   u16 = 1 << 5;  
pub const NORM_INT_CARD_INSERT:    u16 = 1 << 6;  
pub const NORM_INT_CARD_REMOVAL:   u16 = 1 << 7;  
pub const NORM_INT_CARD_INT:       u16 = 1 << 8;   // SDIO Card Interrupt  
pub const NORM_INT_ERROR:          u16 = 1 << 15;  // Error Interrupt 汇总位  
  
// ---- 32-bit 合并读取 (0x030) 时的 Error 位 (bit 16~31) ----  
pub const NORM_INT_CMD_TOUT_ERR:   u32 = 1 << 16;  
pub const NORM_INT_CMD_CRC_ERR:    u32 = 1 << 17;  
pub const NORM_INT_DAT_TOUT_ERR:   u32 = 1 << 20;  
pub const NORM_INT_DAT_CRC_ERR:    u32 = 1 << 21;  
  
// ============================================================  
// Error Interrupt Status (0x32) 位定义 (16-bit, 从 bit 0 开始)  
// ============================================================  
  
pub const ERR_INT_CMD_TIMEOUT:     u16 = 1 << 0;  
pub const ERR_INT_CMD_CRC:         u16 = 1 << 1;  
pub const ERR_INT_CMD_END_BIT:     u16 = 1 << 2;  
pub const ERR_INT_CMD_INDEX:       u16 = 1 << 3;  
pub const ERR_INT_DAT_TIMEOUT:     u16 = 1 << 4;  
pub const ERR_INT_DAT_CRC:         u16 = 1 << 5;  
pub const ERR_INT_DAT_END_BIT:     u16 = 1 << 6;  
pub const ERR_INT_CUR_LIMIT:       u16 = 1 << 7;  
pub const ERR_INT_AUTO_CMD:        u16 = 1 << 8;  
pub const ERR_INT_ADMA:            u16 = 1 << 9;  
pub const ERR_INT_TUNING:          u16 = 1 << 10;  
pub const ERR_INT_BOOT_ACK:        u16 = 1 << 12;  
  
// ============================================================  
// 组合掩码 (16-bit)  
// ============================================================  
  
pub const NORM_INT_ENABLE_MASK: u16 = NORM_INT_CMD_COMPLETE | NORM_INT_XFER_COMPLETE  
    | NORM_INT_BUF_WR_READY | NORM_INT_BUF_RD_READY  
    | NORM_INT_CARD_INSERT | NORM_INT_CARD_REMOVAL  
    | NORM_INT_CARD_INT;  
  
pub const ERR_INT_ENABLE_MASK: u16 = ERR_INT_CMD_TIMEOUT | ERR_INT_CMD_CRC  
    | ERR_INT_CMD_END_BIT | ERR_INT_CMD_INDEX  
    | ERR_INT_DAT_TIMEOUT | ERR_INT_DAT_CRC | ERR_INT_DAT_END_BIT;  
  
pub const ERR_INT_CMD_MASK: u16 = ERR_INT_CMD_TIMEOUT | ERR_INT_CMD_CRC  
    | ERR_INT_CMD_END_BIT | ERR_INT_CMD_INDEX;  
  
pub const ERR_INT_DAT_MASK: u16 = ERR_INT_DAT_TIMEOUT | ERR_INT_DAT_CRC  
    | ERR_INT_DAT_END_BIT;  
  
// ---- 32-bit 合并掩码 ----  
pub const NORM_INT_ERR_ALL: u32 = NORM_INT_CMD_TOUT_ERR | NORM_INT_CMD_CRC_ERR  
    | NORM_INT_DAT_TOUT_ERR | NORM_INT_DAT_CRC_ERR;  
  
pub const NORM_INT_ALL_NEEDED: u32 = (NORM_INT_CMD_COMPLETE | NORM_INT_XFER_COMPLETE  
    | NORM_INT_BUF_RD_READY | NORM_INT_CARD_INT | NORM_INT_ERROR) as u32  
    | NORM_INT_ERR_ALL;  
  
// ============================================================  
// 中断信号使能掩码（用于 NORM_INT_SIG_EN / ERR_INT_SIG_EN）  
// ============================================================  
  
/// SDHCI Normal 中断信号使能：CMD_CMPL + XFER_CMPL + BUF_WR_READY + BUF_RD_READY + CARD_INT  
pub const NORM_INT_SIG_MASK: u16 = NORM_INT_CMD_COMPLETE  
    | NORM_INT_XFER_COMPLETE  
    | NORM_INT_BUF_WR_READY  
    | NORM_INT_BUF_RD_READY  
    | NORM_INT_CARD_INT;  
  
/// Error 中断信号使能  
pub const ERR_INT_SIG_MASK: u16 = ERR_INT_CMD_MASK | ERR_INT_DAT_MASK;  
  
// ============================================================  
// Clock Control Register (0x2C) 位定义 (16-bit)  
// ============================================================  
  
pub const CC_INT_CLK_EN:        u16 = 0x0001;  
pub const CC_INT_CLK_STABLE:    u16 = 0x0002;  
pub const CC_SD_CLK_EN:         u16 = 0x0004;  
pub const CC_CLK_GEN_SEL:       u16 = 0x0020;  
pub const CC_FREQ_SEL_EXT_MASK: u16 = 0x00C0;  // bits[7:6]: 分频器高 2 位 (v3.0)  
pub const CC_FREQ_SEL_MASK:     u16 = 0xFF00;  // bits[15:8]: 分频器低 8 位  
pub const CC_DIV_SHIFT:         u32 = 8;  
pub const CC_EXT_DIV_SHIFT:     u32 = 6;  
  
// ============================================================  
// Software Reset Register (0x2F) 位定义 (8-bit)  
// ============================================================  
  
pub const SWRST_ALL:            u8 = 0x01;  
pub const SWRST_CMD_LINE:       u8 = 0x02;  
pub const SWRST_DAT_LINE:       u8 = 0x04;  
/// SOFTWARE_RESET_DAT 别名 (与 SWRST_DAT_LINE 相同)  
pub const SOFTWARE_RESET_DAT:   u8 = 0x04;  
  
// ============================================================  
// Power Control Register (0x29) 位定义 (8-bit)  
// ============================================================  
  
pub const POWER_ON:       u8 = 0x01;       // bit 0: SD Bus Power  
pub const POWER_VSEL_33V: u8 = 0x07 << 1;  // bits[3:1] = 111b: 3.3V  
pub const POWER_VSEL_30V: u8 = 0x06 << 1;  // bits[3:1] = 110b: 3.0V  
pub const POWER_VSEL_18V: u8 = 0x05 << 1;  // bits[3:1] = 101b: 1.8V  
pub const POWER_330V_ON:  u8 = POWER_ON | POWER_VSEL_33V; // 0x0F  
  
// ============================================================  
// Host Control 1 (0x28) 位定义 (8-bit)  
// ============================================================  
  
pub const HC_HIGH_SPEED:   u8 = 0x04;  // bit 2  
pub const HC_BUS_WIDTH_4:  u8 = 0x02;  // bit 1  
  
// ============================================================  
// 超时常量  
// ============================================================  
  
pub const RESET_TIMEOUT:        u32 = 100_000;  
pub const CLOCK_STABLE_TIMEOUT: u32 = 100_000;  
pub const CMD_RESPONSE_TIMEOUT: u32 = 100_000;  
pub const CMD5_READY_TIMEOUT:   u32 = 1_000;  
/// PIO 数据传输超时 (循环次数)  
pub const PIO_TIMEOUT:          u32 = 1_000_000;  
  
// ############################################################  
//  
//  AIC8800 SDIO WiFi 寄存器定义  
//  
// ############################################################  
  
// ============== AIC8801 / AIC8800DC / AIC8800DW (V1/V2) ==============  
  
/// SDIO 块大小 (512 bytes)  
pub const SDIOWIFI_FUNC_BLOCKSIZE: usize = 512;  
  
/// Byte mode 长度寄存器  
pub const SDIOWIFI_BYTEMODE_LEN_REG: u32 = 0x02;  
  
/// 中断配置寄存器 (写 0x07 使能, 写 0x00 禁用)  
pub const SDIOWIFI_INTR_CONFIG_REG: u32 = 0x04;  
  
/// 睡眠控制寄存器  
pub const SDIOWIFI_SLEEP_REG: u32 = 0x05;  
  
/// 唤醒控制寄存器  
pub const SDIOWIFI_WAKEUP_REG: u32 = 0x09;  
  
/// Flow control 寄存器 (读取可用 credits)  
pub const SDIOWIFI_FLOW_CTRL_REG: u32 = 0x0A;  
  
/// Register block 控制  
pub const SDIOWIFI_REGISTER_BLOCK: u32 = 0x0B;  
  
/// Byte mode 使能寄存器 (1 = 禁用 byte mode)  
pub const SDIOWIFI_BYTEMODE_ENABLE_REG: u32 = 0x11;  
  
/// 待读块计数寄存器 (读取固件待发数据的块数)  
pub const SDIOWIFI_BLOCK_CNT_REG: u32 = 0x12;  
  
/// Flow control 掩码寄存器地址  
/// Linux: SDIOWIFI_FLOWCTRL_MASK_REG = 0x7F  
pub const SDIOWIFI_FLOWCTRL_MASK_REG: u32 = 0x7F;  
  
/// 写 FIFO 地址 (CMD53 multi-byte write 目标)  
pub const SDIOWIFI_WR_FIFO_ADDR: u32 = 0x07;  
  
/// 读 FIFO 地址 (CMD53 multi-byte read 目标)  
pub const SDIOWIFI_RD_FIFO_ADDR: u32 = 0x08;  
  
// ============== AIC8800D80 / AIC8800D80X2 (V3) ==============  
  
pub const SDIOWIFI_INTR_ENABLE_REG_V3: u32 = 0x00;  
pub const SDIOWIFI_INTR_PENDING_REG_V3: u32 = 0x01;  
pub const SDIOWIFI_INTR_TO_DEVICE_REG_V3: u32 = 0x02;  
pub const SDIOWIFI_FLOW_CTRL_Q1_REG_V3: u32 = 0x03;  
pub const SDIOWIFI_MISC_INT_STATUS_REG_V3: u32 = 0x04;  
pub const SDIOWIFI_BYTEMODE_LEN_REG_V3: u32 = 0x05;  
pub const SDIOWIFI_BYTEMODE_LEN_MSB_REG_V3: u32 = 0x06;  
pub const SDIOWIFI_BYTEMODE_ENABLE_REG_V3: u32 = 0x07;  
pub const SDIOWIFI_MISC_CTRL_REG_V3: u32 = 0x08;  
pub const SDIOWIFI_FLOW_CTRL_Q2_REG_V3: u32 = 0x09;  
pub const SDIOWIFI_CLK_TEST_RESULT_REG_V3: u32 = 0x0A;  
pub const SDIOWIFI_RD_FIFO_ADDR_V3: u32 = 0x0F;  
pub const SDIOWIFI_WR_FIFO_ADDR_V3: u32 = 0x10;  
  
// ============== 时钟控制 ==============  
  
pub const SDIOCLK_FREE_RUNNING_BIT: u8 = 1 << 6;  
  
// ============== SDIO 帧类型标识 ==============  
  
/// 数据帧  
pub const SDIO_TYPE_DATA: u8 = 0x00;  
/// 配置帧 (通用 / 类型判断 mask)  
pub const SDIO_TYPE_CFG: u8 = 0x10;  
/// 配置帧 - 命令响应 (CMD CFM)  
pub const SDIO_TYPE_CFG_CMD_RSP: u8 = 0x11;  
/// 配置帧 - 数据确认 (TX CFM)  
pub const SDIO_TYPE_CFG_DATA_CFM: u8 = 0x12;  
/// 配置帧 - 固件 print 输出  
pub const SDIO_TYPE_CFG_PRINT: u8 = 0x13;  
  
// ============== Flow control ==============  
  
/// flow_ctrl_reg 中可用 credits 的掩码 (7-bit, bit7 保留)  
/// 注意: Linux fdrv 中 SDIOWIFI_FLOWCTRL_MASK_REG=0x7F 是寄存器地址,  
///       但实际 flow_ctrl 值也是 7-bit (bit7 = other_interrupt flag)  
pub const SDIOWIFI_FLOWCTRL_MASK: u8 = 0x7F;  
  
/// BLOCK_CNT_REG 中的 "其他中断" 标志位 (bit7)  
pub const SDIO_OTHER_INTERRUPT: u8 = 0x80;  
  
/// Flow control 数据帧阈值 (credits <= 此值时暂停发送)  
/// Linux: DATA_FLOW_CTRL_THRESH = 2  
pub const DATA_FLOW_CTRL_THRESH: u8 = 2;  
  
// ============== 电源管理 ==============  
  
/// SDIO 电源控制间隔 (ms)  
/// Linux: SDIOWIFI_PWR_CTRL_INTERVAL = 30  
pub const SDIOWIFI_PWR_CTRL_INTERVAL: u32 = 30;  
  
/// SDIO 睡眠状态  
pub const SDIO_SLEEP_ST: u8 = 0;  
/// SDIO 活跃状态  
pub const SDIO_ACTIVE_ST: u8 = 1;  
  
// ============== 杂项常量 ==============  
  
/// Flow control 重试次数  
/// Linux: FLOW_CTRL_RETRY_COUNT = 50  
pub const FLOW_CTRL_RETRY_COUNT: u32 = 50;  
  
/// 单帧最大缓冲区大小  
/// Linux: BUFFER_SIZE = 1536  
pub const CMD_BUF_MAX: usize = 1536;  
  
/// TX 块大小 (与 SDIOWIFI_FUNC_BLOCKSIZE 相同)  
pub const TXPKT_BLOCKSIZE: usize = 512;  
  
/// TX 对齐要求  
pub const TX_ALIGNMENT: usize = 4;  
  
/// TX 队列最大长度  
/// Linux: TXQLEN = 2048 * 4  
pub const TXQLEN: usize = 2048 * 4;