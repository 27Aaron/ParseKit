# ParseKit

多平台社交媒体解析与媒体下载（Rust 库 + CLI `pk`）。

平台：`wechat` · `douyin` · `bilibili`。

```bash
nix develop
just check          # fmt + clippy + test（对齐 CI）
just resolve '分享链接'
just download '分享链接'
```

扫码登录会将凭据写入 `.env.local`；也可直接设置环境变量
`YUANBAO_COOKIE` / `BILIBILI_COOKIE`：

```bash
pk wechat login              # 微信扫码 → YUANBAO_COOKIE
pk bilibili login            # 扫码 → BILIBILI_COOKIE（更高清晰度）
pk wechat status && pk bilibili status
```

新增平台时使用[统一适配器模板](./docs/adding-platform.md)。

## License

[MIT](./LICENSE)
