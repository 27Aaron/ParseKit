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

## License

[MIT](./LICENSE)
