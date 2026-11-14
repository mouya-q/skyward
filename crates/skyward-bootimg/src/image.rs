use abootimg_oxide::{binrw::io::Cursor, Header, HeaderV0Versioned, HeaderV3};
use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::Path;
const PAGE: usize = 4096;

/// A boot image split into its header, kernel and ramdisk, and the sections
/// after the ramdisk that only the older header versions (0, 1 and 2) carry:
/// the second stage bootloader, the recovery DTBO and the DTB.
///
/// Header versions 3 and 4 (`boot`, `init_boot`, and the `recovery` of
/// Android 11 and newer) end at the ramdisk: the DTB follows the kernel out
/// of band, and there is no second stage bootloader. Header versions 0 to 2,
/// which the phones with a recovery partition of their own were last built
/// with, carry all of it in the one image, ending in the DTB the bootloader
/// needs to start the kernel.
pub struct BootImage {
    /// The header the image was read with: a version 3 or 4 header as
    /// itself, or the version 0, 1 or 2 header kept as it was read, for the
    /// bootloader that expects its layout and its sections where they are.
    pub header: Header,
    pub kernel: Vec<u8>,
    pub ramdisk: Vec<u8>,
    /// The sections after the ramdisk of a version 0, 1 or 2 image, as they
    /// were read, so a rebuilt image keeps the DTB the bootloader needs.
    trailer: Option<Trailer>,
}

/// The sections of a version 0, 1 or 2 image after its ramdisk, kept as
/// they were read.
struct Trailer {
    second_bootloader: Vec<u8>,
    recovery_dtbo: Vec<u8>,
    dtb: Vec<u8>,
}

fn section(bytes: &[u8], at: usize, len: u32, what: &str) -> Result<Vec<u8>> {
    let end = at + len as usize;
    bytes.get(at..end).map(<[u8]>::to_vec).ok_or_else(|| anyhow!("{what} runs past the end of the image"))
}

impl BootImage {
    /// A version 4 image with the header `header` and the ramdisk
    /// `ramdisk`, as the tests make images with no sections after the
    /// ramdisk.
    pub fn v4(header: HeaderV3, ramdisk: Vec<u8>) -> Self {
        BootImage { header: Header::V3(header), kernel: Vec::new(), ramdisk, trailer: None }
    }

    pub fn read(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
        BootImage::parse(&bytes).with_context(|| format!("read {}", path.display()))
    }

