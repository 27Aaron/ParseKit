# ParseKit

多平台社交媒体解析与媒体下载（Rust 库 + CLI `pk`）。

平台：`wechat` · `douyin` · `bilibili`

```bash
nix develop
cargo test --locked --all-targets
cargo run --bin pk -- resolve "分享链接"
cargo run --bin pk -- download "分享链接" -o ./downloads
```

微信视频号需要 `YUANBAO_COOKIE`（见 `.env.example`）。

## License

[MIT](./LICENSE)
