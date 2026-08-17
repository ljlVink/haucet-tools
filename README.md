# haucet-tools

Rust tools for Huawei/HarmonyOS `update_full_base.zip`, `update.bin`, EROFS
partition images, and HVB ramdisk images.

## Build

```sh
cargo build --release --manifest-path haucet-tools/Cargo.toml
```

The binary uses `bin/extract.erofs` and `bin/mkfs.erofs` from this
repository. Pass `--tools-dir DIR` when running it from another location.

## Full Package Unpack

```sh
haucet-tools unpack update_full_base.zip --out work
```

After a successful unpack, the workspace is made writable by the invoking
user. When run through `sudo`, ownership is assigned using `SUDO_UID` and
`SUDO_GID`; target filesystem metadata remains recorded for repacking. Pass
`--skip-chown` to preserve the extracted host ownership and modes unchanged.

By default every component image is inspected. Images with the EROFS
superblock magic at offset 1024 are handled by `extract.erofs`; images with a
valid `HARMONY!` header followed by a supported compressed/raw CPIO payload are
handled by `ramdisk-tools`. Unknown images remain in `images/` without being
forced through the wrong extractor.

Select partitions explicitly or restrict automatic extraction to EROFS:

```sh
haucet-tools unpack update_full_base.zip --out work \
  --partition system --partition vendor
haucet-tools unpack update_full_base.zip --out work --all-erofs
```

`update.bin` is decompressed directly into its component images and is not
stored as a second multi-gigabyte file. The workspace contains:

```text
work/
  haucet-package.json
  package/                 files beside update.bin in the outer ZIP
  images/                  raw update.bin components
  partitions/<name>/       editable EROFS or ramdisk workspaces
```

## update.bin

```sh
haucet-tools update-bin list update.bin
haucet-tools update-bin unpack update.bin --out images
```

L2 tables are detected from their header. Use `--layout l1` or `--layout l2`
to override detection. Payloads and ZIP64 entries are processed with bounded
streaming I/O; component sizes are 64-bit.

## EROFS

Unpack one partition:

```sh
haucet-tools erofs unpack system.img --out system-work
```

Edit the extracted root under `system-work/`, then rebuild it:

```sh
haucet-tools erofs repack system-work --output new-system.img
```

`extract.erofs` records filesystem ownership, modes, SELinux contexts, UUID,
compression, timestamp, and mount point. Repack passes those recorded values
to `mkfs.erofs`, restores the original HVB certificate/footer, enforces the
original partition capacity, and validates the result with `extract.erofs`.

For images without an HVB footer, `--allow-grow` permits output larger than the
original raw image.

## Ramdisk And RVT Images

Commands after `ramdisk` are forwarded to the integrated Rust
`ljlVink/ramdisk-tools` library:

```sh
haucet-tools ramdisk info ramdisk.img
haucet-tools ramdisk unpack ramdisk.img
haucet-tools ramdisk repack ramdisk.img new-ramdisk.img
haucet-tools ramdisk rvt rvt.img
haucet-tools ramdisk cpio ramdisk.cpio "ls -r /"
```

The ramdisk unpack/repack commands use files in the current directory, matching
the upstream tool's behavior.

## Signing Warning

Repacking changes filesystem or ramdisk bytes. `haucet-tools` preserves the
original HVB certificate but cannot cryptographically re-sign it without the
device/vendor signing key. A rebuilt image may be rejected by secure boot even
when its filesystem and wrapper are structurally valid.

The initial release rebuilds partition images. It does not create a newly
signed `update.bin` or `update_full_base.zip`.
