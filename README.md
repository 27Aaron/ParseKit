# ParseKit

多平台社交媒体解析与媒体下载（Rust 库 + CLI `pk`）。

平台：`wechat` · `douyin` · `bilibili`

```bash
nix develop
just check          # fmt + clippy + test（对齐 CI）
just resolve '分享链接'
just download '分享链接'
```

微信视频号需要 `YUANBAO_COOKIE`（见 `.env.example`）。

B 站可选登录以解锁更高清晰度：在 `.env` / `.env.local` 设置 `BILIBILI_COOKIE`，或运行 `pk bilibili login` 扫码（会写入 `.env.local`）。

新增平台时使用[统一适配器模板](./docs/adding-platform.md)。

## License

[MIT](./LICENSE)
