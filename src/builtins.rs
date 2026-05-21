//! 内建命令实现集合 + PATH 可执行文件查找。
//!
//! 所有内建 runner（`run_echo` / `run_pwd` / `run_type` / `run_cd`）共享同一签名风格：
//! 拿 `sink: &mut dyn Write` 写正常输出，拿 `err_sink: &mut dyn Write` 写错误信息——
//! 由上层 REPL 根据 `>` / `1>` / `2>` / 追加等重定向语义在调用前打开对应 sink。
//!
//! `find_in_path` 既被 `run_type` 用于命中判定，也被 `exec::run_external` 用于外部
//! 命令解析，故放在本模块作为单一数据源。

use std::collections::HashMap;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

/// shell 内建命令清单，作为 `type` 命令查询的单一数据源。
/// 后续阶段新增内建（如 pwd/cd）时只需在此处追加。
pub const BUILTINS: &[&str] = &["echo", "exit", "type", "pwd", "cd", "complete", "jobs"];

/// 后台作业状态。
///
/// 本阶段（codecrafters「list a single background job」）仅需要 `Running` 一个变体——
/// 题面 Notes 明示「detecting when jobs exit will come in later stages」。
/// `Done` / `Stopped` 等状态留待后续阶段加入；此 enum 故意保留扩展空间但**不**预先
/// 实现 SIGCHLD reap 机制，避免过早设计。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStatus {
    Running,
}

impl JobStatus {
    /// 状态短串，用于 `jobs` 行内打印（左对齐到 24 字符宽）。
    /// 显式 `&'static str` 映射，避免依赖 `Display` impl，便于后续阶段为不同状态
    /// 加入额外字段时不破坏格式契约。
    pub fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Running => "Running",
        }
    }
}

/// 单个后台作业的元信息。
///
/// 字段与题面「Tracking Background Jobs」四要素一一对应：
/// - `id`：作业编号（`[N]` 中的 N），从 1 起递增；由 `next_job_id` 计数器分配；
/// - `pid`：子进程 PID（`Command::spawn` 返回的 `Child::id()`，`u32`）；
/// - `command`：命令字符串，本阶段用 `parsed.argv.join(" ")`（zsh 风格、无尾 `&`、
///   无重定向片段）。题面明示「trailing `&` 是可选的」，tester 容忍；
/// - `status`：当前状态，本阶段恒为 `Running`。
///
/// 不存 `started_at` / `exit_code` / `Child` 句柄等——本阶段无需 reap，留作未来扩展。
///
/// `pid` 在本阶段不参与 `jobs` 行内输出（题面格式未要求 PID 列），但保留字段以便
/// 后续阶段 `jobs -l` flag 与 SIGCHLD reap 路径直接复用；`#[allow(dead_code)]`
/// 显式抑制本阶段编译警告。
#[derive(Debug, Clone)]
pub struct Job {
    pub id: u32,
    #[allow(dead_code)]
    pub pid: u32,
    pub command: String,
    pub status: JobStatus,
}

/// 按 PATH 顺序查找可执行文件。
/// 命中条件：文件存在、是普通文件、Unix 执行位（owner/group/other 任一）置位。
/// 目录不存在 / 无权限读取 / 非普通文件等场景静默跳过，与 bash 实际行为一致。
pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        let Ok(meta) = candidate.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        if meta.permissions().mode() & 0o111 != 0 {
            return Some(candidate);
        }
    }
    None
}

