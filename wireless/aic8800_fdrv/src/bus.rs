use alloc::{collections::VecDeque, sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicUsize, Ordering};
use axpoll::PollSet;
use axsync::Mutex;
use kspin::SpinNoIrq;
use sdhci_cv1800::{CviSdhci, mask_unmask_card_irq_raw, regs::*};
use core::ptr::{read_volatile, write_volatile};
use aic8800_sdio::SdioHost;

/// 总线状态
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BusState {
    Down,
    Up,
}

/// TX 帧封装
pub struct TxFrame {
    pub data: Vec<u8>,
    pub priority: u8,
}

/// SDIO 总线共享资源
pub struct WifiBus {
    /// SDHCI 控制器实例（TX/RX 线程共享，通过 Mutex 互斥）
    pub sdio: Arc<Mutex<CviSdhci>>,

    /// SDHCI MMIO 基地址（ISR 裸写用，不经过 Mutex）
    pub sdio_mmio_base: AtomicUsize,

    /// 总线状态
    pub state: SpinNoIrq<BusState>,

    // ---- SDHCI 完成标志（ISR → SDHCI 驱动等待者）---- 
    pub sdhci_cmd_complete: AtomicBool,  
    pub sdhci_xfer_complete: AtomicBool,  
    pub sdhci_buf_rd_ready: AtomicBool,  
    pub sdhci_error_status: AtomicU16,  
    pub sdhci_pollset: PollSet,  

    // ---- RX 侧 ----
    /// RX PollSet：SDIO Card Interrupt → ISR 唤醒 wifi-rx 线程
    pub rx_irq_pollset: PollSet,

    /// RX 帧队列（ISR 不写此队列；wifi-rx 线程读 FIFO 后按类型分发）
    /// 数据帧队列（wifi-rx → NetDevice）
    pub data_rx_queue: SpinNoIrq<VecDeque<Vec<u8>>>,
    pub data_rx_pollset: PollSet,

    /// CMD 响应队列（wifi-rx → CmdMgr）
    pub cmd_rsp_queue: SpinNoIrq<VecDeque<Vec<u8>>>,
    pub cmd_rsp_pollset: PollSet,

    /// TX Confirm 队列（wifi-rx → TX 确认处理）
    pub tx_cfm_queue: SpinNoIrq<VecDeque<Vec<u8>>>,
    pub tx_cfm_pollset: PollSet,

    // ---- TX 侧 ----
    /// TX 帧队列（上层 → wifi-tx 线程）
    pub tx_queue: SpinNoIrq<VecDeque<TxFrame>>,
    pub tx_pktcnt: AtomicU32,
    pub tx_wake_pollset: PollSet,

    /// CMD 发送请求（CmdMgr → wifi-tx 线程）
    pub cmd_pending: SpinNoIrq<Option<Vec<u8>>>,
    pub cmd_pending_flag: AtomicBool,

    /// CmdMgr 错误标志：shutdown 时设为 true，  
    /// send_cmd 等待者被唤醒后检查此标志返回 Err  
    pub cmd_rsp_error: AtomicBool,
}

impl WifiBus {
    pub fn new(sdio: CviSdhci) -> Arc<Self> {
        let base = sdio.mmio_base();
        Arc::new(Self { 
            sdio: Arc::new(Mutex::new(sdio)), 
            sdio_mmio_base: AtomicUsize::new(base),
            state: SpinNoIrq::new(BusState::Down), 
            sdhci_cmd_complete: AtomicBool::new(false),  
            sdhci_xfer_complete: AtomicBool::new(false),  
            sdhci_buf_rd_ready: AtomicBool::new(false),  
            sdhci_error_status: AtomicU16::new(0),  
            sdhci_pollset: PollSet::new(),  
            rx_irq_pollset: PollSet::new(), 
            data_rx_queue: SpinNoIrq::new(VecDeque::new()),
            data_rx_pollset: PollSet::new(),
            cmd_rsp_queue: SpinNoIrq::new(VecDeque::new()), 
            cmd_rsp_pollset: PollSet::new(),
            tx_cfm_queue: SpinNoIrq::new(VecDeque::new()), 
            tx_cfm_pollset: PollSet::new(),
            tx_queue: SpinNoIrq::new(VecDeque::new()),
            tx_pktcnt: AtomicU32::new(0), 
            tx_wake_pollset: PollSet::new(), 
            cmd_pending: SpinNoIrq::new(None), 
            cmd_pending_flag: AtomicBool::new(false), 
            cmd_rsp_error: AtomicBool::new(false),
        })
    }

