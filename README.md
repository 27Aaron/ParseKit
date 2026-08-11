# ParseKit

多平台社交媒体解析与媒体下载库（Rust）。

## 平台

| ID | 说明 | 凭据 |
|----|------|------|
| `wechat_channels` | 微信视频号 | `YUANBAO_COOKIE` |
| `douyin` | 抖音公开分享页 | 无 |

## CLI

```bash
cp .env.example .env.local   # 填 YUANBAO_COOKIE 等
cargo run --bin pk -- platforms
cargo run --bin pk -- resolve "分享文案或链接"
cargo run --bin pk -- download "分享文案或链接" -o ./downloads
# 多画质：默认最高；--source 0 指定下标；--prefer smallest 优先小文件
cargo run --bin pk -- download "…" --source 1
cargo run --bin pk -- download "…" --prefer smallest
```

环境变量见 `.env.example`。无 CLI：`cargo build --no-default-features`。

## 库用法（Bot / 其它程序）

```rust
use parse_kit::ParseKit;

# async fn demo() -> parse_kit::Result<()> {
let kit = ParseKit::builder()
    .wechat(std::env::var("YUANBAO_COOKIE").unwrap())?
    .douyin()?
    .build()?;
let post = kit.resolve_text("分享文案或链接").await?;
let sources: Vec<_> = post.media_sources().collect();
let dir = std::path::Path::new("./downloads");
let downloader = kit.media_downloader_for(&post, dir, 200 * 1024 * 1024)?;
let media = downloader.download_playable(sources).await?;
# let _ = media;
# Ok(())
# }
```

结果以 `ResolvedPost.media: Vec<MediaItem>` 为准；用 `media_sources()` / `primary_video()` 取地址。

## 如何新增平台

1. 在 `src/platforms/<name>/`（或单文件）实现 `PlatformResolver`（`platform_id`、`extract_share_url`、`resolve_*`）。
2. 在 `platforms::Platform` 增加变体，并补全 `match`（`display_name` / `capability_note` 等）。
3. 在 `ParseKitBuilder` 增加注册方法；如需无状态 URL 提取，加入 `STATELESS_EXTRACTORS`。
4. 评审媒体 CDN host，写入平台 `REVIEWED_*_MEDIA_HOSTS`，并在 `MediaDownloader::for_platform` 接线。
5. 单元测试 + 脱敏 fixture（`tests/fixtures/`）；live 测试用 `#[ignore]`。
6. 更新本 README 平台表与 `.env.example`（若需要新凭据）。

门禁：host allowlist、拒绝私网 SSRF、临时文件可清理、日志不泄露 cookie / 签名 URL。

## 开发

```bash
nix develop # 或 direnv allow（见 .envrc）
cargo test
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
```

媒体探测需要 `ffprobe`（`nix develop` 已带）。

## License

[MIT](./LICENSE)
