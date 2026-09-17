//! Qoder 状态栏渲染：从 stdin 读 JSON 状态输入，向 stdout 输出两行状态文本。
//! 由 ~/.qoder/statusline-command.sh 移植而来，去掉对 bash / jq / awk / git 的运行时依赖。

mod inflate;

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
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
/// 该 commit 的 tag（附注 tag 需读对象库解引用），找不到再退回 7 位短 SHA。worktree 与
/// submodule 的 `.git` 是文件，内容形如 `gitdir: <路径>`，需顺着指向再读其 HEAD。
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
            // detached：有 tag 指向该 commit 就显示 tag 名，没有则退回 7 位短 SHA。
            // 引用与对象都在共享 gitdir（worktree 场景）里找
            let common = common_git_dir(&git_dir);
            return Some(
                find_tag(&common, head).unwrap_or_else(|| head.chars().take(7).collect()),
            );
        }
        return None;
    }
}

/// 在 tag 引用里找指向 `sha` 的那个，同时覆盖松散引用与 packed-refs。
///
/// 松散附注 tag（值是 tag 对象 SHA）借助对象库解引用；打包进 packed-refs 的附注 tag
/// 直接读 `^` 解引发行。tag 对象 delta 存储等解引用不出的情况，由调用方退回短 SHA。
fn find_tag(git_dir: &Path, sha: &str) -> Option<String> {
    if let Some(name) = find_loose_tag(&git_dir.join("refs/tags"), git_dir, sha) {
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
/// 如 `release/v2.0.3`）：值直接是 commit SHA（轻量 tag）时直接比较；附注 tag 的候选
/// 收齐后统一解引用——松散对象逐个查，pack 侧批量检索，避免每个 tag 都把全部包扫一遍
fn find_loose_tag(dir: &Path, git_dir: &Path, sha: &str) -> Option<String> {
    let mut annotated: Vec<(String, String)> = Vec::new(); // (tag 名, tag 对象 SHA)
    if let Some(name) = collect_loose_tags(dir, "", sha, &mut annotated) {
        return Some(name);
    }

    let mut candidates: Vec<[u8; 20]> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    for (name, object_sha) in &annotated {
        if read_loose_object(git_dir, object_sha)
            .and_then(|object| tag_target(&object))
            .as_deref()
            == Some(sha)
        {
            return Some(name.clone());
        }
        if let Some(sha20) = hex_to_sha(object_sha) {
            candidates.push(sha20);
            names.push(name.clone());
        }
    }

    for (i, object) in read_packed_objects(git_dir, &candidates) {
        if tag_target(&object).as_deref() == Some(sha) {
            return Some(names[i].clone());
        }
    }
    None
}

/// 递归收集松散 tag 引用：值直接命中返回 Some(名)；不能直接判断的记入 pending
fn collect_loose_tags(
    dir: &Path,
    prefix: &str,
    sha: &str,
    pending: &mut Vec<(String, String)>,
) -> Option<String> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        // 用 `/` 手工拼接而非 Path::join：git ref 名必须以 `/` 分隔，
        // 而 join 在 Windows 上会给出 `\`
        let full = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if path.is_dir() {
            if let Some(hit) = collect_loose_tags(&path, &full, sha, pending) {
                return Some(hit);
            }
            continue;
        }
        // git 更新引用时先写 `<名>.lock` 再改名，忽略避免读到半成品
        if name.ends_with(".lock") {
            continue;
        }
        let Some(content) = std::fs::read_to_string(&path).ok() else {
            continue;
        };
        let content = content.trim();
        if content == sha {
            return Some(full);
        }
        if content.len() == 40 && content.bytes().all(|b| b.is_ascii_hexdigit()) {
            pending.push((full, content.to_string()));
        }
    }
    None
}

/// worktree 的 gitdir（.git/worktrees/<名>）只放本工作树自己的文件（HEAD、index 等），
/// refs 与 objects 都共享主仓 .git，commondir 文件指向那里；普通仓库与 submodule 没有它
fn common_git_dir(git_dir: &Path) -> PathBuf {
    match std::fs::read_to_string(git_dir.join("commondir")) {
        Ok(content) => {
            let common = PathBuf::from(content.trim());
            if common.is_absolute() {
                common
            } else {
                git_dir.join(common)
            }
        }
        Err(_) => git_dir.to_path_buf(),
    }
}

/// 从对象正文里取 tag 指向的目标对象：松散对象带 `<type> <size>\0` 前缀，pack 条目
/// 没有，先剥掉再读首行 `object <sha>`；非 tag 对象返回 None
fn tag_target(object: &[u8]) -> Option<String> {
    let content = match object.iter().position(|&b| b == 0) {
        Some(end) if end < 32 => &object[end + 1..],
        _ => &object[..],
    };
    let first = content.split(|b| *b == b'\n').next()?;
    let target = std::str::from_utf8(first).ok()?.strip_prefix("object ")?;
    let target = target.trim();
    // 目标行必须是完整 SHA；commit 对象首行是 `tree <sha>`，自然不匹配
    (target.len() == 40 && target.bytes().all(|b| b.is_ascii_hexdigit())).then(|| target.to_string())
}

/// 读松散对象：zlib 解压出对象正文。上限 256 字节足够覆盖 tag 对象首行，
/// 超出时解压提前收尾，正文开头仍然完整
fn read_loose_object(git_dir: &Path, sha: &str) -> Option<Vec<u8>> {
    let path = git_dir.join("objects").join(&sha[..2]).join(&sha[2..]);
    inflate::zlib_decompress(&std::fs::read(path).ok()?, 256)
}

