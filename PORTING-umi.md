# Skyward on the Xiaomi Mi 10 (`umi`)

This is a port of [Skyward](https://github.com/kirklin/skyward), the recovery
written from scratch in Rust, to the Xiaomi Mi 10 (codename `umi`, Qualcomm
SM8250 / Snapdragon 865, kernel 4.19, single-slot). It carries the device
configuration and the code changes the phone's firmware needs, which are the
older boot image header the phone's recovery is packed with and the gzip
ramdisk its kernel reads.

The phone runs a ported HyperOS 4 (`OS4.0.29.0.XEOCNXM`) whose vendor side is
the stock Android 13 vendor of the phone's last official build. The port is
built on the official HyperOS 1.0.3.0 (`OS1.0.3.0.TJBCNXM`) fastboot package,
whose recovery partition it takes as the base and names in `[stock]`.

## What is different from a phone the upstream tree already carries

Everything below was read from the phone itself, not assumed from another
device.

| Fact | Value on `umi` | Consequence |
| --- | --- | --- |
| Boot image header | **version 2** | Upstream `skyward-bootimg` refused v0-v2 outright (`bail!`); a v2 header carries the kernel, the ramdisk, a recovery DTBO and the DTB the bootloader starts from, none of which v3 has. |
| Ramdisk compression | **gzip** | The kernel was built with `CONFIG_RD_GZIP=y` and `# CONFIG_RD_LZ4 is not set`. Upstream packed every ramdisk in LZ4, which this kernel cannot read. |
| Slots | **none** (single boot, recovery, dtbo) | `ab = false`. Dual boot and boot-control work are not offered; the install page hides them on its own, as they key off `boot_a`/`boot_b`. |
| Stock package | **fastboot tarball** | The phone's OTA is an sdat card package with no `payload.bin` and no `recovery.img`; the recovery partition only comes from a fastboot package, a `.tgz` of the partition images. Upstream read only OTA payloads (`.zip`). |
| Touchscreen | **built into the kernel** | No module to load; `[modules] load` stays empty. |
| WLAN | **`qca_cld3_qca6390`** (WCN3988) | One module, no dependencies. Its firmware and MAC address are symlinks in `/vendor/firmware/wlan/qca_cld/qca6390/`, pointing at `/mnt/vendor/persist/wlan/` — a folder the phone's own persist does not keep; the MAC is at the top of `persist` instead. |
| Vibration motor | **AW8697** (`aw8697_haptic`, i2c-2 005a) | Its force-feedback bitmap (`120070000`) carries none of `FF_PERIODIC`/`FF_CUSTOM`, so Skyward's force-feedback click cannot drive it. `[haptics]` is left out; the recovery runs with no vibration. |
| Secure services | **HIDL** `keymaster@4.x`, `gatekeeper@1.0` | Skyward's crypto stack speaks the AIDL `IKeyMintDevice` of Android 13+. This phone's firmware registers no KeyMint AIDL service, so decrypting the data partition from the recovery is **not available** on this port. |
| Display | **1080x2340 @440**, `j2_mp_42_02_0b_dsc_cmd` | The KMS path reads the connector's mode and topology itself; no per-model display code was needed. |
| OS version property | `ro.build.version.incremental` | The phone carries no `ro.mi.os.version.incremental` in the recovery's own property set. |

## The code changes

Narrow, and each confined to what the phone's firmware requires.

1. **`crates/skyward-bootimg/src/image.rs`** — `BootImage` now keeps the header
   it read (`Header`, not `HeaderV3`) and, for versions 0-2, the sections after
   the ramdisk (`second_bootloader`, `recovery_dtbo`, `dtb`) as they were read.
   `parse` reads them; `to_bytes` writes the header back in its own version and
   lays the sections out again the same way, so the bootloader finds its DTB
   where it looks for it. A version 3/4 image is byte-for-byte what it was.

   A version 1 or 2 header also *names* where its recovery DTBO lands, in a
   64-bit field at byte 1636. Because a rebuilt ramdisk is a different size,
   the sections after it move, and that field is rewritten to the offset this
   layout gives them. Without that rewrite the bootloader reads the DTBO from
   where the stock image had it — into the middle of the new ramdisk — and the
   dtbo/dm-dtbo step fails. (Found on the first real artifact: the header said
   73,285,632 while the DTBO had moved to 91,615,232.)
2. **`crates/skyward-bootimg/src/ramdisk.rs`** — `decode` takes gzip as well as
   LZ4 (the `1f 8b` magic in front chooses); `encode_gzip` packs a rebuilt
   archive the way the phone's kernel reads it. `flate2` is the new dependency.
3. **`crates/skyward-pack/src/main.rs`** — the recovery is compressed to match
   the header version it was read with (gzip for v0-2, LZ4 for v3/4); `stock`
   takes the recovery image out of a fastboot tarball as well as an OTA payload,
   streamed from its server and read only as far as `recovery.img`.
4. **`crates/skyward-config/src/lib.rs`** — `[stock] package` may be a `.tgz`
   as well as a `.zip`.
5. **`devices/umi.toml`** — the device configuration, every field taken from the
   phone.
6. **`crates/skyward/src/main.rs`** — a phone with no slots no longer stops the
   UI from starting: `booted_slot()`, which turned `Ok(None)` into an error
   ("not an A/B device"), is replaced by a match on
   `skyward_vab::current_slot()` — `Ok(Some(slot))` prepares as before,
   anything else logs the fact to kmsg and carries on. The whole-partition
   mount path below then does the work the slot path would have.
7. **`crates/skyward-vab/src/partitions.rs`** — the suffix the logical
   partitions actually carry in `super` is derived from the partition names the
   metadata lists (`suffix_of`: `system_a` → `_a`, `system` → empty) instead of
   assuming the slot's own `_a`/`_b`; every place that spelled a partition
   name, matched one, or stripped a suffix (`slot_partitions`, `base`,
   `map_and_mount`, `mount_snapshot`, `mount_mapped`, `slot_bases`,
   `slot_sizes`, `map_whole`, `mount_one`, `mounted_partitions`,
   `unmount_partitions`) follows the actual suffix, so whole-partition
   (`no-slot`) phones mount. `skyward-info` and the mount list of the UI follow.
8. **`crates/skyward/src/ui.rs`** — a panel that carries no normal-maximum
   brightness property in its device-tree node, as an early-2024 DTB of a
   2020 phone does, no longer fails the UI: the whole backlight range the
   panel offers is then its normal maximum. (`qcom,mdss-dsi-bl-max-level` is
   added as a pairing fallback: it pairs with itself, which scales nothing,
   but makes the level the factory property of the Qualcomm pairing is
   measured against readable when no `mi,*` pair exists.)

## Building

The GitHub Actions workflow builds for `umi` alone on a plain push, as the
phone this fork ports to; a tag, or a run by hand, still builds the devices it
names, or every `devices/*.toml` when it names none. It takes the recovery
image out of the stock fastboot package, verifies its SHA-256 against the
configuration, and packs the recovery on top of it.

The maker's own copy of that package (`cdnorg.d.miui.com`,
`bigota.d.miui.com`) answers some requests and refuses others — it served a
`HEAD` here and returned `403` to a browser, and a CI job streaming 5.6 GiB from
it hung for over thirty minutes before being cancelled. The base is therefore a
tarball of just the recovery image, uploaded as a release asset of this fork at
the tag `umi-stock`, and `[stock] package` points at that. The SHA-256 in the
configuration is of the recovery image as the maker's package holds it, so the
check still proves the base is the maker's own bytes.

