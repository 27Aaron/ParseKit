# ParseKit

多平台社交媒体解析与媒体下载库（Rust）+ CLI `pk`。

## 平台

| ID | 说明 | 凭据 |
|----|------|------|
| `wechat_channels` | 微信视频号 | `YUANBAO_COOKIE` |
| `douyin` | 抖音公开分享页（视频 / 图集） | 无 |
| `bilibili` | 哔哩哔哩公开稿件（BV/av/b23） | 无 |

## Workspace

```text
parse-kit/          # 本仓库根：库 crate
crates/pk/          # CLI crate（depends on parse-kit）
```

```bash
cargo test -p parse-kit --locked --all-targets
cargo run -p pk -- platforms
```

## CLI

```bash
cp .env.example .env.local   # 填 YUANBAO_COOKIE 等
cargo run -p pk -- platforms
cargo run -p pk -- resolve "分享文案或链接"
cargo run -p pk -- download "分享文案或链接" -o ./downloads
# 多画质：默认最高；--source 0 指定下标；--prefer smallest 优先小文件
cargo run -p pk -- download "…" --source 1
cargo run -p pk -- download "…" --prefer smallest
```

环境变量见 `.env.example`。

## 库用法（Bot / 其它程序）

```rust
use parse_kit::{ParseKit, PlatformId};

# async fn demo() -> parse_kit::Result<()> {
let kit = ParseKit::builder()
    .wechat(std::env::var("YUANBAO_COOKIE").unwrap())?
    .douyin()?
    .bilibili()?
    .build()?;
let post = kit.resolve_text("分享文案或链接").await?;
assert_eq!(post.platform, PlatformId::Douyin); // 或 WechatChannels / Bilibili
let sources: Vec<_> = post.media_sources().collect();
let dir = std::path::Path::new("./downloads");
let downloader = kit.media_downloader_for(&post, dir, 200 * 1024 * 1024)?;
let media = downloader.download_playable(sources).await?;
# let _ = media;
# Ok(())
# }
```

- 结果以 `ResolvedPost.media` 为准；`platform` 为 [`PlatformId`]。
- 图集：`ContentKind::ImageSet`，`media_sources()` 列出每张图。
- 下载重试：无 `decode_key` 时可 `Range` 续传；微信 XOR 前缀仍整段重下。

## 如何新增平台

1. 在 `src/platforms/<name>/` 实现 `PlatformResolver`。
2. 扩展 `PlatformId` + `Platform` 变体与 match。
3. `ParseKitBuilder` 注册；可选加入 `STATELESS_EXTRACTORS`。
4. 评审 CDN host，接线 `MediaDownloader::for_platform`。
5. 单元测试 + `tests/fixtures/`；live 测试 `#[ignore]`。
6. 更新本 README 平台表。

门禁：host allowlist、拒绝私网 SSRF、临时文件可清理、日志不泄露 cookie / 签名 URL。

## 开发

```bash
nix develop # 或 direnv allow（见 .envrc）
cargo test --locked --all-targets
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
```

媒体探测需要 `ffprobe`（`nix develop` 已带）。

## License

[MIT](./LICENSE)
