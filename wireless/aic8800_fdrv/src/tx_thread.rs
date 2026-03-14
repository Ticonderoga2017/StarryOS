use aic8800_sdio::SdioHost;
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::vec;
use core::{future::poll_fn, sync::atomic::Ordering};
use core::task::Poll;

use axtask::future::block_on;
use log;

use crate::bus::{BusState, TxFrame, WifiBus};
use sdhci_cv1800::{mask_unmask_card_irq_raw, regs::*};

const TAIL_LEN: usize = 4;   
const BUFFER_SIZE: usize = 1536;  
const FLOW_CTRL_CMD_RETRY: u32 = 10;  
/// 每次 tx_process 最多处理的数据帧数，防止饿死其他任务  
const TX_BATCH_LIMIT: u32 = 16;  
const MAX_TX_QUEUE_LEN: usize = 256;

#[derive(Debug)]
pub enum TxError {
    QueueFull,
}

fn align_up(val: usize, align: usize) -> usize {
    (val + align - 1) & !(align - 1)
}

fn has_pending_work(bus: &WifiBus) -> bool {
    bus.cmd_pending_flag.load(Ordering::Acquire)
        || bus.tx_pktcnt.load(Ordering::Acquire) > 0
}

/// 对 CMD 帧做 TX_ALIGNMENT + TAIL_LEN + BLOCK_SIZE 对齐  
fn pad_cmd_frame(cmd: &mut Vec<u8>) -> usize {
    // Step 1: TX_ALIGNMENT (4 字节) 对齐  
    let aligned = align_up(cmd.len(), TX_ALIGNMENT);
    cmd.resize(aligned, 0);

    // Step 2: 如果不是 BLOCK_SIZE 整数倍，追加 TAIL_LEN  
    if cmd.len() % SDIOWIFI_FUNC_BLOCKSIZE != 0 {
        cmd.extend_from_slice(&[0u8; TAIL_LEN]);
    }

    // Step 3: 向上取整到 BLOCK_SIZE 
    let final_len = align_up(cmd.len(), SDIOWIFI_FUNC_BLOCKSIZE);
    cmd.resize(final_len, 0);
    final_len
}

/// 启动 wifi-tx 线程
pub fn start(bus: Arc<WifiBus>) {
    axtask::spawn_with_name(
        move || {
            log::info!("[wifi-tx] thread started");
            block_on(poll_fn(|cx| {
                // 检查总线状态
                if *bus.state.lock() == BusState::Down {
                    return Poll::Ready(());
                }

                // 处理所有待发帧
                let did_work = tx_process(&bus);

                // 注册 waker
                bus.tx_wake_pollset.register(cx.waker());

                // 双重检查
                if did_work || has_pending_work(&bus) {
                    cx.waker().wake_by_ref();
                }

                bus.cmd_rsp_pollset.wake();
 
                Poll::Pending
            }))
        },
        "wifi-tx".into(),
    );
}

