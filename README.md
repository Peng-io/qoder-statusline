# qoder-statusline

Qoder CLI 状态栏的 Rust 实现：从 stdin 读取状态 JSON，向 stdout 输出两行状态文本。
由 bash + jq 脚本移植而来，运行时不再依赖 bash / jq / awk。

## 显示效果

- 第一行：`[vim 模式] | 模型 | 上下文窗口（进度条 + 已用百分比）| 额度`
- 第二行：`当前目录 | 代码变更统计 | git 分支（或 worktree）`

```text
[INSERT] | dfmodel | 1M ctx ▓▓▓▓░░░░░░ 37% | 额度: 820/1000
D:\code\demo | +12/-3 lines | branch: main
```

## 输入字段

Qoder CLI 在每次渲染时通过 stdin 传入一个 JSON 对象，本程序消费以下字段：

| 字段 | 用途 |
| --- | --- |
| `vim.mode` | 第一行 `[INSERT]` / `[NORMAL]` |
| `model.display_name` | 模型名 |
| `context_window.context_window_size` | 窗口大小（`1000000` → `1M`） |
| `context_window.used_percentage` | 进度条 + 百分比 |
| `credits.total_remaining` / `credits.user_quota.total` | `额度: 剩余/总量`（`credits` 缺失时自动隐藏该段） |
| `cwd`（回退 `workspace.current_dir`） | 第二行当前目录 |
| `cost.total_lines_added` / `cost.total_lines_removed` | `+N/-M lines` |
| `worktree.branch` / `worktree.path` | `worktree: 路径 @ 分支`（仅 git worktree 会话出现，路径折叠 `~`） |
| `workspace.current_dir`（回退 `cwd`） | 非 worktree 时探测 git 分支 |

字段缺失、空串、非法输入均有兜底：对应段落隐藏或输出占位文本，不会报错。

## 安装

需要 Rust stable 工具链：

```bash
cargo build --release
cp target/release/statusline.exe ~/.qoder/statusline.exe
```

在 `~/.qoder/settings.json` 中把状态栏指向它：

```json
"statusLine": {
  "type": "command",
  "command": "~/.qoder/statusline.exe",
  "padding": 0
}
```

重启 Qoder 会话后生效。

## 设计说明

- **单进程**：原 bash 版每次刷新要启动十几次 `jq`，外加 `awk`、`git` 子进程；Rust 版一次进程调用完成，本机实测渲染快约 17 倍。
- **行为对齐**：输出与原脚本逐字节一致（含空输入、缺字段等边界情况），并修正了原脚本在 Windows 下 `~` 路径折叠不生效的问题。
- **体积优化**：release 配置启用 `strip` / LTO / `codegen-units=1` / `panic="abort"` / `opt-level="s"`，产物约 400KB。
- **跨平台**：代码无平台特定逻辑，仅 Windows 下会额外隐藏 git 子进程的控制台窗口。
- **唯一依赖**：[serde_json](https://crates.io/crates/serde_json)。

## 项目结构

```text
src/main.rs   # 全部逻辑
Cargo.toml    # 含 release 体积优化配置
```
