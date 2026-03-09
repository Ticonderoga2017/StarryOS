#![no_std]
#![no_main]
#![doc = include_str!("../README.md")]

#[macro_use]
extern crate axlog;

extern crate alloc;
extern crate axruntime;

use alloc::{borrow::ToOwned, vec::Vec};

use axfs::FS_CONTEXT;

mod entry;

pub const CMDLINE: &[&str] = &["/bin/sh", "-c", include_str!("init.sh")];

#[unsafe(no_mangle)]
fn main() {
    starry_api::init();

    // #[cfg(feature = "sg2002")]
    // {
    //     // wireless 完整初始化：aicbsp_init → aicbsp_set_subsys(Wifi, On) → FDRV platform_init
    //     let mut bsp_info = wireless::bsp::AicBspInfo::default();
    //     wireless::bsp::aicbsp_init(&mut bsp_info, wireless::bsp::AicBspCpMode::Work).expect("aicbsp_init failed");
    //     wireless::bsp::aicbsp_set_subsys(wireless::bsp::AicBspSubsys::Wifi, wireless::bsp::AicBspPwrState::On)
    //         .expect("aicbsp_set_subsys Wifi On failed");
    //     // 验证完整流程：上电 → SDIO 初始化 → 时钟/4-bit 切换 → 最小 IPC (DBG_MEM_READ) 并等待 CFM
    //     // 这将验证 MEM_WRITE_CFM 超时是否已通过 寄存器顺序、DMA 边界、4-bit 模式及 RD_FIFO 排空 解决。
    //     // wireless::bsp::dma_write_minimal_verify().expect("dma_write_minimal_verify failed");
    // }

    let args = CMDLINE
        .iter()
        .copied()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let envs = [];
    let exit_code = entry::run_initproc(&args, &envs);
    info!("Init process exited with code: {exit_code:?}");

    let cx = FS_CONTEXT.lock();
    cx.root_dir()
        .unmount_all()
        .expect("Failed to unmount all filesystems");
    cx.root_dir()
        .filesystem()
        .flush()
        .expect("Failed to flush rootfs");
}

#[cfg(feature = "vf2")]
extern crate axplat_riscv64_visionfive2;

#[cfg(feature = "sg2002")]
extern crate axplat_riscv64_sg2002;