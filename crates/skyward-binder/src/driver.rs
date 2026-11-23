use crate::parcel::{Payload, Reader};
use anyhow::{bail, Context, Result};
use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::Path;

/// The driver every program on the phone reaches its neighbours through.
/// A kernel that has the driver's own filesystem has no node of its own at
/// the top of `/dev`: the nodes are inside the filesystem, where the phone's
/// own init mounts them and links them. Both places are looked for.
const DEVICE: [&str; 2] = ["/dev/binder", "/dev/binderfs/binder"];

/// The second driver, which the phone's hardware services talk over. Older
/// phones have the node itself; newer ones have the driver's own filesystem
/// and the node inside it.
const HARDWARE: [&str; 2] = ["/dev/hwbinder", "/dev/binderfs/hwbinder"];

/// The layout this crate speaks. The driver refuses a program that knows a
/// different one.
const PROTOCOL: i32 = 8;

/// Room the driver fills with what arrives for us, the size libbinder asks
/// for: a megabyte less the two pages the driver keeps for itself.
const MAP_BYTES: usize = 1024 * 1024 - 8192;

/// Build a driver request number the way the kernel's headers do.
const fn ioc(dir: u32, kind: u8, nr: u8, size: usize) -> u32 {
    (dir << 30) | ((size as u32) << 16) | ((kind as u32) << 8) | nr as u32
}

const WRITE: u32 = 1;
const READ: u32 = 2;

const BINDER_WRITE_READ: u32 = ioc(WRITE | READ, b'b', 1, size_of::<WriteRead>());
const BINDER_SET_MAX_THREADS: u32 = ioc(WRITE, b'b', 5, 4);
const BINDER_VERSION: u32 = ioc(WRITE | READ, b'b', 9, 4);

const BC_TRANSACTION: u32 = ioc(WRITE, b'c', 0, size_of::<TransactionData>());
const BC_TRANSACTION_SG: u32 = ioc(WRITE, b'c', 17, size_of::<TransactionData>() + 8);
const BC_FREE_BUFFER: u32 = ioc(WRITE, b'c', 3, 8);
const BC_ACQUIRE: u32 = ioc(WRITE, b'c', 5, 4);
const BC_RELEASE: u32 = ioc(WRITE, b'c', 6, 4);

const BR_ERROR: u32 = ioc(READ, b'r', 0, 4);
const BR_REPLY: u32 = ioc(READ, b'r', 3, size_of::<TransactionData>());
const BR_DEAD_REPLY: u32 = ioc(0, b'r', 5, 0);
const BR_TRANSACTION_COMPLETE: u32 = ioc(0, b'r', 6, 0);
const BR_INCREFS: u32 = ioc(READ, b'r', 7, 16);
const BR_ACQUIRE: u32 = ioc(READ, b'r', 8, 16);
const BR_RELEASE: u32 = ioc(READ, b'r', 9, 16);
const BR_DECREFS: u32 = ioc(READ, b'r', 10, 16);
const BR_NOOP: u32 = ioc(0, b'r', 12, 0);
const BR_SPAWN_LOOPER: u32 = ioc(0, b'r', 13, 0);
const BR_FAILED_REPLY: u32 = ioc(0, b'r', 17, 0);
const BR_FROZEN_REPLY: u32 = ioc(0, b'r', 18, 0);

/// The call takes no file descriptors but is willing to receive them.
const TF_ACCEPT_FDS: u32 = 0x10;
/// The reply carries a bare error number in place of a payload.
const TF_STATUS_CODE: u32 = 0x08;

#[repr(C)]
#[derive(Default)]
struct WriteRead {
    write_size: u64,
    write_consumed: u64,
    write_buffer: u64,
    read_size: u64,
    read_consumed: u64,
    read_buffer: u64,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct TransactionData {
    target: u64,
    cookie: u64,
    code: u32,
    flags: u32,
    sender_pid: i32,
    sender_euid: u32,
    data_size: u64,
    offsets_size: u64,
    buffer: u64,
    offsets: u64,
}

fn bytes_of<T>(v: &T) -> &[u8] {
    // Every field is a plain integer, so the struct is its own bytes.
    unsafe { std::slice::from_raw_parts((v as *const T).cast::<u8>(), size_of::<T>()) }
}

/// This program's end of the driver.
pub struct Binder {
    file: File,
    map: *mut libc::c_void,
}

impl Binder {
    pub fn open() -> Result<Binder> {
        let node = DEVICE
            .iter()
            .map(Path::new)
            .find(|p| p.exists())
            .with_context(|| format!("none of {} is here, so no service can be reached", DEVICE.join(" or ")))?;
        Binder::on(node)
    }

    /// The driver the phone's hardware services talk over, which is a
    /// second one of the same kind.
    pub fn hardware() -> Result<Binder> {
        let node = HARDWARE
            .iter()
            .map(Path::new)
            .find(|p| p.exists())
            .with_context(|| format!("none of {} is here, so no hardware service can be reached", HARDWARE.join(" or ")))?;
        Binder::on(node)
    }

    fn on(node: &Path) -> Result<Binder> {
        let file = File::options().read(true).write(true).open(node).with_context(|| format!("open {}", node.display()))?;
        let mut version: i32 = 0;
        unsafe { call(&file, BINDER_VERSION, &mut version as *mut i32 as *mut libc::c_void) }.context("ask the driver its protocol")?;
        if version != PROTOCOL {
            bail!("the driver speaks protocol {version}, Skyward speaks {PROTOCOL}");
        }
        // Skyward only ever calls out, so the driver never needs us to take
        // a call and never has to ask for a thread to take it on.
        let mut threads: u32 = 0;
        unsafe { call(&file, BINDER_SET_MAX_THREADS, &mut threads as *mut u32 as *mut libc::c_void) }.context("give up taking calls")?;
        let map = unsafe {