# RoomHash 共享投票

这是一个宿主无关、响应式的纯 Rust WASM 投票应用。所有用户可见的应用界面、
导航、表单草稿、命中检测、滚动、结果条形图和公开票据列表都由 WASM 生成并
绘制；宿主只提供 Portable Surface Canvas、输入、全屏、文本输入、持久化和
不透明 P2P 消息转发能力。

应用 ID 固定为 `org.roomhash.voting`。它采用反向域名命名，和仓库名、Rust
crate 名或发布文件名相互独立，第三方宿主应使用 manifest 中的这个 ID 标识应用。

> 不同节点看到的统计结果可能不同，取决于各自实际收集到的数据。这是每个
> 收集者的本地可见视图，不是全局强一致结果。

## 构建与检查

需要稳定版 Rust、`wasm32-unknown-unknown` target 和 Node.js 20+：

```sh
git clone git@github.com:roomhash/voting.git
cd voting
rustup target add wasm32-unknown-unknown
npm run check
```

`npm run test` 只运行 Rust 单元测试；`npm run build` 生成发布产物；
`npm run check` 会完成格式、静态检查、测试、构建和真实 WASM ABI 验收。
项目不需要 npm 运行时依赖。

`dist/` 只包含独立发布所需文件：

- `voting.wasm`：无 imports 的 Portable Surface ABI v3 应用；
- `roomhash.json`：`abi: portable-surface-v1`；
- `voting.torrent`：单文件 WebTorrent 元数据，内置 GitHub Raw HTTP seed；
- `README.md`、`LICENSE`。

检查覆盖 Rust 格式、Clippy、单元测试、真实 WASM ABI、无 imports、manifest
哈希、320×480/375×812/768×1024/1440×900 响应式场景、WASM 内全屏入口、
两节点乱序事件收敛和快照恢复。

`dist/roomhash.json`、`dist/voting.wasm` 与 `dist/voting.torrent` 是宿主集成入口。
manifest 的 `distribution` 同时提供 info hash、大小和完整 magnet URI。其 `ws`
与 `xs` 分别直接指向本仓库 `main/dist/voting.wasm` 和
`main/dist/voting.torrent` 的 GitHub Raw 地址，不依赖 RoomHash WebUI 或
`roomhash.github.io/appstore`。宿主校验 SHA-256 后再加载 WASM。

## 分布式语义

- 每个节点维护可参与投票列表，参与者可选择要查看和参加的投票；创建者可删除
  自己创建的投票。
- 投票有效期由创建者选择，但不得超过 14 天；到期后投票内容、选项和票据会
  从节点状态中自动清除，不再继续转发。
- 每张 ballot 记录 nick、voterHash、optionId、revision、pollId、expiresAtMs 和
  eventId。
- 每个投票按 `voterHash` 只计一票；最大 `(revision,eventId)` 胜出，因此改票
  和乱序投递仍然收敛。
- WASM 验证所有远程事件和快照，按 event ID 排重，拒绝 echo 和非法字段。
- 所有合法原始 ballot 保留并公开展示，明确标识“当前计票”或“已被改票替代”。
- 宿主 Mesh 只负责多跳转发和 anti-entropy，不理解投票领域数据。

## Portable Surface ABI

模块导出 ABI v3 标准入口，输入仅使用通用 `viewport`、`pointer`、`wheel`、
`key`、`text`、`remote`、`state-request` 和 `snapshot`。输出为 Canvas2D display
list、通用 effects、公开 events、snapshot 和 persist。应用不依赖 RoomHash DOM、
CSS、HTML 表单或专用 JavaScript，也可以运行在其他兼容宿主中。

公开 Hash 和 event ID 是排重与完整性标识，不等价于强身份签名。高风险投票
仍需在宿主身份层引入公钥签名或外部认证。

MIT License
