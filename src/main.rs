//! Qoder 状态栏渲染：从 stdin 读 JSON 状态输入，向 stdout 输出两行状态文本。
//! 由 ~/.qoder/statusline-command.sh 移植而来，去掉对 bash / jq / awk / git 的运行时依赖。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;

const BAR_WIDTH: usize = 10;

fn main() {
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);

    let root: Value = match serde_json::from_str(raw.trim()) {
        Ok(Value::Null) => Value::Null,
        Ok(value) if value.is_object() => value,
        // 非法 JSON 或标量输入（正常不会出现）：沿用旧脚本的兜底输出
        _ => {
            print!("+/- lines");
            let _ = std::io::stdout().flush();
            return;
        }
    };

    let mut top: Vec<String> = Vec::new();
    let mut bottom: Vec<String> = Vec::new();

    if let Some(mode) = get_text(&root, "/vim/mode") {
        top.push(format!("[{mode}]"));
    }
    if let Some(model) = get_text(&root, "/model/display_name") {
        top.push(model);
    }
    if let Some(size) = get_number(&root, "/context_window/context_window_size") {
        let human = human_readable_size(size as u64);
        match get_number(&root, "/context_window/used_percentage") {
            Some(pct) => top.push(format!("{human} ctx {} {pct}%", build_ctx_bar(pct))),
            None => top.push(format!("{human} ctx")),
        }
    }
    if let (Some(total), Some(remaining)) = (
        get_text(&root, "/credits/user_quota/total"),
        get_text(&root, "/credits/total_remaining"),
    ) {
        top.push(format!("额度: {remaining}/{total}"));
    }

    if let Some(cwd) = get_text(&root, "/cwd").or_else(|| get_text(&root, "/workspace/current_dir")) {
        bottom.push(cwd);
    }
    let added = get_u64(&root, "/cost/total_lines_added").unwrap_or(0);
    let removed = get_u64(&root, "/cost/total_lines_removed").unwrap_or(0);
    bottom.push(format!("+{added}/-{removed} lines"));

    match (
        get_text(&root, "/worktree/branch"),
        get_text(&root, "/worktree/path"),
    ) {
        (Some(branch), Some(path)) => {
            bottom.push(format!("worktree: {} @ {branch}", shorten_home(&path)));
        }
        _ => {
            // 候选顺序与上面展示用的 /cwd 优先相反——对齐旧脚本的刻意行为，勿统一
            let git_cwd =
                get_text(&root, "/workspace/current_dir").or_else(|| get_text(&root, "/cwd"));
            if let Some(branch) = git_cwd.as_deref().and_then(git_branch) {
                bottom.push(format!("branch: {branch}"));
            }
        }
    }

    let line1 = top.join(" | ");
    let line2 = bottom.join(" | ");
    match (line1.is_empty(), line2.is_empty()) {
        (false, false) => print!("{line1}\n{line2}"),
        (false, true) => print!("{line1}"),
        (true, false) => print!("{line2}"),
        (true, true) => {}
    }
    let _ = std::io::stdout().flush();
}