The artifact is `skyward-<ref>-umi-OS1.0.3.0.TJBCNXM.zip`, holding one file,
`skyward-<ref>-umi-OS1.0.3.0.TJBCNXM.img`, exactly 128 MiB.

## Installing and recovering

The image fills the `recovery` partition. **On this phone `recovery` is not a
slot** — flashing the image writes the recovery; the running system is
untouched, for the system lives in `boot` and in the `super` dynamic
partitions. To go back, flash the stock recovery from the same fastboot
package:

```
fastboot flash recovery recovery.img
```

`avbctl` inside Skyward and the recovery's own settings can turn verified
boot's checks off when a package the system rejects has to be installed, as on
any phone the upstream tree carries. There is no slot to fall back to; keep
the stock `recovery.img` from the fastboot package to hand.

## Known limitations on this port

- **No data decryption.** The phone's key service is HIDL `keymaster@4.x`;
  Skyward asks for the AIDL KeyMint device and finds none. The data partition
  cannot be opened with the screen lock from the recovery.
- **No vibration.** The AW8697 driver takes no periodic force-feedback effect,
  which is the only path Skyward's click uses.
- **Dual boot is not offered.** The phone has no slots.
- **WLAN is untested on hardware.** The module, its firmware files and the
  supplicant are all present and configured from the phone; the bring-up path
  (cnss2, `fs_ready`, the control socket) has not yet been run on the device.

