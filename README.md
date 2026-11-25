# trash-pro

把**文件或文件夹**移入系统回收站 / 废纸篓（Recycle Bin / Trash / Wastebasket）的跨平台工具库。

- 基于 [`trash`](https://github.com/Byron/trash-rs)（Byron/trash-rs），支持 **Linux、macOS、Windows** × **x64、arm64**
- 支持**批量**操作，逐项返回结果，单项失败不影响其余项
- 没有回收站的系统（无桌面纯服务器 / 容器）自动**回退到系统临时目录**，不会丢数据
- 回退移动会**记录删除前的原始路径**，支持**列出**与**恢复**（找回）
- 提供 **Node.js 原生绑定**（node-rs / napi-rs），Node 可直接调用
- 提供独立 **CLI** 命令行工具

## 仓库结构

```
trash-pro/
├── core/          # Rust 核心库（crate: trash-pro）+ CLI
├── node/          # npm 包（@totoodev/trash），napi-rs 绑定
│   └── npm/       # 各平台二进制子包（@totoodev/trash-darwin-arm64 等）
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

## 找回（恢复）

| 去向 | 如何找回 |
|---|---|
| 系统回收站（`trash`） | 用系统自带能力：macOS 废纸篓"放回原处"、Windows 回收站"还原"、Linux 文件管理器还原（freedesktop `.trashinfo` 保存了原始路径） |
| 临时回退目录（`temp`） | trash-pro 自动写入记录文件 `<目标>.trashinfo.json`（原始绝对路径、时间、类型），随时可 `list` + `restore` |

记录在移动**成功前**写入——只要返回成功就一定能找回。恢复时**原子占用**原始位置：目标位置已有同名项时默认报冲突错误（`overwrite` 可强制替换），不会静默覆盖。

记录文件示例（`数据.txt.trashinfo.json`）：

```json
{
  "version": 1,
  "originalPath": "/home/u/项目/数据.txt",
  "originalPathBytes": [47, 104, 111, 109, 101, 47, 117, 47, ...],
  "deletedAt": "2026-11-25T02:14:24Z",
  "kind": "file"
}
```

`originalPathBytes` 精确保存原始路径字节（非 UTF-8 路径也能完整恢复），`originalPath` 供人阅读。

## Rust 用法

```toml
[dependencies]
trash-pro = { path = "core" }
```

```rust
use trash_pro::{delete, delete_all, list_recoverable, restore, restore_all, Destination, Options};

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

// 找回：列出回退目录中的可恢复项（含删除前路径），并恢复
for item in list_recoverable(None)? {
    println!("{} <- {}", item.item.display(), item.original.display());
    restore(&item.item, /* overwrite */ false)?;
}
// 批量恢复：restore_all(["/tmp/trash-pro/a.txt", ...], false)
```

## Node.js 用法

```js
// npm install @totoodev/trash
const { trash, trashSync, list, restore, restoreSync } = require('@totoodev/trash')

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

// 找回：列出回退目录中的可恢复项（含删除前路径）
const items = list()            // list(tempDir?) —— 同步
// [{ item, originalPath, deletedAt, kind }, ...]

// 恢复到删除前的位置（目标被占时逐项报冲突；{ overwrite: true } 强制替换）
const restored = await restore(items.map(i => i.item))
// restoreSync(items, { overwrite }) —— 同步
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

interface RecoverableItem {
  item: string
  originalPath: string
  deletedAt: string
  kind: 'file' | 'directory' | 'symlink'
}
function list(tempDir?: string): RecoverableItem[]

interface RestoreOptions { overwrite?: boolean }
interface RestoreItemResult {
  item: string
  ok: boolean
  restoredTo?: string
  error?: string
}
function restoreSync(items: string[], options?: RestoreOptions): RestoreItemResult[]
function restore(items: string[], options?: RestoreOptions): Promise<RestoreItemResult[]>
```

批量接口**从不因单项失败抛异常**，失败信息在 `error` 字段中逐项返回。

## CLI 用法

```bash
cargo build -p trash-pro --release   # 产出 target/release/trash-pro

trash-pro <PATH>...                     # 移入回收站（不可用时回退临时目录）
trash-pro --force-temp <PATH>...        # 直接移入临时目录
trash-pro --no-fallback <PATH>...       # 回收站不可用时直接报错
trash-pro --temp-dir <DIR> <PATH>...    # 指定回退目录

trash-pro list [--temp-dir <DIR>]       # 列出回退目录中的可恢复项（含删除前路径）
trash-pro restore [--overwrite] <ITEM>...  # 恢复到删除前的位置
```

输出每行 `trashed|temp|error<TAB>...`（默认命令）、`ITEM<TAB>原始路径<TAB>时间<TAB>类型`（list）、
`restored|error<TAB>...`（restore），任一项失败退出码为 1。
（`list` / `restore` 是保留子命令名；要删除恰好叫这两个名字的文件请写 `./list`。）

## 平台与分发

| 平台 | 目标 triple | 回收站实现 |
|---|---|---|
| macOS | `aarch64-apple-darwin` / `x86_64-apple-darwin` | `NSFileManager` 废纸篓 |
| Windows | `aarch64-pc-windows-msvc` / `x86_64-pc-windows-msvc` | 回收站（SHFileOperation / IFileOperation） |
| Linux (glibc) | `aarch64-unknown-linux-gnu` / `x86_64-unknown-linux-gnu` | FreeDesktop Trash 规范 |
| Linux (musl/Alpine) | `aarch64-unknown-linux-musl` / `x86_64-unknown-linux-musl` | FreeDesktop Trash 规范 |

Node 包采用 node-rs 标准分发：主包 **`@totoodev/trash`** + 8 个平台二进制子包
（`@totoodev/trash-darwin-arm64`、`@totoodev/trash-linux-x64-gnu` …）作为 `optionalDependencies`，
npm 安装时自动选择当前平台。源码级构建：

```bash
cd node
npm install
npm run build       # napi build --release --platform（产出 trash.<平台>.node + index.js/index.d.ts）
npm test
```

CI（`.github/workflows/ci.yml`）在 Linux/macOS/Windows 原生架构上跑冒烟测试并构建全部 8 个目标：
win/mac 用 napi 直构，Linux gnu 用 cargo-zigbuild（**glibc 2.17 基线**，兼容 Kylin/UOS 等信创发行版，
带符号版本回归护栏），Linux musl 用 cargo-zigbuild 覆盖 Alpine 容器。
推送 `v*` 标签即发布全部 npm 包：`napi artifacts` + `napi pre-publish` 按标签定版，
认证为**双模**——首发需在仓库 Secrets 配置 `NPM_TOKEN`（新包在 npm 上尚不存在，无法用 Trusted
Publishing）；此后移除该 secret，发布自动走 npm Trusted Publishing（OIDC，无需长期令牌）。

## 开发

```bash
cargo test            # 核心库测试（不触碰真实回收站，仅探测性测试除外）
cargo build -p trash-pro --release
cd node && npm run build && npm test
```

MIT License. 基于 [trash-rs](https://github.com/Byron/trash-rs)（MIT）与 [napi-rs](https://github.com/napi-rs/napi-rs)（MIT）。
