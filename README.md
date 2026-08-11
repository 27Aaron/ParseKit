# Parse

轻量、异步的社交媒体解析与媒体下载库（Rust）。

面向「解析内核」：识别分享链接 → 解析元数据/媒体地址 → 安全下载。  
**不包含** Telegram / 飞书等 IM 机器人；那些作为独立应用依赖本库。

当前已接入：

| 平台 | 状态 | 说明 |
|------|------|------|
| 微信视频号 | ✅ | 需元宝 Cookie |
| 抖音 | ✅ 视频 | 分享页解析；图集/笔记暂不支持 |

后续计划：快手、B 站等。新平台实现 `PlatformResolver`，挂到 `platforms::Platform` 并用 `ParseHub::builder` 注册即可（见 `src/platforms/mod.rs`）。

## 作为依赖使用

```toml
# Cargo.toml
parse-core = { git = "https://github.com/27Aaron/Parse.git", branch = "main" }
```

```rust
use parse_core::{ParseHub, media::MediaDownloader};

#[tokio::main]
async fn main() -> parse_core::Result<()> {
    // Full hub (WeChat + Douyin). Cookie only needed when WeChat is registered.
    let hub = ParseHub::new(std::env::var("WECHAT_YUANBAO_COOKIE").unwrap())?;
    // Or Douyin-only: ParseHub::builder().douyin()?.build()?

    let post = hub.resolve_text("分享文案或链接…").await?;

    // CDN allowlist is explicit per platform (owned by platforms/*, not model).
    let _downloader = match post.platform.as_str() {
        "douyin" => MediaDownloader::for_douyin("/tmp/parse-media", 200 * 1024 * 1024)?,
        _ => MediaDownloader::for_wechat_channels("/tmp/parse-media", 200 * 1024 * 1024)?,
    };
    println!("{:#?}", post);
    Ok(())
}
```

相关产品：

- [parse_bot](https://github.com/27Aaron/parse_bot) — Telegram 交付壳

## 模块边界

| 层 | 放什么 | 不放什么 |
|----|--------|----------|
| `model` | 跨平台结果类型 | CDN 名单、cookie、平台文案 |
| `media` | 通用下载 / probe | 平台专属加解密细节（仅 re-export） |
| `platforms/<name>` | 解析、hosts、identity、专属逻辑 | 交付产品（Bot）细节 |
| `hub` | 多平台注册与分发 | 上游协议细节 |

## 开发

```bash
cargo test
# 需要真实 Cookie 的线上用例：
# WECHAT_YUANBAO_COOKIE='…' cargo test --test wechat_live -- --ignored --nocapture
```

需要系统上有 `ffprobe`（`ffmpeg`）以便探测媒体。

## License

MIT
