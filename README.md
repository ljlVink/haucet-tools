<p align="center">
  <img src="assets/logo.png" width="150" height="150" alt="Haucet Tools" />
</p>


<h1 align="center">Haucet Tools</h1>


Rust tools for Huawei/HarmonyOS `update_full_base.zip`, `update.bin`, EROFS partition images, and HVB ramdisk images.

## Examples

```sh
haucet-tools unpack update.bin --out images
haucet-tools unpack update_full_base.zip --out work
haucet-tools unpack update_full_base.zip --out work --partition system --partition vendor
haucet-tools unpack update_full_base.zip --out work --all-erofs
haucet-tools erofs unpack system.img --out system-work
haucet-tools erofs repack system-work --output new-system.img
haucet-tools partition-info ramdisk.img # also system.img
haucet-tools ramdisk unpack ramdisk.img --out ramdisk-work
haucet-tools ramdisk repack ramdisk-work ramdisk.img --out new-ramdisk.img
haucet-tools partition-info rvt.img
haucet-tools partition-info ptable.img
haucet-tools cpio ramdisk.cpio ls --recursive /
haucet-tools fastboot devices
haucet-tools fastboot flash updater updater_vendor.img
haucet-tools fastboot getvar product
haucet-tools fastboot reboot
haucet-tools fastboot oem device-info
haucet-tools vcom devices
haucet-tools vcom flash COM3 0x80000000 loader.bin
```

## Build

The binary uses `bin/extract.erofs` and `bin/mkfs.erofs` from this
repository. Keep the binary beside this repository so it can locate `bin/`.

```sh
cargo build --release
```

## Signing Warning

Repacking changes filesystem or ramdisk bytes. `haucet-tools` preserves the original HVB certificate but cannot cryptographically re-sign it without the device/vendor signing key. A rebuilt image may be rejected by secure boot even when its filesystem and wrapper are structurally valid.

The initial release rebuilds partition images. It does not create a newly signed `update.bin` or `update_full_base.zip`.
## References

[fastboot rust implemention](https://github.com/boardswarm/fastboot-rs)

## License

GPL 3.0
