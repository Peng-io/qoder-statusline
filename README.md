# qoder-statusline

Qoder CLI 状态栏的 Rust 实现：从 stdin 读取状态 JSON，向 stdout 输出两行状态文本。
由 bash + jq 脚本移植而来，运行时不再依赖 bash / jq / awk / git。

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

- **零子进程**：原 bash 版每次刷新要启动十几次 `jq`，外加 `awk`、`git` 子进程；Rust 版一个进程做完，git 信息改为直接读 `.git` 下的文件。其中那次 `git symbolic-ref` 本机实测占约 24ms，去掉后单次渲染从 39ms 降到 15ms（空转基线 16ms），已贴到进程创建的地板。
- **git 读取方式**：从 cwd 逐级向上找 `.git` 并读其 `HEAD`——在分支上取符号引用的分支名；detached HEAD 时找指向该 commit 的 tag（附注 tag 读对象库解引用：松散对象、pack 内完整对象、packed-refs 的 `^` 解引发行都覆盖），找不到退回 7 位短 SHA。同时覆盖松散引用与 packed-refs，以及 worktree / submodule 的 `gitdir:` 指针。
- **行为对齐**：除 detached HEAD 一处外，输出与原脚本逐字节一致（含空输入、缺字段等边界情况）。原脚本在 detached HEAD 时 `symbolic-ref` 失败、整段不显示分支，现在会显示 tag 或短 SHA。另修正了原脚本在 Windows 下 `~` 路径折叠不生效的问题。
- **已知局限**：tag 只做精确匹配，detached 在无 tag 的提交上比 `git describe --tags --always` 少一个「最近 tag + 距离」的回溯；delta 存储的 tag 对象与 idx v1 包不解析，退回 7 位短 SHA（实测多个真实仓库的 tag 对象均以完整对象存储）。
- **体积**：release 配置启用 `strip` / LTO / `codegen-units=1` / `panic="abort"` / `opt-level="s"`，产物约 360KB。
- **跨平台**：代码无平台特定逻辑，纯 std + serde_json。
- **唯一依赖**：[serde_json](https://crates.io/crates/serde_json)。

## 项目结构

```text
src/main.rs     # 状态栏逻辑与 git 元信息读取
src/inflate.rs  # 极简 zlib 解压（读 git 对象用），含单元测试
Cargo.toml      # 含 release 体积优化配置
```