/// idx v2 的固定区长度：8 字节 magic + 版本，之后是 256 项 fanout
const IDX_HEADER: usize = 8 + 256 * 4;

/// 批量读取 pack 内对象：每个 idx 只打开一次，先读表头 + fanout 做首字节预筛，
/// 有候选落在包内才整读并在 idx 里逐个二分。返回 (候选序号, 对象正文)。
/// 只支持 idx v2 与完整（非 delta）对象——tag 对象极小，实测各仓库均以完整对象存储；
/// delta 条目会被跳过，由调用方退回短 SHA
fn read_packed_objects(git_dir: &Path, shas: &[[u8; 20]]) -> Vec<(usize, Vec<u8>)> {
    let mut hits = Vec::new();
    let Ok(entries) = std::fs::read_dir(git_dir.join("objects").join("pack")) else {
        return hits;
    };
    for entry in entries.flatten() {
        let idx_path = entry.path();
        if idx_path.extension().and_then(|e| e.to_str()) != Some("idx") {
            continue;
        }
        let mut head = [0u8; IDX_HEADER];
        let Ok(mut file) = File::open(&idx_path) else {
            continue;
        };
        if file.read_exact(&mut head).is_err()
            || head[..4] != [0xff, b't', b'O', b'c']
            || u32::from_be_bytes(head[4..8].try_into().expect("固定 4 字节")) != 2
        {
            continue;
        }
        let fanout = |byte: usize| -> usize {
            u32::from_be_bytes(
                head[8 + byte * 4..12 + byte * 4].try_into().expect("fanout 固定 4 字节"),
            ) as usize
        };
        let has_candidate = shas.iter().any(|sha| {
            let first = sha[0] as usize;
            let lo = if first == 0 { 0 } else { fanout(first - 1) };
            lo != fanout(first)
        });
        if !has_candidate {
            continue;
        }

        let Ok(idx) = std::fs::read(&idx_path) else {
            continue;
        };
        let count = fanout(255);
        for (i, sha) in shas.iter().enumerate() {
            if hits.iter().any(|(hit, _)| *hit == i) {
                continue;
            }
            let first = sha[0] as usize;
            let lo = if first == 0 { 0 } else { fanout(first - 1) };
            let hi = fanout(first);
            if let Some(object) = search_idx(&idx_path, &idx, count, lo, hi, sha) {
                hits.push((i, object));
            }
        }
    }
    hits
}

/// 在整读的 idx 缓冲里二分定位 sha；命中则按偏移读对应 pack 条目
fn search_idx(
    idx_path: &Path,
    idx: &[u8],
    count: usize,
    mut lo: usize,
    mut hi: usize,
    sha: &[u8; 20],
) -> Option<Vec<u8>> {
    while lo < hi {
        let mid = (lo + hi) / 2;
        match idx.get(IDX_HEADER + mid * 20..IDX_HEADER + mid * 20 + 20)?.cmp(&sha[..]) {
            std::cmp::Ordering::Less => lo = mid + 1,
            std::cmp::Ordering::Greater => hi = mid,
            std::cmp::Ordering::Equal => {
                let offsets = IDX_HEADER + count * 20 + count * 4;
                let off32 = u32::from_be_bytes(
                    idx.get(offsets + mid * 4..offsets + mid * 4 + 4)?.try_into().ok()?,
                );
                let offset = if off32 & 0x8000_0000 == 0 {
                    off32 as u64
                } else {
                    // 高位为 1 时低 31 位是 64 位偏移表的序号，表紧跟在 32 位偏移表之后
                    let big = offsets + count * 4 + (off32 & 0x7fff_ffff) as usize * 8;
                    u64::from_be_bytes(idx.get(big..big + 8)?.try_into().ok()?)
                };
                return read_pack_entry(&idx_path.with_extension("pack"), offset);
            }
        }
    }
    None
}

/// 读 pack 条目：变长头给出类型与解压后大小，随后是 zlib 流
fn read_pack_entry(pack_path: &Path, offset: u64) -> Option<Vec<u8>> {
    let mut file = File::open(pack_path).ok()?;
    file.seek(SeekFrom::Start(offset)).ok()?;

    let mut byte = [0u8; 1];
    file.read_exact(&mut byte).ok()?;
    let obj_type = (byte[0] >> 4) & 7;
    let mut size = (byte[0] & 0x0f) as u64;
    let mut shift = 4;
    while byte[0] & 0x80 != 0 {
        file.read_exact(&mut byte).ok()?;
        size |= ((byte[0] & 0x7f) as u64) << shift;
        shift += 7;
    }
    // 4 = 完整 tag 对象；6/7 是 delta 存储，放弃（调用方退回短 SHA）
    if obj_type != 4 || size > 64 * 1024 {
        return None;
    }

    // 读取上限：deflate 真实编码不膨胀，2 倍大小足以覆盖并防御损坏头部的谎报
    let mut compressed = Vec::new();
    file.take(size * 2 + 1024).read_to_end(&mut compressed).ok()?;
    // 解压上限取 size + 1：正常流恰好解出 size 字节，能走完 adler32 校验
    let out = inflate::zlib_decompress(&compressed, (size + 1) as usize)?;
    (out.len() as u64 == size).then_some(out)
}

/// 40 位十六进制 SHA-1 转 20 字节；非法输入返回 None
fn hex_to_sha(hex: &str) -> Option<[u8; 20]> {
    if hex.len() != 40 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut sha = [0u8; 20];
    for (i, byte) in sha.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(sha)
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
