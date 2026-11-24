# trash-pro

把**文件或文件夹**移入系统回收站 / 废纸篓（Recycle Bin / Trash / Wastebasket）的跨平台工具库。

- 基于 [`trash`](https://github.com/Byron/trash-rs)（Byron/trash-rs），支持 **Linux、macOS、Windows** × **x64、arm64**
- 支持**批量**操作，逐项返回结果，单项失败不影响其余项
- 没有回收站的系统（无桌面纯服务器 / 容器）自动**回退到系统临时目录**，不会丢数据
- 提供 **Node.js 原生绑定**（node-rs / napi-rs），Node 可直接调用
- 提供独立 **CLI** 命令行工具

## 仓库结构

```
trash-pro/
├── core/          # Rust 核心库（crate: trash-pro）+ CLI
├── node/          # npm 包（trash-pro），napi-rs 绑定
│   └── npm/       # 各平台二进制子包（trash-pro-darwin-arm64 等）
└── .github/       # CI：全平台构建矩阵 + npm 发布
```

## 回退语义

| 情况 | 行为 |
|---|---|
| 系统回收站可用 | 移入回收站（`Destination::Trash` / `method: "trash"`） |
| 回收站不可用（无桌面服务器、容器、回收站目录不可写等） | 移入 `<系统临时目录>/trash-pro/<原文件名>`（`method: "temp"`） |
| 目标不存在 / 是根目录（`/`、`C:\`、`..`）/ 路径非法 | 硬错误，**不回退**，返回错误 |
| 临时目录出现同名文件 | 自动去重：`name`、`name.1`、`name.2`… |
| 临时目录在别的文件系统上 | 自动降级为复制 + 删除（跨设备移动） |

选项：

- `fallback_to_temp`（默认 `true`）——回收站失败时回退临时目录；`false` 则直接报错
- `force_temp`（默认 `false`）——跳过回收站，直接移入临时目录
- `temp_dir`——自定义回退目录（默认 `<系统临时目录>/trash-pro`）

> **macOS 说明**：默认使用 `NSFileManager.trashItemAtURL`（无需额外权限、不阻塞），
> 而非 trash-rs 默认的 Finder/`osascript` 方式（需要自动化权限，无头会话中会阻塞数分钟）。
> 代价是 Finder 的"放回原处（Put Back）"可能不可用。

## Rust 用法

```toml
[dependencies]
trash-pro = { path = "core" }
```

```rust
use trash_pro::{delete, delete_all, Destination, Options};

// 单个文件或文件夹
let dest = delete("some-file-or-dir", &Options::default())?;

// 批量：逐项返回结果
let outcomes = delete_all(["a.txt", "b-dir", "missing.txt"], &Options::default());
for outcome in outcomes {
    match outcome.result {
        Ok(Destination::Trash) => println!("进了回收站: {}", outcome.path.display()),
        Ok(Destination::Temp(p)) => println!("进了临时目录: {} -> {}", outcome.path.display(), p.display()),
        Err(e) => eprintln!("失败: {e}"),
    }
}
```

## Node.js 用法

```js
const { trash, trashSync } = require('trash-pro')

// 异步（worker 线程执行，不阻塞事件循环）；也支持 ESM import
const results = await trash(['a.txt', 'some-dir'])

// 同步
const results2 = trashSync(['a.txt'], {
  fallbackToTemp: true,   // 默认 true
  forceTemp: false,       // 默认 false
  tempDir: undefined,     // 默认 <系统临时目录>/trash-pro
})

for (const r of results) {
  if (r.ok) {
    console.log(r.path, '->', r.method, r.destination ?? '')
  } else {
    console.error(r.path, 'failed:', r.error)
  }
}
```

TypeScript 类型随包附带（`index.d.ts`）：

```ts
interface TrashOptions {
  fallbackToTemp?: boolean
  forceTemp?: boolean
  tempDir?: string
}
interface TrashItemResult {
  path: string
  ok: boolean
  method?: 'trash' | 'temp'
  destination?: string
  error?: string
}
function trashSync(paths: string[], options?: TrashOptions): TrashItemResult[]
function trash(paths: string[], options?: TrashOptions): Promise<TrashItemResult[]>
```

批量接口**从不因单项失败抛异常**，失败信息在 `error` 字段中逐项返回。

## CLI 用法

```bash
cargo build -p trash-pro --release   # 产出 target/release/trash-pro

trash-pro <PATH>...                     # 移入回收站（不可用时回退临时目录）
trash-pro --force-temp <PATH>...        # 直接移入临时目录
trash-pro --no-fallback <PATH>...       # 回收站不可用时直接报错
trash-pro --temp-dir <DIR> <PATH>...    # 指定回退目录
```

输出每行 `trashed|temp|error<TAB>路径[<TAB>-><TAB>目标]`，任一项失败退出码为 1。

## 平台与分发

| 平台 | 目标 triple | 回收站实现 |
|---|---|---|
| macOS | `aarch64-apple-darwin` / `x86_64-apple-darwin` | `NSFileManager` 废纸篓 |
| Windows | `aarch64-pc-windows-msvc` / `x86_64-pc-windows-msvc` | 回收站（SHFileOperation / IFileOperation） |
| Linux (glibc) | `aarch64-unknown-linux-gnu` / `x86_64-unknown-linux-gnu` | FreeDesktop Trash 规范 |
| Linux (musl/Alpine) | `aarch64-unknown-linux-musl` / `x86_64-unknown-linux-musl` | FreeDesktop Trash 规范 |

Node 包采用 node-rs 标准分发：主包 `trash-pro` + 8 个平台二进制子包
（`trash-pro-darwin-arm64`、`trash-pro-linux-x64-gnu` …）作为 `optionalDependencies`，
npm 安装时自动选择当前平台。源码级构建：

```bash
cd node
npm install
npm run build       # napi build --release --platform（产出 trash-pro.<平台>.node + index.js/index.d.ts）
npm test
```

CI（`.github/workflows/ci.yml`）在 Linux/macOS/Windows 上跑测试并为 8 个目标交叉构建；
打 `v*` 标签后自动执行 `napi artifacts` + `napi pre-publish` 并发布全部 npm 包（需 `NPM_TOKEN`）。

## 开发

```bash
cargo test            # 核心库测试（不触碰真实回收站，仅探测性测试除外）
cargo build -p trash-pro --release
cd node && npm run build && npm test
```

MIT License. 基于 [trash-rs](https://github.com/Byron/trash-rs)（MIT）与 [napi-rs](https://github.com/napi-rs/napi-rs)（MIT）。
