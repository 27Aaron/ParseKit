# Parse

多平台社交媒体解析与媒体下载库（Rust）。

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