/// TX 处理主逻辑
/// 
/// 1. CMD 优先：如果有 cmd_pending，先发 CMD
/// 2. Data：检查 flow_ctrl → dequeue → 构造帧 → send
fn tx_process(bus: &WifiBus) -> bool {
    let mut did_work = false;

    // 检查总线状态 
    if *bus.state.lock() == BusState::Down {
        return false;
    }

    // ---- Step 1: CMD 优先 ----
    if bus.cmd_pending_flag.load(Ordering::Acquire) {
        let cmd_buf = bus.cmd_pending.lock().take();
        if let Some(mut cmd) = cmd_buf {
            bus.cmd_pending_flag.store(false, Ordering::Release);

            // 对 CMD 帧做对齐（对应 Linux aicwf_sdio_tx_msg 的 alignment + tail）
            let send_len = pad_cmd_frame(&mut cmd);      

            let base = bus.sdio_mmio_base.load(Ordering::Acquire);
            if base != 0 {
                mask_unmask_card_irq_raw(base, true);
            }      

            log::info!("[wifi-tx] CMD frame ready, send_len={}", send_len); 

            // Flow control for CMD（对应 Linux aicwf_sdio_tx_msg 中的 flow_ctrl_msg）
            let mut fc_ok = false;
            {
                // 持锁发送 CMD
                let mut sdio = bus.sdio.lock();
                for retry in 0..FLOW_CTRL_CMD_RETRY {
                    match sdio.read_byte(1, SDIOWIFI_FLOW_CTRL_REG) {
                        Ok(fc) => {
                            let fc_val = fc & SDIOWIFI_FLOWCTRL_MASK;
                            log::info!(  
                                "[wifi-tx] CMD flow_ctrl: raw=0x{:02x}, credits={}, retry={}",  
                                fc, fc_val, retry  
                            ); 
                            if fc_val != 0 {
                                // 额外检查：buffer_cnt * BUFFER_SIZE 必须 > send_len  
                                // 对应 Linux: buffer_cnt > 0 && len < (buffer_cnt * BUFFER_SIZE) 
                                if (fc_val as usize) * BUFFER_SIZE > send_len {
                                    fc_ok = true;
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            log::error!("[wifi-tx] CMD flow_ctrl read err: {:?}", e);  
                            break;
                        }
                    }
                    // 让出锁和 CPU 再重试  
                    drop(sdio);  
                    for _ in 0..10_000 { core::hint::spin_loop(); }   
                    sdio = bus.sdio.lock();  
                }

                // flow_ctrl 通过后，在同一个锁内直接 write_fifo  
                if fc_ok {  
                    log::info!("[wifi-tx] calling write_fifo...");  
                    if let Err(e) = sdio.write_fifo(1, SDIOWIFI_WR_FIFO_ADDR, &cmd) {  
                        log::error!("[wifi-tx] CMD write_fifo failed: {:?}", e);  
                    } else {  
                        log::info!("[wifi-tx] CMD write_fifo OK");  
                        did_work = true;  
                    }  
                }  
            }

            if fc_ok && did_work {
                // 先 wake RX 线程（CARD_INT 仍然 masked，ISR 不会触发，  
                //   wake() 内部的 log::info! 不会被 ISR 打断） 
                bus.rx_irq_pollset.wake();

                // 最后才 unmask CARD_INT  
                //   此后不再有任何 log 调用，ISR 触发后只设 flag 不调 wake() 
                if base != 0 {
                    mask_unmask_card_irq_raw(base, false);
                }
            } else if !fc_ok {
                log::error!("[wifi-tx] CMD flow_ctrl timeout, dropping CMD");  
                bus.cmd_rsp_error.store(true, Ordering::Release);  
                bus.cmd_rsp_pollset.wake();  
                // unmask CARD_INT 恢复原状  
                if base != 0 {  
                    mask_unmask_card_irq_raw(base, false);  
                } 
            } else {
                // write_fifo 失败，也要 unmask  
                if base != 0 {  
                    mask_unmask_card_irq_raw(base, false);  
                }
            }
        }
    }

    // ---- Step 2: Data 发送（对应 Linux aicwf_sdio_bustx_thread 的数据处理）----
    // 检查有帧可发
    let mut batch_count: u32 = 0;  
    while bus.tx_pktcnt.load(Ordering::Acquire) > 0 {
        // Batch limit：防止饿死其他任务
        if batch_count >= TX_BATCH_LIMIT {
            break;
        }

        // CMD 优先：如果有 CMD 待发，中断数据循环
        if bus.cmd_pending_flag.load(Ordering::Acquire) {
            break;
        }

        // 检查总线状态  
        if *bus.state.lock() == BusState::Down {  
            break;  
        }         

        let sdio = bus.sdio.lock();

        // 检查 flow control credits
        let fc = match sdio.read_byte(1, SDIOWIFI_FLOW_CTRL_REG) {
            Ok(v) => v & SDIOWIFI_FLOWCTRL_MASK,
            Err(_) => break,
        };

        if batch_count < 3 {  
            log::debug!("[wifi-tx] DATA flow_ctrl: credits={}", fc);  
        }  

        if fc <= DATA_FLOW_CTRL_THRESH {
            // credits 不足，等下次唤醒  
            break;
        }

        // 释放 SDIO 锁再操作队列（避免持锁时间过长）  
        drop(sdio);  

        // 从队列取帧
        let frame = bus.tx_queue.lock().pop_front();
        let Some(frame) = frame else {
            break;
        };
        bus.tx_pktcnt.fetch_sub(1, Ordering::AcqRel);

        // 构造 sdio_header + payload
        let total_len = frame.data.len() + 4;
        let aligned_len = align_up(total_len, TX_ALIGNMENT);

        // TAIL_LEN + BLOCK_SIZE 对齐
        let final_len = if aligned_len % SDIOWIFI_FUNC_BLOCKSIZE != 0 {
            align_up(aligned_len + TAIL_LEN, SDIOWIFI_FUNC_BLOCKSIZE)
        } else {
            aligned_len
        };

        let mut buf = vec![0u8; final_len];
        // sdio_header: [len_lo, len_hi | flags, type, reserved]  
        buf[0] = (total_len & 0xFF) as u8;
        buf[1] = ((total_len >> 8) & 0x0F) as u8;
        buf[2] = SDIO_TYPE_DATA;
        buf[3] = 0x00; // AIC8801: reserved=0; AIC8800D80: 需要 CRC8 
        buf[4..4 + frame.data.len()].copy_from_slice(&frame.data);

        // 重新获取 SDIO 锁发送  
        let sdio = bus.sdio.lock(); 
        if let Err(e) = sdio.write_fifo(1, SDIOWIFI_WR_FIFO_ADDR, &buf) {
            log::error!("[wifi-tx] DATA write_fifo failed: {:?}", e);  
        } else {
            // 发送后 unmask CARD_INT  
            let base = bus.sdio_mmio_base.load(Ordering::Acquire);  
            if base != 0 {  
                mask_unmask_card_irq_raw(base, false);  
            }  
            bus.rx_irq_pollset.wake();  
        }
        did_work = true;    
        batch_count += 1;
    }
    did_work
}

/// 将以太网帧入队 TX 队列
pub fn enqueue_data_frame(
    bus: &Arc<WifiBus>,
    eth_frame: Vec<u8>
) -> Result<(), TxError> {
    let mut queue = bus.tx_queue.lock();
    if queue.len() >= MAX_TX_QUEUE_LEN {
        return Err(TxError::QueueFull);
    }

    queue.push_back(TxFrame {
        data: eth_frame,
        priority: 0, // 默认优先级，后续可按 802.1p/DSCP 分类 
    });
    drop(queue);

    bus.tx_pktcnt.fetch_add(1, Ordering::AcqRel);
    bus.tx_wake_pollset.wake();
    Ok(())
}