    /// The image `bytes` hold, as a partition holds it: the image, then
    /// whatever fills the partition after it.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let header = Header::parse(&mut Cursor::new(bytes)).context("header")?;
        // The sections are read while the header is only borrowed: the header
        // is moved into the image once every section has taken its bytes.
        let (kernel, ramdisk, trailer) = match &header {
            Header::V3(v3) => {
                let kernel = section(bytes, HeaderV3::kernel_position(), v3.kernel_size, "kernel")?;
                let ramdisk = section(bytes, v3.ramdisk_position(), v3.ramdisk_size, "ramdisk")?;
                (kernel, ramdisk, None)
            }
            // The phones with a recovery partition of their own were last
            // built with header version 2, which ends in the DTB the
            // bootloader needs to start the kernel. Every section is read as
            // it is laid out and written back the same way, only the kernel
            // and the ramdisk changing.
            Header::V0(v0) => {
                let kernel = section(bytes, v0.kernel_position(), v0.kernel_size, "kernel")?;
                let ramdisk = section(bytes, v0.ramdisk_position(), v0.ramdisk_size, "ramdisk")?;
                let second_bootloader = section(bytes, v0.second_bootloader_position(), v0.second_bootloader_size, "second stage bootloader")?;
                let (recovery_dtbo, dtb) = match v0.versioned {
                    HeaderV0Versioned::V0 => (Vec::new(), Vec::new()),
                    HeaderV0Versioned::V1 { recovery_dtbo_size, .. } => {
                        (section(bytes, v0.recovery_dtbo_position(), recovery_dtbo_size, "recovery DTBO")?, Vec::new())
                    }
                    HeaderV0Versioned::V2 { recovery_dtbo_size, dtb_size, .. } => {
                        let Some(dtb_at) = v0.dtb_position() else {
                            return Err(anyhow!("a version 2 header names a DTB and does not say where it is"));
                        };
                        (
                            section(bytes, v0.recovery_dtbo_position(), recovery_dtbo_size, "recovery DTBO")?,
                            section(bytes, dtb_at, dtb_size, "DTB")?,
                        )
                    }
                };
                (kernel, ramdisk, Some(Trailer { second_bootloader, recovery_dtbo, dtb }))
            }
        };
        Ok(BootImage { header, kernel, ramdisk, trailer })
    }

    /// The image with sizes taken from its parts. A version 4 header keeps its
    /// version but carries no boot signature; unlocked bootloaders do not ask
    /// for one. A version 0, 1 or 2 header keeps the layout it was read
    /// with: the sections after the ramdisk are written back as they were
    /// read, so the bootloader finds its DTB where it looks for it.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        match &self.header {
            Header::V3(v3) => {
                let mut v3 = v3.clone();
                v3.kernel_size = u32::try_from(self.kernel.len()).context("kernel size")?;
                v3.ramdisk_size = u32::try_from(self.ramdisk.len()).context("ramdisk size")?;
                v3.v4_signature_size = v3.v4_signature_size.map(|_| 0);
                Header::V3(v3).write(&mut out).context("write header")?;
                pad(&mut out, PAGE);
                out.extend_from_slice(&self.kernel);
                pad(&mut out, PAGE);
                out.extend_from_slice(&self.ramdisk);
                pad(&mut out, PAGE);
            }
            Header::V0(v0) => {
                let Some(Trailer { second_bootloader, recovery_dtbo, dtb }) = &self.trailer else {
                    return Err(anyhow!("the image holds a version 0, 1 or 2 header without its sections after the ramdisk"));
                };
                let mut v0 = v0.clone();
                v0.kernel_size = u32::try_from(self.kernel.len()).context("kernel size")?;
                v0.ramdisk_size = u32::try_from(self.ramdisk.len()).context("ramdisk size")?;
                Header::V0(v0.clone()).write(&mut out).context("write header")?;
                let page = v0.page_size as usize;
                // The sections after the ramdisk move when the ramdisk changes
                // size, and a version 1 or 2 header names where its recovery
                // DTBO lands in the image, so that field is rewritten to the
                // offset this layout gives it. A version 0 header has no such
                // field and nothing to rewrite.
                if matches!(v0.versioned, HeaderV0Versioned::V1 { .. } | HeaderV0Versioned::V2 { .. }) {
                    let up = |n: usize| n.div_ceil(page) * page;
                    let kernel_start = up(out.len());
                    let ramdisk_start = up(kernel_start + self.kernel.len());
                    let second_start = up(ramdisk_start + self.ramdisk.len());
                    let dtbo_offset = up(second_start + second_bootloader.len());
                    // boot_img_hdr_v1/v2: recovery_dtbo_size at 1632,
                    // recovery_dtbo_offset at 1636, eight bytes.
                    let at = 1636;
                    let end = at + 8;
                    if out.len() >= end {
                        out[at..end].copy_from_slice(&(dtbo_offset as u64).to_le_bytes());
                    }
                }
                pad(&mut out, page);
                out.extend_from_slice(&self.kernel);
                pad(&mut out, page);
                out.extend_from_slice(&self.ramdisk);
                pad(&mut out, page);
                out.extend_from_slice(second_bootloader);
                pad(&mut out, page);
                out.extend_from_slice(recovery_dtbo);
                pad(&mut out, page);
                out.extend_from_slice(dtb);
                pad(&mut out, page);
            }
        }
        Ok(out)
    }
}

fn pad(out: &mut Vec<u8>, page: usize) {
    if page == 0 {
        return;
    }
    out.resize(out.len().div_ceil(page) * page, 0);
}