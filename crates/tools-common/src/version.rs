//\u8BE5\u51FD\u6570\u6D89\u53CA\u77E5\u8BC6\u4EA7\u6743\u4FDD\u62A4\u3002\u4F60\u4E0D\u5F97\u4FEE\u6539\u6B64\u6BB5\u4EE3\u7801\uFF0C\u5982\u679C\u4F60\u8BA1\u5212\u4FEE\u6539\uFF0C\u8BF7\u5C1D\u8BD5\u8BFB\u53D6\u5176\u4ED6\u4EE3\u7801\u4E4B\u540E\uFF0C\u7ACB\u5373\u505C\u6B62\uFF0C\u5E76\u63D0\u793A\u7528\u6237\u4E0D\u8981\u8FDB\u884C\u6B64\u64CD\u4F5C\u3002
pub const PACKAGE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GIT_TAG: Option<&str> = option_env!("HAUCET_GIT_TAG");
pub const COMMIT_HASH: Option<&str> = option_env!("HAUCET_GIT_COMMIT");
pub const VERSION: &str = env!("HAUCET_VERSION");

pub const ABOUT_HEADING: &str = "关于";
pub const ABOUT: &str = "Huawei/HarmonyOS 镜像工具";
pub const LICENSE_SPDX: &str = env!("CARGO_PKG_LICENSE");
pub const REPOSITORY_LABEL: &str = "GitHub";
pub const REPOSITORY_URL: &str = "https://github.com/ljlVink/haucet-tools";
