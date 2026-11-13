# Skyward on the Xiaomi Mi 10 (`umi`)

This is a port of [Skyward](https://github.com/kirklin/skyward), the recovery
written from scratch in Rust, to the Xiaomi Mi 10 (codename `umi`, Qualcomm
SM8250 / Snapdragon 865, kernel 4.19, single-slot). It carries the device
configuration and the code changes the phone's firmware needs, which are the
older boot image header the phone's recovery is packed with and the gzip
ramdisk its kernel reads.

The phone runs a ported HyperOS 4 (`OS4.0.29.0.XEOCNXM`) whose vendor side is
the stock Android 13 vendor of the phone's last official build. The port is
built on that last official build, HyperOS 1.0.7.0 (`OS1.0.7.0.TJBCNXM`),
whose recovery partition it takes as the base and knows as `[stock]`.

## What is different from a phone the upstream tree already carries

Everything below was read from the phone itself on `OS1.0.7.0.TJBCNXM`, not
assumed from another device.

| Fact | Value on `umi` | Consequence |
| --- | --- | --- |
| Boot image header | **version 2** | Upstream `skyward-bootimg` refused v0-v2 outright (`bail!`); a v2 header carries the kernel, the ramdisk, a recovery DTBO and the DTB the bootloader starts from, none of which v3 has. |
| Ramdisk compression | **gzip** | The kernel was built with `CONFIG_RD_GZIP=y` and `# CONFIG_RD_LZ4 is not set`. Upstream packed every ramdisk in LZ4, which this kernel cannot read. |
| Slots | **none** (single boot, recovery, dtbo) | `ab = false`. Dual boot and boot-control work are not offered; the install page hides them on its own, as they key off `boot_a`/`boot_b`. |
| Stock package | **fastboot tarball** | The phone's OTA is an sdat card package with no `payload.bin` and no `recovery.img`; the recovery partition only comes from the maker's fastboot package, a `.tgz` of the partition images. Upstream read only OTA payloads (`.zip`). |
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

## Building

The GitHub Actions workflow builds every `devices/*.toml` on a push, and `umi`
is one of them. It takes the recovery image out of the maker's fastboot
package at `bigota.d.miui.com` (5.6 GiB, of which only the recovery image is
read), verifies its SHA-256 against the configuration, and packs the recovery
on top of it. The artifact is `skyward-<ref>-umi-OS1.0.7.0.TJBCNXM.zip`.

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
- **The stock recovery SHA-256 in `devices/umi.toml` is a placeholder** until
  the first CI run that takes the image out of the fastboot package prints the
  real one. Until it is filled in, the build stops at the SHA-256 check, which
  is the intended stop: it refuses to pack a base it has not verified.

## Verification status

Read this honestly; each line is a claim with its evidence.

- **Implemented** — the changes above exist in the tree.
- **Parsed offline** — the stock `boot.img` of the phone's package
  (header v2, gzip ramdisk, 32.5 MiB recovery DTBO) was read by a Python
  parser that matches the Rust layout rules, one page at a time.
- **Compiled** — NOT RUN here (no Rust toolchain on the phone); the CI run
  compiles it.
- **Booted / Runtime / Hardware verified** — NOT RUN. Nothing has been flashed
  to the phone from this port.
