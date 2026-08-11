# Parse

轻量、异步的社交媒体解析与媒体下载库（Rust）。

面向「解析内核」：识别分享链接 → 解析元数据/媒体地址 → 安全下载。  
**不包含** Telegram / 飞书等 IM 机器人；那些作为独立应用依赖本库。

当前已接入：

| 平台 | 状态 |
|------|------|
| 微信视频号 | ✅ |

后续计划：抖音、快手、B 站等（在 `ParseHub` 注册新平台即可）。

## 作为依赖使用

```toml
# Cargo.toml
parse-core = { git = "https://github.com/27Aaron/Parse.git", branch = "main" }
```

```rust
use parse_core::{ParseHub, media::MediaDownloader};

#[tokio::main]
async fn main() -> parse_core::Result<()> {
    let hub = ParseHub::new(std::env::var("WECHAT_YUANBAO_COOKIE").unwrap())?;
    let post = hub.resolve_text("分享文案或链接…").await?;
    println!("{:#?}", post);
    Ok(())
}
```

相关产品：

- [parse_bot](https://github.com/27Aaron/parse_bot) — Telegram 交付壳

## 开发

```bash
cargo test
# 需要真实 Cookie 的线上用例：
# WECHAT_YUANBAO_COOKIE='…' cargo test --test wechat_live -- --ignored --nocapture
```

需要系统上有 `ffprobe`（`ffmpeg`）以便探测媒体。

## License

MIT
