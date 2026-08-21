use common::formats::update_bin::UpdateLayout;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Operation {
    FullUnpack,
    UpdateList,
    UpdateUnpack,
    ErofsUnpack,
    ErofsRepack,
    RamdiskUnpack,
    RamdiskRepack,
    RamdiskPatch,
}

impl Operation {
    pub(crate) const ALL: [Self; 8] = [
        Self::FullUnpack,
        Self::UpdateList,
        Self::UpdateUnpack,
        Self::ErofsUnpack,
        Self::ErofsRepack,
        Self::RamdiskUnpack,
        Self::RamdiskRepack,
        Self::RamdiskPatch,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::FullUnpack => "Package unpack",
            Self::UpdateList => "update.bin list",
            Self::UpdateUnpack => "update.bin unpack",
            Self::ErofsUnpack => "EROFS unpack",
            Self::ErofsRepack => "EROFS repack",
            Self::RamdiskUnpack => "Ramdisk unpack",
            Self::RamdiskRepack => "Ramdisk repack",
            Self::RamdiskPatch => "Ramdisk patch",
        }
    }

    pub(crate) fn input_label(self) -> &'static str {
        match self {
            Self::ErofsRepack | Self::RamdiskRepack => "Workspace",
            _ => "Input",
        }
    }

    pub(crate) fn secondary_label(self) -> Option<&'static str> {
        match self {
            Self::RamdiskRepack => Some("Original image"),
            Self::RamdiskPatch => Some("Replacement binary"),
            _ => None,
        }
    }

    pub(crate) fn output_label(self) -> Option<&'static str> {
        match self {
            Self::UpdateList => None,
            Self::RamdiskPatch | Self::RamdiskRepack | Self::ErofsRepack => Some("Output image"),
            _ => Some("Output directory"),
        }
    }

    pub(crate) fn needs_erofs_tools(self) -> bool {
        matches!(
            self,
            Self::FullUnpack | Self::ErofsUnpack | Self::ErofsRepack
        )
    }

    pub(crate) fn needs_layout(self) -> bool {
        matches!(
            self,
            Self::FullUnpack | Self::UpdateList | Self::UpdateUnpack
        )
    }

    pub(crate) fn needs_skip_chown(self) -> bool {
        matches!(
            self,
            Self::FullUnpack | Self::UpdateUnpack | Self::ErofsUnpack | Self::RamdiskUnpack
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LayoutChoice {
    Auto,
    L1,
    L2,
}

impl LayoutChoice {
    pub(crate) const ALL: [Self; 3] = [Self::Auto, Self::L1, Self::L2];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::L1 => "L1",
            Self::L2 => "L2",
        }
    }

    pub(crate) fn to_update_layout(self) -> UpdateLayout {
        match self {
            Self::Auto => UpdateLayout::Auto,
            Self::L1 => UpdateLayout::L1,
            Self::L2 => UpdateLayout::L2,
        }
    }
}