    /// 关闭总线，停止所有线程
    pub fn shutdown(self: &Arc<Self>) {
        // 1. 设置 BUS_DOWN（线程循环会检测此状态并退出） 
        *self.state.lock() = BusState::Down;

        // 2. 禁用 AIC8800 芯片端 SDIO 中断  
        {
            let sdio = self.sdio.lock();
            let _ = sdio.write_byte(1, SDIOWIFI_INTR_CONFIG_REG, 0x00);
            sdio.disable_irq();
        }

        // 3. 唤醒 TX 线程，等待其退出  
        //    TX 线程检测 BUS_DOWN 后会停止发送并退出  
        self.tx_wake_pollset.wake();  
        // flush TX 队列（TX 线程退出后不会再访问）  
        self.tx_queue.lock().clear();  
    
        // 4. 唤醒 RX 线程，等待其退出  
        self.rx_irq_pollset.wake();  
    
        // 5. flush RX 相关队列  
        self.data_rx_queue.lock().clear();  
    
        // 6. 唤醒所有 CMD 等待者并标记错误  
        self.cmd_rsp_error.store(true, Ordering::Release);  
        self.cmd_rsp_pollset.wake();  
        self.tx_cfm_pollset.wake();  
        self.sdhci_pollset.wake();

        clear_global_bus();
    
        log::info!("[wifi-bus] shutdown: interrupts disabled, threads notified"); 
    }
}

/// 全局 WifiBus 引用（init 后设置，ISR 读取）
static WIFI_BUS_PTR: AtomicUsize = AtomicUsize::new(0);

pub fn set_global_bus(bus: &Arc<WifiBus>) {
    let ptr = Arc::into_raw(Arc::clone(bus)); // refcount + 1
    let old = WIFI_BUS_PTR.swap(ptr as usize, Ordering::AcqRel);
    if old != 0 {
        // 释放旧的引用  
        unsafe { Arc::from_raw(old as *const WifiBus); }  
    }
}

pub fn get_global_bus() -> Option<&'static WifiBus> {
    let ptr = WIFI_BUS_PTR.load(Ordering::Acquire);
    if ptr == 0 { 
        None 
    } else {
        unsafe { 
            Some(&*(ptr as *const WifiBus)) 
        } 
    }
}

/// shutdown 时调用，释放全局引用  
pub fn clear_global_bus() {
    let old = WIFI_BUS_PTR.swap(0, Ordering::AcqRel);
    if old != 0 {
        unsafe { Arc::from_raw(old as *const WifiBus); }
    }
}

/// PLIC IRQ #38 处理函数
///
/// 约束：不持锁、不分配堆、不调度。仅操作 Atomic + MMIO 裸写 + waker.wake()
pub fn sdio1_irq_handler() {
    let Some(bus) = get_global_bus() else {
        return;
    };
    let base = bus.sdio_mmio_base.load(Ordering::Acquire);
    if base == 0 { return; }
    
    // 读 NORM_AND_ERR_INT_STS (offset 0x030)
    let status = unsafe {
        read_volatile((base + SDHCI_INT_STATUS_NORM as usize) as *const u32)
    };

    if status == 0 { return; }

    // CARD_INT (bit 8): AIC8800 有数据要发给主机
    if status & (NORM_INT_CARD_INT as u32) != 0 {
        // 屏蔽 CARD_INT 信号，防止重复触发（电平触发）
        mask_unmask_card_irq_raw(base, true);
        // 唤醒 wifi-rx 线程
        bus.rx_irq_pollset.wake();
    }

    // CMD_CMPL (bit 0)
    if status & (NORM_INT_CMD_COMPLETE as u32) != 0 {
        bus.sdhci_cmd_complete.store(true, Ordering::Release);
    }

    // XFER_CMPL (bit 1)
    if status & (NORM_INT_XFER_COMPLETE as u32) != 0 {
        bus.sdhci_xfer_complete.store(true, Ordering::Release);
    }

    // BUF_RD_READY (bit 5)
    if status & (NORM_INT_BUF_RD_READY as u32) != 0 {
        bus.sdhci_buf_rd_ready.store(true, Ordering::Release);
    }

    // ERR_INT (bit 15)
    if status & (NORM_INT_ERROR as u32) != 0 {
        let err = unsafe {
            read_volatile((base + SDHCI_INT_STATUS_ERR as usize) as *const u16)
        };
        bus.sdhci_error_status.store(err, Ordering::Release);
        // W1C 清除 ERR_INT_STS  
        unsafe {
            write_volatile(
                (base + SDHCI_INT_STATUS_ERR as usize) as *mut u16, 
                err,
            );
        }
    }
    // W1C 清除所有非 CARD_INT 的 Normal 状态位
    //   CARD_INT 是电平触发，不能通过 W1C 清除，只能通过 mask 屏蔽 
    let w1c_mask = (status & !(NORM_INT_CARD_INT as u32)) as u16;  
    if w1c_mask != 0 {  
        unsafe {  
            write_volatile(  
                (base + SDHCI_INT_STATUS_NORM as usize) as *mut u16,  
                w1c_mask  
            );  
        }  
    }  
    // 唤醒 SDHCI 等待者（CMD/XFER/BUF_RD/ERR 共用）  
    if w1c_mask != 0 {
        bus.sdhci_pollset.wake();
    }
}