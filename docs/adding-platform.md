# 添加平台

三个现有平台都遵循同一条处理链：

```text
分享文本 -> share.rs -> 规范 URL -> resolver.rs -> 上游响应
                                               -> parse.rs -> ResolvedPost

mod.rs -> PlatformSpec -> 路由匹配 + 能力说明 + 下载主机白名单/请求身份
```

## 固定目录结构

```text
src/platforms/<name>/
├── mod.rs       # 对外门面；声明唯一的 SPEC
├── resolver.rs  # HTTP 请求、重定向、总超时、PlatformResolver 实现
├── share.rs     # 从文本提取链接、校验主机、生成规范 URL
├── parse.rs     # 把页面/API 数据映射为 ResolvedPost
├── hosts.rs     # 媒体主机白名单和下载请求身份
└── tests.rs     # URL 与 fixture 单元测试
```

平台独有逻辑可以增加文件，但不要改变以上六个文件的职责。例如微信额外使用 `api.rs` 和 `decrypt.rs`。

## `mod.rs` 模板

```rust
//! Example platform adapter.

mod hosts;
mod parse;
mod resolver;
mod share;

#[cfg(test)]
mod tests;

use super::PlatformSpec;
use crate::PlatformId;

pub use hosts::{REVIEWED_MEDIA_HOSTS, download_identity};
pub use resolver::ExampleResolver;
pub use share::extract_share_url;

pub const SPEC: PlatformSpec = PlatformSpec::new(
    PlatformId::Example,
    "public share page",
    extract_share_url,
    REVIEWED_MEDIA_HOSTS,
    download_identity,
);
```

`SPEC` 是平台的唯一静态登记信息。全局链接匹配、CLI 能力说明和 `MediaDownloader::for_platform` 都从它读取配置，不再各自维护主机白名单或下载请求头。

## `resolver.rs` 最小骨架

```rust
use std::time::Duration;

use reqwest::Client;
use url::Url;

use crate::{
    ResolvedPost, Result,
    platforms::{
        PlatformResolver, PlatformSpec,
        util::{DEFAULT_RESOLVE_TIMEOUT, resolve_with_timeout, resolver_http_client},
    },
};

use super::{SPEC, extract_share_url};

const RESOLVE_TIMEOUT: Duration = DEFAULT_RESOLVE_TIMEOUT;

#[derive(Clone)]
pub struct ExampleResolver {
    client: Client,
    timeout: Duration,
}

impl ExampleResolver {
    pub fn new() -> Result<Self> {
        let timeout = RESOLVE_TIMEOUT;
        let client = resolver_http_client(timeout, "无法初始化 Example HTTP 客户端")?;
        Ok(Self { client, timeout })
    }

    pub async fn resolve_text(&self, input: &str) -> Result<ResolvedPost> {
        let url = extract_share_url(input)?;
        self.resolve_url(&url).await
    }

    pub async fn resolve_url(&self, url: &Url) -> Result<ResolvedPost> {
        let url = extract_share_url(url.as_str())?;
        resolve_with_timeout(
            self.timeout,
            self.resolve_normalized(&url),
            "Example 解析总超时",
        )
        .await
    }

    async fn resolve_normalized(&self, url: &Url) -> Result<ResolvedPost> {
        // 1. 使用 self.client 请求上游。
        // 2. 将响应交给 parse.rs。
        todo!()
    }
}

impl PlatformResolver for ExampleResolver {
    fn spec(&self) -> &'static PlatformSpec {
        &SPEC
    }

    async fn resolve_url(&self, url: &Url) -> Result<ResolvedPost> {
        ExampleResolver::resolve_url(self, url).await
    }
}
```

`PlatformResolver` 已统一提供 `platform_id`、`extract_share_url` 和 `resolve_text`。具体实现只登记 `SPEC` 并实现 `resolve_url`；同名的公开固有方法用于保持单平台调用方便。

## 注册清单

1. 在 `PlatformId` 增加枚举值，并同步 `ALL`、`as_str`、`display_name`、`default_title` 和 `parse`。
2. 在 `platforms/mod.rs` 导出模块与 Resolver，把 `SPEC` 加入 `PLATFORM_SPECS`，再补齐 `Platform` 的枚举分派。
3. 在 `ParseKitBuilder` 增加构造方法；决定是否加入 `ParseKit::new` 和 CLI 默认配置。
4. 在 `lib.rs` 导出需要公开的 Resolver 和兼容常量。
5. 为 `share.rs` 写接受/拒绝用例，为 `parse.rs` 提交脱敏 fixture；真实网络测试保持 `#[ignore]`。
6. 更新 README 平台列表并运行 `just check`。

下载器无需再增加平台 `match`：只要 `SPEC` 中的主机白名单和请求身份正确，`MediaDownloader::for_platform` 会自动使用它们。