/// 取文本字段：空串按缺失处理（对齐旧脚本的 -n 判断），数字兼容转成字符串
fn get_text(root: &Value, pointer: &str) -> Option<String> {
    match root.pointer(pointer)? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn get_number(root: &Value, pointer: &str) -> Option<f64> {
    match root.pointer(pointer)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn get_u64(root: &Value, pointer: &str) -> Option<u64> {
    get_number(root, pointer).map(|n| n as u64)
}

fn human_readable_size(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{}M", n / 1_000_000)
    } else if n >= 1_000 {
        format!("{}k", n / 1_000)
    } else {
        n.to_string()
    }
}

fn build_ctx_bar(pct: f64) -> String {
    let filled = (pct * BAR_WIDTH as f64 / 100.0)
        .round()
        .clamp(0.0, BAR_WIDTH as f64) as usize;
    let mut bar = "▓".repeat(filled);
    bar.push_str(&"░".repeat(BAR_WIDTH - filled));
    bar
}

/// 复刻 `git symbolic-ref -q --short HEAD || git describe --tags --always`：从 cwd 逐级向上
/// 找 `.git`，读 HEAD。不调用 git 命令，因此没有子进程。
///
/// 在分支上时 HEAD 是符号引用，取其分支名；detached HEAD 时 HEAD 存的是裸 SHA，改为找指向
/// 该 commit 的 tag，找不到再退回 7 位短 SHA。worktree 与 submodule 的 `.git` 是文件，内容
/// 形如 `gitdir: <路径>`，需顺着指向再读其 HEAD。
fn git_branch(cwd: &str) -> Option<String> {
    let mut dir = Path::new(cwd);
    loop {
        let dot_git = dir.join(".git");
        let git_dir = if dot_git.is_dir() {
            dot_git
        } else if dot_git.is_file() {
            let content = std::fs::read_to_string(&dot_git).ok()?;
            let target = PathBuf::from(content.trim().strip_prefix("gitdir:")?.trim());
            if target.is_absolute() {
                target
            } else {
                dir.join(target)
            }
        } else {
            dir = dir.parent()?;
            continue;
        };

        let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
        let head = head.trim();
        if let Some(name) = head.strip_prefix("ref: refs/heads/") {
            return Some(name.to_string());
        }
        if !head.is_empty() && head.bytes().all(|b| b.is_ascii_hexdigit()) {
            // detached：有 tag 指向该 commit 就显示 tag 名，没有则退回 7 位短 SHA
            return Some(
                find_tag(&git_dir, head).unwrap_or_else(|| head.chars().take(7).collect()),
            );
        }
        return None;
    }
}

/// 在 tag 引用里找指向 `sha` 的那个，同时覆盖松散引用与 packed-refs。
///
/// 已知局限：松散的附注 tag（refs/tags 下未打包、值是 tag object SHA 的文件）需要解析对象库
/// 才能解引用，这里匹配不到。打包进 packed-refs 的 tag 带 `^` 行给出 peeled commit，可以正常
/// 匹配，而 clone 来的仓库 tag 基本都是这一种。
fn find_tag(git_dir: &Path, sha: &str) -> Option<String> {
    if let Some(name) = find_loose_tag(&git_dir.join("refs/tags"), sha) {
        return Some(name);
    }

    let text = std::fs::read_to_string(git_dir.join("packed-refs")).ok()?;
    // `^` 行是上一行附注 tag 解引用后的 commit，用 pending 记住那个 tag 名
    let mut pending: Option<&str> = None;
    for line in text.lines() {
        if let Some(peeled) = line.strip_prefix('^') {
            if peeled.trim() == sha
                && let Some(name) = pending
            {
                return Some(name.to_string());
            }
            pending = None;
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let Some((value, name)) = line.split_once(' ') else {
            continue;
        };
        pending = name.strip_prefix("refs/tags/");
        if value == sha
            && let Some(name) = pending
        {
            return Some(name.to_string());
        }
    }
    None
}

/// 递归扫描 refs/tags 下的松散引用，返回相对 `refs/tags` 的 tag 名（tag 名可含 `/`，
/// 如 `release/v2.0.3`）
fn find_loose_tag(dir: &Path, sha: &str) -> Option<String> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if let Some(found) = find_loose_tag(&path, sha) {
                // 用 `/` 手工拼接而非 Path::join：git ref 名必须以 `/` 分隔，
                // 而 join 在 Windows 上会给出 `\`
                return Some(format!("{name}/{found}"));
            }
        } else if std::fs::read_to_string(&path).is_ok_and(|c| c.trim() == sha) {
            return Some(name);
        }
    }
    None
}

/// 把 home 前缀折叠为 ~。原 bash 脚本的 sed 替换在 Windows 路径下不生效，这里做了修正。
fn shorten_home(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let Some(home) = std::env::home_dir().map(|h| h.to_string_lossy().replace('\\', "/")) else {
        return normalized;
    };
    if !home.is_empty()
        && normalized
            .to_ascii_lowercase()
            .starts_with(&home.to_ascii_lowercase())
    {
        let rest = &normalized[home.len()..];
        if rest.is_empty() {
            return "~".to_string();
        }
        if rest.starts_with('/') {
            return format!("~{rest}");
        }
    }
    normalized
}