/// 启动期一次性扫描 PATH，返回所有可执行文件 basename 的有序列表。
///
/// 与 `find_in_path` 共享同一可执行性判定标准（`is_file()` + `0o111` 任一执行位），
/// 作为 TAB 补全候选源使用。返回顺序：按 PATH 顺序、目录内 `read_dir` 顺序——
/// **不去重**，去重责任交由调用方（如 completion 端按 builtin 优先策略合并）。
///
/// 错误处理（与 bash 行为一致）：
/// - `PATH` 环境变量缺失 → 直接返回空 vec
/// - 某个 PATH 目录不存在 / 无权限 / 非目录 → `read_dir` 失败，静默跳过该目录
/// - 单个 entry 的 `metadata()` 读取失败 → 静默跳过该 entry
///
/// 不向 stderr 写任何错误信息：TAB 补全是高频热路径，避免污染交互终端。
pub fn list_path_executables() -> Vec<String> {
    let mut out = Vec::new();
    let Some(path_var) = std::env::var_os("PATH") else {
        return out;
    };
    for dir in std::env::split_paths(&path_var) {
        // read_dir 失败（目录不存在 / 无权限 / 非目录）静默跳过整目录
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            // metadata 读取失败的 entry（如 symlink 悬空）静默跳过
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            if meta.permissions().mode() & 0o111 == 0 {
                continue;
            }
            // file_name 在 Linux 上一般是合法 UTF-8；遇到非 UTF-8 字节走 lossy
            // 转换并入候选——后续作为命令名补全到 line 时仍是合法 String。
            out.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    out
}

/// `echo` 内建：把所有参数用单空格连接后写入 sink。
/// 引号内空格已在 token 内部保留，此处单空格 join 是正确行为。
pub fn run_echo(sink: &mut dyn Write, args: &[String]) -> io::Result<()> {
    writeln!(sink, "{}", args.join(" "))
}

/// `pwd` 内建：打印当前工作目录的绝对路径。
/// `current_dir()` 内部调用 `getcwd(2)`，由 OS 保证返回绝对路径。
/// 目录被删除 / 无权限等异常场景：错误信息写入 err_sink，可被 `2>` 重定向到文件。
pub fn run_pwd(sink: &mut dyn Write, err_sink: &mut dyn Write) -> io::Result<()> {
    match std::env::current_dir() {
        Ok(path) => writeln!(sink, "{}", path.display()),
        Err(e) => {
            // 写入 err_sink 的失败本身仍 fallback 到顶层 eprintln!，避免双重错误丢失
            writeln!(err_sink, "pwd: {}", e)
        }
    }
}

/// `type` 内建：查询目标名是 builtin、PATH 中可执行文件还是 not found。
/// `builtin` / PATH 命中走 stdout sink；`not found` 走 err_sink（与 bash 行为一致），
/// 可被 `2>` 重定向到文件。无参数时静默（与既有行为一致）。
pub fn run_type(
    sink: &mut dyn Write,
    err_sink: &mut dyn Write,
    args: &[String],
) -> io::Result<()> {
    let Some(target) = args.first() else {
        return Ok(());
    };
    let target = target.as_str();
    if BUILTINS.contains(&target) {
        writeln!(sink, "{} is a shell builtin", target)
    } else if let Some(path) = find_in_path(target) {
        writeln!(sink, "{} is {}", target, path.display())
    } else {
        writeln!(err_sink, "{}: not found", target)
    }
}

/// `cd` 内建：切换当前工作目录。
///
/// 取首个参数作为目标路径；支持绝对路径、相对路径（./、../、子目录名）与 `~`
/// 相对路径由内核基于当前进程 cwd 解析，无需在此做字符串展开。
/// 无参数场景本阶段不覆盖，静默跳过。
/// 注：cd 不产 stdout 输出，故不接 sink；错误信息走 err_sink，可被 `2>` 捕获。
///
/// `~` 展开：本阶段仅匹配精确 `~`，不处理 `~/subdir` 或 `~user` 形式；
/// HOME 缺失时按统一错误格式输出，避免 unwrap 导致 REPL 中断。
///
/// 直接调用 `chdir(2)`：失败时 OS 保证 cwd 不变，无 TOCTOU 风险。
/// 不存在 / 非目录 / 无权限等失败统一打印同一错误信息以匹配测试期望。
/// 错误信息回显用户原始输入 target，不展开为 home 路径（与 bash 行为一致）。
pub fn run_cd(err_sink: &mut dyn Write, args: &[String]) {
    if let Some(target) = args.first() {
        let resolved = if target == "~" {
            match std::env::var("HOME") {
                Ok(home) => home,
                Err(_) => {
                    let _ = writeln!(err_sink, "cd: {}: No such file or directory", target);
                    return;
                }
            }
        } else {
            target.clone()
        };
        if std::env::set_current_dir(&resolved).is_err() {
            let _ = writeln!(err_sink, "cd: {}: No such file or directory", target);
        }
    }
}

/// `complete` 内建：本阶段仅识别 `-p <name>` 形态并输出"未注册规格"错误。
///
/// bash 真实行为：`complete -p git`（未注册）写入 stderr 的错误格式
/// `complete: git: no completion specification`，可被 `2>` 重定向到文件——
/// 故此处错误统一走 `err_sink`，与 `run_type` / `run_cd` 错误信道一致。
///
/// 解析规则（精确匹配，不做泛化）：
/// - `args == ["-p", <name>, ...]` → 向 err_sink 写错误信息（即便后续还有多余参数，
///   也以第二个 token 作为命令名，与 bash 行为一致：`complete -p git foo` 仍打印 git 行）。
/// - 其他形态（无参 / 仅 `-p` / 其他 flag / 单个非 flag 参数）：本阶段题目未规定，
///   静默 `Ok(())` 返回，避免污染 codecrafters 后续阶段预期输出。规格存储与
///   `complete -C` 注册等能力留待后续阶段。
/// `complete` 内建：本阶段支持
///   1. `-C <path> <cmd>`：把 `<cmd> -> <path>` 写入跨命令存活的 `registry`，无输出
///   2. `-r <cmd>`：从 `registry` 删除该命令的补全规则，无输出；未注册命令也按
///      静默成功处理（题面 Notes 明确）。删除后 `-p` 自动落入未命中分支，
///      TAB 补全侧凭共享 `Rc<RefCell<HashMap>>` 自然回退到默认响铃路径
///   3. `-p <cmd>` 命中：向 sink（stdout）输出 `complete -C '<path>' <cmd>`
///   4. `-p <cmd>` 未命中：向 err_sink（stderr）输出
///      `complete: <cmd>: no completion specification`
///   5. 其他形态：静默 `Ok(())`（与 `run_type` 风格一致，避免污染后续阶段预期）
///
/// 多空格归一化由上游 tokenizer 已经完成（dispatch 收到的 args 已是干净 token），
/// 本函数不再处理任何空白；单引号是字面 ASCII 0x27，与 shell 转义无关。
pub fn run_complete(
    sink: &mut dyn Write,
    err_sink: &mut dyn Write,
    args: &[String],
    registry: &mut HashMap<String, String>,
) -> io::Result<()> {
    match args.first().map(|s| s.as_str()) {
        Some("-C") => {
            // `-C <path> <cmd>`：注册补全脚本；后续多余参数忽略（与 bash 容差一致）
            if let (Some(path), Some(cmd)) = (args.get(1), args.get(2)) {
                registry.insert(cmd.clone(), path.clone());
            }
            Ok(())
        }
        Some("-p") => {
            if let Some(cmd) = args.get(1) {
                if let Some(path) = registry.get(cmd) {
                    return writeln!(sink, "complete -C '{}' {}", path, cmd);
                }
                return writeln!(err_sink, "complete: {}: no completion specification", cmd);
            }
            Ok(())
        }
        Some("-r") => {
            // `-r <cmd>`：从 registry 删除该命令的补全规则；无任何输出。
            // 题面 Notes 明确：未注册命令的 `-r` 也按静默成功处理——`HashMap::remove`
            // 返回 `Option<String>`，丢弃即可（`None` 即未注册）。
            // 与 `-C` / `-p` 容差一致：缺第二参或多余参数均静默 `Ok(())`。
            // 删除后 `-p` 自动落入 err_sink "no completion specification" 分支；
            // TAB 补全侧凭同一份 `Rc<RefCell<HashMap>>` 共享表查不到 → 回退默认响铃路径。
            if let Some(cmd) = args.get(1) {
                registry.remove(cmd);
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// `jobs` 内建：列出当前 shell 已知的、仍在运行的后台作业。
///
/// codecrafters 阶段「list a single background job」要求按 bash 兼容格式输出：
///
/// ```text
/// [1]+  Running                 sleep 10
/// ```
///
/// ## 格式契约（精确）
///
/// 单条作业一行，格式 `"[{id}]{mark}  {status:<24}{cmd}\n"`：
/// - `[<id>]`：方括号紧贴 job 编号，无空格
/// - `<mark>`：`+` 表示**最近一个**后台作业，`-` 表示次新，更早的作业无标记。
///   bash 真实行为下「最近作业」即作业表中最后一条；本阶段单作业场景下唯一一条
///   恒为 `+`，但实现已按多作业规则前向兼容（`idx == len-1` → `+`，`idx == len-2`
///   → `-`，其余 → 空格占位以保持列对齐）
/// - **2 个空格**分隔 mark 与 status 字段
/// - `status` 字段总宽 **24 字符**（`{:<24}` 左对齐填充）：`"Running"` 7 字符 +
///   17 个空格 = 24
/// - `cmd`：命令字符串，本阶段用 `parsed.argv.join(" ")` 风格（无尾 `&`、无重定向
///   片段，zsh 风格，tester 容忍）
///
/// ## 信道与重定向语义
///
/// 输出全部走 `sink`（stdout），可被 `>` / `1>` / `>>` / `1>>` 重定向到文件。
/// 本阶段无错误路径——遍历空表也只是 0 行输出，不向 `err_sink` 写任何字节。
/// `args` 暂未使用（题面未规定 flag），保留参数位以与 `run_type` / `run_complete`
/// 签名风格对齐，便于后续阶段加入 `jobs -l` 等 flag 时零调用点改动。
pub fn run_jobs(
    sink: &mut dyn Write,
    _err_sink: &mut dyn Write,
    _args: &[String],
    jobs: &[Job],
) -> io::Result<()> {
    if jobs.is_empty() {
        return Ok(());
    }
    let last_idx = jobs.len() - 1;
    for (idx, job) in jobs.iter().enumerate() {
        // mark 计算：最近作业 `+`，次新 `-`，更早 ` ` 占位。
        // 本阶段单作业场景下 idx == last_idx == 0，恒为 `+`。
        let mark = if idx == last_idx {
            '+'
        } else if idx + 1 == last_idx {
            '-'
        } else {
            ' '
        };
        // 一次性 writeln! 写整行——避免分多次 write 在 `>` 重定向下被其他写入
        // 穿插造成字节顺序问题。`{:<24}` 左对齐填充到 24 字符总宽。
        writeln!(
            sink,
            "[{}]{}  {:<24}{}",
            job.id,
            mark,
            job.status.as_str(),
            job.command
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 跑 `run_complete` 的薄封装：返回 (stdout, stderr) 字符串对，便于断言。
    fn invoke(
        args: &[&str],
        registry: &mut HashMap<String, String>,
    ) -> (String, String) {
        let mut sink: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        run_complete(&mut sink, &mut err, &owned, registry).expect("run_complete");
        (
            String::from_utf8(sink).expect("utf8 stdout"),
            String::from_utf8(err).expect("utf8 stderr"),
        )
    }

    // ---- Stage EP4：complete -r 删除分支回归用例 ----

    #[test]
    fn complete_r_removes_existing_entry() {
        // 先 `-C` 注册 → `-r` 删除 → `-p` 走 err_sink "no completion specification"
        let mut reg: HashMap<String, String> = HashMap::new();

        let (out, err) = invoke(&["-C", "/tmp/completer.sh", "git"], &mut reg);
        assert!(out.is_empty() && err.is_empty(), "-C 无输出");
        assert_eq!(reg.get("git").map(String::as_str), Some("/tmp/completer.sh"));

        let (out, err) = invoke(&["-r", "git"], &mut reg);
        assert!(out.is_empty() && err.is_empty(), "-r 无输出");
        assert!(!reg.contains_key("git"), "registry 已清空");

        let (out, err) = invoke(&["-p", "git"], &mut reg);
        assert!(out.is_empty(), "-p 未命中不写 stdout");
        assert_eq!(err, "complete: git: no completion specification\n");
    }

    #[test]
    fn complete_r_unregistered_silent_ok() {
        // 直接对未注册命令 `-r`：sink/err_sink 均空、registry 仍为空、Ok(())
        let mut reg: HashMap<String, String> = HashMap::new();

        let (out, err) = invoke(&["-r", "git"], &mut reg);
        assert!(out.is_empty(), "未注册 -r 不写 stdout");
        assert!(err.is_empty(), "未注册 -r 不写 stderr");
        assert!(reg.is_empty(), "registry 仍为空");
    }

    #[test]
    fn complete_r_then_recover_via_c() {
        // `-C → -r → -C` 重注册后 `-p` 重新命中，验证 registry 状态机无残留
        let mut reg: HashMap<String, String> = HashMap::new();

        let (_, _) = invoke(&["-C", "/old/path", "git"], &mut reg);
        let (_, _) = invoke(&["-r", "git"], &mut reg);
        let (_, _) = invoke(&["-C", "/new/path", "git"], &mut reg);

        let (out, err) = invoke(&["-p", "git"], &mut reg);
        assert_eq!(out, "complete -C '/new/path' git\n");
        assert!(err.is_empty(), "-p 命中不写 stderr");
    }

    // ---- Stage「list a single background job」：run_jobs 格式契约用例 ----

    /// 跑 `run_jobs` 的薄封装：返回 (stdout, stderr) 字符串对，便于断言。
    fn invoke_jobs(jobs: &[Job]) -> (String, String) {
        let mut sink: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        run_jobs(&mut sink, &mut err, &[], jobs).expect("run_jobs");
        (
            String::from_utf8(sink).expect("utf8 stdout"),
            String::from_utf8(err).expect("utf8 stderr"),
        )
    }

    #[test]
    fn jobs_single_running_exact_format() {
        // 题面 tester 唯一场景：单作业、Running、`+` 标记、24 宽 status 填充
        let jobs = vec![Job {
            id: 1,
            pid: 84470,
            command: "sleep 10".to_string(),
            status: JobStatus::Running,
        }];
        let (out, err) = invoke_jobs(&jobs);
        // 完整逐字节匹配：`[1]+` + 2 空格 + "Running" + 17 空格 + "sleep 10\n"
        assert_eq!(out, "[1]+  Running                 sleep 10\n");
        assert!(err.is_empty(), "jobs 不向 stderr 写字节");
    }

    #[test]
    fn jobs_status_field_padded_to_24_chars() {
        // 显式验证：mark 后的 2 空格分隔之后，status + 填充共 24 字符宽，紧接 cmd
        let jobs = vec![Job {
            id: 1,
            pid: 1,
            command: "x".to_string(),
            status: JobStatus::Running,
        }];
        let (out, _) = invoke_jobs(&jobs);
        // 行格式：`[1]+  ` (6 字节) + status_field (24 字节) + "x\n"
        // 计算 status_field 起止：从 "  " 之后到 "x" 之前
        let prefix = "[1]+  ";
        let suffix = "x\n";
        assert!(out.starts_with(prefix));
        assert!(out.ends_with(suffix));
        let status_field = &out[prefix.len()..out.len() - suffix.len()];
        assert_eq!(status_field.len(), 24, "status 字段总宽必须为 24");
        assert!(status_field.starts_with("Running"));
        // 7 字符 "Running" + 17 空格 = 24
        assert_eq!(&status_field[7..], &" ".repeat(17));
    }

    #[test]
    fn jobs_empty_table_no_output() {
        // 空作业表：sink / err_sink 均零字节，Ok(())
        let (out, err) = invoke_jobs(&[]);
        assert!(out.is_empty(), "空表不写 stdout");
        assert!(err.is_empty(), "空表不写 stderr");
    }

    #[test]
    fn jobs_multi_marker_plus_minus_space() {
        // 多作业前向兼容验证：最近 `+`、次新 `-`、更早空格占位
        // 本阶段 tester 不测，但 mark 计算逻辑需自洽避免未来阶段返工
        let jobs = vec![
            Job {
                id: 1,
                pid: 100,
                command: "a".to_string(),
                status: JobStatus::Running,
            },
            Job {
                id: 2,
                pid: 200,
                command: "b".to_string(),
                status: JobStatus::Running,
            },
            Job {
                id: 3,
                pid: 300,
                command: "c".to_string(),
                status: JobStatus::Running,
            },
        ];
        let (out, _) = invoke_jobs(&jobs);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3);
        // idx=0 → 空格（最早），idx=1 → `-`（次新），idx=2 → `+`（最近）
        assert!(lines[0].starts_with("[1]   "), "最早作业空格占位: {:?}", lines[0]);
        assert!(lines[1].starts_with("[2]-  "), "次新作业 `-`: {:?}", lines[1]);
        assert!(lines[2].starts_with("[3]+  "), "最近作业 `+`: {:?}", lines[2]);
    }
}
