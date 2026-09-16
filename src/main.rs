//! Qoder 状态栏渲染：从 stdin 读 JSON 状态输入，向 stdout 输出两行状态文本。
//! 由 ~/.qoder/statusline-command.sh 移植而来，去掉对 bash / jq / awk 的运行时依赖。

use std::io::{Read, Write};
use std::process::{Command, Stdio};

use serde_json::Value;

const BAR_WIDTH: usize = 10;

/// CREATE_NO_WINDOW：spawn git 时隐藏控制台窗口，避免状态栏刷新时闪黑框
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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

fn git_branch(cwd: &str) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.args([
        "-C",
        cwd,
        "--no-optional-locks",
        "symbolic-ref",
        "--short",
        "HEAD",
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = cmd.output().ok()?;
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() && !branch.is_empty() {
        Some(branch)
    } else {
        None
    }
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