## Verification status

Read this honestly; each line is a claim with its evidence.

- **Implemented** — the changes above exist in the tree.
- **Parsed offline** — the stock recovery of the maker's fastboot package
  (header v2, gzip ramdisk, a 14,055,186-byte recovery DTBO and a
  1,613,904-byte DTB after it) was read by an independent Python parser that
  follows the boot image v2 layout rules, and the built image was checked
  against it, entry by entry:
  - the partition is the full 134,217,728 bytes;
  - the header keeps version 2, its page size, address and kernel size;
  - the ramdisk is gzip, holds all 869 stock entries (867 of them byte for
    byte identical) plus the 13 the port adds;
  - the kernel is copied byte for byte (SHA-256 `eb7547b964eae158…`);
  - `recovery_dtbo_offset` matches the rebuilt layout, and the DTBO
    (14,055,186 bytes) and the DTB (1,613,904 bytes) are the stock bytes;
  - the AVB footer names the image size and points at the vbmeta image the
    packer wrote (`AVB0`, unsigned, as an unlocked bootloader wants).
- **Compiled** — the CI runs on commits `85934802`, `94065d1a`/`a935fe91` and
  `07942dde` built the tools and this device's recovery; `Lint and test` and
  `Build the recovery service and the packer` passed, and `Recovery for umi`
  produced the image above. NOT RUN locally: no Rust toolchain on the phone
  (`rustup-init` cannot spawn threads under this proot).
- **Flashed** — three builds have been written to the `recovery` partition
  with `dd`, each read back and checked byte for byte (write 134,217,728
  bytes; read-back SHA-256 equal). The stock TWRP the phone carried was
  backed up first, to
  `backup/twrp_recovery_umi_20261114_111830.img` (134,217,728 bytes,
  SHA-256 `f4d4d5a705246fea2faceab88bd97897ab38383c6b6c1b127bd749dc2449e673`,
  verified: header v2, gzip ramdisk, 3674 entries, `twres`/`sbin` intact),
  and the maker's own `recovery.img` from the fastboot package stays on the
  computer as the rollback of record.
- **Booted** — parts verified, whole not yet. The first build (`8593480`)
  came up as far as `skyward: umi 0.2.0 recovery up … asking for Menu` and
  then failed `not an A/B device` (found in `/sys/fs/pstore/console-ramoops-0`
  after the boot); the panel and the backlight came up (`set backlight from
  0 to 200`), the screen stayed dark. The second build (`a935fe9`) got
  past that (the log shows `no slot: this phone keeps its system in whole
  partitions`), only to fail at the panel's normal-maximum backlight
  property: the DTB of `OS1.0.3.0` (early 2024) carries none of the `mi,*`
  pairs the UI looks for, and the recovery looped failing every five seconds.
  The third build (`07942dd`) carries the fix for that; **what it shows on
  the screen has not been reported yet**.
- **Runtime / Hardware verified** — NOT RUN. Mounting, wiping, installing,
  MTP and the WLAN bring-up have not been tried from the recovery yet.