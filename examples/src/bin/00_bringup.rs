//! Demo 00: board bring-up.
//!
//! Prints chip revision and MAC, tests and maps PSRAM into the heap, reads
//! the ES8311 codec's chip ID and scans the codec I2C bus (SDA GPIO51,
//! SCL GPIO50).

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

extern crate alloc;

use alloc::vec::Vec;
use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use esp_backtrace as _;
use esp_hal::{
    Async,
    efuse::{self, InterfaceMacAddress},
    i2c::master::{AcknowledgeCheckFailedReason, Error as I2cError, I2c},
    psram::Psram,
};
use esp32_s31_coreboard::board::{self, i2c_addr};
use log::{info, warn};

esp_bootloader_esp_idf::esp_app_desc!();

/// ES8311 chip ID registers and the values they hold.
const ES8311_ID_REGS: [u8; 2] = [0xFD, 0xFE];
const ES8311_ID: [u8; 2] = [0x83, 0x11];
/// ES8311 chip version register.
const ES8311_VERSION_REG: u8 = 0xFF;

/// Upper bound for one I2C transfer, so a stuck bus can't hang the demo.
const I2C_TIMEOUT: Duration = Duration::from_millis(50);

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    esp32_s31_coreboard_examples::start!(p, "ESP32-S31-Function-CoreBoard-1 bring-up");

    let rev = efuse::chip_revision();
    info!("Chip revision: v{}.{}", rev.major, rev.minor);
    info!(
        "Wi-Fi STA MAC: {}",
        efuse::interface_mac_address(InterfaceMacAddress::Station)
    );

    let psram = Psram::new(p.PSRAM, Default::default());
    test_psram(&psram);
    esp_alloc::psram_allocator!(&psram);
    check_heap();

    match board::i2c(p.I2C0, p.GPIO51, p.GPIO50) {
        Ok(mut i2c) => {
            check_codec(&mut i2c).await;
            scan_i2c(&mut i2c).await;
        }
        Err(e) => warn!("I2C init failed: {e:?}"),
    }

    info!("Bring-up done.");
    halt().await
}

/// Reads one ES8311 register, `None` on any error or timeout.
async fn read_reg(i2c: &mut I2c<'_, Async>, reg: u8) -> Option<u8> {
    let mut value = [0u8];
    with_timeout(
        I2C_TIMEOUT,
        i2c.write_read_async(i2c_addr::ES8311, &[reg], &mut value),
    )
    .await
    .ok()?
    .ok()?;
    Some(value[0])
}

/// Checks the ES8311 chip ID and logs it with the chip version.
#[allow(
    clippy::large_stack_frames,
    reason = "the async I2C transfer future is ~1 KiB; awaited once from main"
)]
async fn check_codec(i2c: &mut I2c<'_, Async>) -> bool {
    let mut id = [0u8; 2];
    for (byte, reg) in id.iter_mut().zip(ES8311_ID_REGS) {
        match read_reg(i2c, reg).await {
            Some(v) => *byte = v,
            None => {
                warn!("ES8311 did not answer at 0x{:02X}", i2c_addr::ES8311);
                return false;
            }
        }
    }
    if id != ES8311_ID {
        warn!(
            "Device at 0x{:02X} has chip ID {id:02X?}, expected {ES8311_ID:02X?}",
            i2c_addr::ES8311
        );
        return false;
    }
    let version = read_reg(i2c, ES8311_VERSION_REG).await;
    info!("ES8311 chip ID {id:02X?}, version {version:02X?}");
    true
}

/// Writes an address-derived pattern over the whole PSRAM and reads it back.
fn test_psram(psram: &Psram) {
    let (start, size) = psram.raw_parts();
    info!("PSRAM: {} KiB mapped at {:p}", size / 1024, start);
    if size == 0 {
        warn!("PSRAM not detected");
        return;
    }

    let words = size / 4;
    let base = start.cast::<u32>();
    let pattern = |i: usize| (i as u32).wrapping_mul(0x9E37_79B9) ^ 0xA5A5_5A5A;

    let t0 = Instant::now();
    for i in 0..words {
        // SAFETY: `i` stays inside the mapped PSRAM region, which nothing else uses yet.
        unsafe { base.add(i).write_volatile(pattern(i)) };
    }
    let write_ms = t0.elapsed().as_millis();

    let t0 = Instant::now();
    let errors = (0..words)
        // SAFETY: as above.
        .filter(|&i| unsafe { base.add(i).read_volatile() } != pattern(i))
        .count();
    let read_ms = t0.elapsed().as_millis();

    let mib = size as u64 / (1024 * 1024);
    info!(
        "PSRAM test: {errors} bad words, write {write_ms} ms (~{} MiB/s), read {read_ms} ms (~{} MiB/s)",
        mib * 1000 / write_ms.max(1),
        mib * 1000 / read_ms.max(1),
    );
}

fn check_heap() {
    let buf: Vec<u8> = alloc::vec![0x5A; 1024 * 1024];
    let ok = buf.iter().all(|&b| b == 0x5A);
    info!(
        "Heap: 1 MiB allocation at {:p} {}",
        buf.as_ptr(),
        if ok { "OK" } else { "CORRUPT" }
    );
    drop(buf);
    info!("{}", esp_alloc::HEAP.stats());
}

#[allow(
    clippy::large_stack_frames,
    reason = "the async I2C transfer future is ~1 KiB; awaited once from main"
)]
async fn scan_i2c(i2c: &mut I2c<'_, Async>) {
    info!("Scanning the codec I2C bus...");
    let mut found: u128 = 0;
    for addr in 0x08u8..0x78 {
        let mut byte = [0u8; 1];
        match with_timeout(I2C_TIMEOUT, i2c.read_async(addr, &mut byte)).await {
            Ok(Ok(())) => found |= 1 << addr,
            Ok(Err(I2cError::AcknowledgeCheckFailed(AcknowledgeCheckFailedReason::Address))) => {}
            // The address was acknowledged, only the read was refused.
            Ok(Err(I2cError::AcknowledgeCheckFailed(_))) => found |= 1 << addr,
            Ok(Err(e)) => warn!("  0x{addr:02X}: error {e:?}"),
            Err(_) => warn!("  0x{addr:02X}: timeout"),
        }
    }

    for addr in (0x08u8..0x78).filter(|&a| found & (1 << a) != 0) {
        let name = if addr == i2c_addr::ES8311 {
            "ES8311 audio codec"
        } else {
            "unknown"
        };
        info!("  0x{addr:02X}: {name}");
    }
    info!("I2C scan: {} device(s)", found.count_ones());
}

async fn halt() -> ! {
    loop {
        Timer::after(Duration::from_secs(10)).await;
        info!("alive, uptime {} s", Instant::now().as_secs());
    }
}
