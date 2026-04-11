use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;
use crate::{bus::WifiBus, tx_thread};

pub struct AicWifiDevice {
    bus: Arc<WifiBus>,
}

impl AicWifiDevice {
    pub fn new(bus: Arc<WifiBus>) -> Self {
        Self { bus }
    }

    pub fn mac_address(&self) -> Option<[u8; 6]> {
        self.bus.connected_sta_mac.lock().clone()
    }

    /// 发送以太网帧（DA+SA+ethertype+payload）
    pub fn transmit(&self, eth_frame: Vec<u8>) -> Result<(), ()> {
        tx_thread::enqueue_data_frame(&self.bus, eth_frame).map_err(|_|())
    }

    /// 接收以太网帧（非阻塞，返回 None 表示无数据）
    pub fn receive(&self) -> Option<Vec<u8>> {
        self.bus.data_rx_queue.lock().pop_front()
    }

    /// 是否有待接收数据
    pub fn can_receive(&self) -> bool {
        !self.bus.data_rx_queue.lock().is_empty()
    }

    /// 是否已连接
    pub fn is_connected(&self) -> bool {
        self.bus.connected_vif_idx.load(Ordering::Acquire) != 0xFF
    }
}