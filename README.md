# haucet-tools

Rust tools for Huawei/HarmonyOS `update_full_base.zip`, `update.bin`, EROFS partition images, and HVB ramdisk images.

## Examples

```sh
haucet-tools update-bin list update.bin
sudo haucet-tools update-bin unpack update.bin --out images
sudo haucet-tools unpack update_full_base.zip --out work
sudo haucet-tools unpack update_full_base.zip --out work --partition system --partition vendor
sudo haucet-tools unpack update_full_base.zip --out work --all-erofs
sudo haucet-tools erofs unpack system.img --out system-work
haucet-tools erofs repack system-work --output new-system.img
haucet-tools ramdisk info ramdisk.img
haucet-tools ramdisk unpack ramdisk.img --out ramdisk-work
haucet-tools ramdisk repack ramdisk-work ramdisk.img --out new-ramdisk.img
haucet-tools rvt rvt.img
haucet-tools cpio ramdisk.cpio ls --recursive /
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

## License

GPL 3.0
