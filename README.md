# ParseKit

Rust 库与 CLI（`pk`），从分享链接解析并下载社交媒体媒体。

| 平台 | 内容 | 说明 |
| --- | --- | --- |
| 微信视频号 (`wechat`) | 视频 | 需要 `YUANBAO_COOKIE` |
| 抖音 (`douyin`) | 视频 / 图集 | 移动端签名接口；上游变更后可能失效 |
| 哔哩哔哩 (`bilibili`) | 视频 | 登录 cookie 可提高清晰度 |

```bash
nix develop
just check          # fmt + clippy + test（对齐 CI）
just resolve '分享链接'
just download '分享链接'
```

扫码登录会把凭据写入 `.env.local`，也可直接设置 `YUANBAO_COOKIE` / `BILIBILI_COOKIE`：

```bash
pk wechat login              # 微信扫码 → YUANBAO_COOKIE
pk bilibili login            # 扫码 → BILIBILI_COOKIE
pk wechat status && pk bilibili status
```

新增平台见 [统一适配器模板](./docs/adding-platform.md)。

## Thanks

- [ParseHub](https://github.com/z-mio/ParseHub)

## License

[MIT](./LICENSE)
