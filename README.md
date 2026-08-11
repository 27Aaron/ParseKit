# ParseKit

多平台社交媒体解析与媒体下载库（Rust）。

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
