# Third-Party Notices

This file records the material shipped with, or used as a direct source for,
Haucet Tools. It is not a substitute for the referenced license texts.

## Bundled Runtime Components

| Component | Location | License | Source and notes |
| --- | --- | --- | --- |
| `extract.erofs`, `mkfs.erofs` | `bin/` | GPL-2.0 | Built from [sekaiacg/erofs-tools](https://github.com/sekaiacg/erofs-tools), which includes [erofs-utils](https://github.com/erofs/erofs-utils) and other libraries. See `LICENSES/GPL-2.0.txt`. |
| Cygwin runtime | `bin/cygwin1.dll` | LGPL-3.0-or-later with the Cygwin linking exception | [Cygwin licensing terms](https://cygwin.com/licensing.html) and [source](https://cygwin.com/git.html). See `LICENSES/LGPL-3.0.txt`. Distributors must provide source corresponding to the DLL version they ship. |
| HarmonyOS Sans SC Regular | `assets/HarmonyOS_Sans_SC_Regular.ttf` | HarmonyOS Sans Fonts License Agreement | Copyright 2021 Huawei Device Co., Ltd. The agreement permits bundling unmodified copies with non-font software, requires a prominent usage notice, forbids standalone redistribution, and must accompany copies. See `LICENSES/HarmonyOS-Sans.txt`. |

The EROFS and Cygwin binaries are separate programs invoked at runtime; they
are not relicensed under Haucet Tools' GPL-3.0-only license. A release should
identify the exact source revisions used to build them and publish the matching
source beside the binary artifacts. The versions currently committed do not
contain enough provenance metadata to prove that correspondence, so this is a
release-compliance item rather than a resolved attribution issue.

## Adapted Code References

- `crates/hm-fastboot` is a modified fork of
  [boardswarm/fastboot-rs](https://github.com/boardswarm/fastboot-rs), whose
  upstream code is available under MIT or Apache-2.0. The fork as distributed
  here is GPL-3.0-only; the upstream MIT notice is retained in
  `LICENSES/MIT-fastboot-rs.txt`.
- HarmonyOS update package behavior was adapted from
  [OpenHarmony update_packaging_tools](https://gitcode.com/openharmony/update_packaging_tools),
  and HVB behavior was adapted from
  [OpenHarmony startup_hvb](https://gitcode.com/openharmony/startup_hvb), both
  under Apache-2.0. See `LICENSES/Apache-2.0.txt`.
- Ramdisk behavior is based on
  [ljlVink/ramdisk-tools](https://github.com/ljlVink/ramdisk-tools), with
  patch-layout reference to [Magisk](https://github.com/topjohnwu/Magisk).
  Both are GPL-3.0 projects.
- Image-flashing behavior references
  [Potato.ImageFlasher](https://github.com/kitsuned/Potato.ImageFlasher), and
  NVE behavior references [hisi-nve](https://github.com/R0rt1z2/hisi-nve).
  Both are GPL-3.0 projects.
