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
use std::process::Child;

/// shell 内建命令清单，作为 `type` 命令查询的单一数据源。
/// 后续阶段新增内建（如 pwd/cd）时只需在此处追加。
pub const BUILTINS: &[&str] = &["echo", "exit", "type", "pwd", "cd", "complete", "jobs"];

/// 后台作业状态。
///
/// 本阶段（codecrafters「Manage Jobs」/ 完成态回收）：
/// - `Running`：进程仍存活，`Child::try_wait()` 返回 `Ok(None)`；
/// - `Done`：进程已正常退出，`Child::try_wait()` 返回 `Ok(Some(_))`，或
///   防御性地把 `Err(_)`（极罕见，如已被外部 reap 导致 ECHILD）也视为 Done。
///
/// `Done` 在 `run_jobs` 渲染一次后立即从作业表中 `retain` 移除，与 bash 行为一致。
/// `Stopped` / 信号终止等状态不在本阶段范围内，故意保留 enum 扩展空间。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStatus {
    Running,
    Done,
}

impl JobStatus {
    /// 状态短串，用于 `jobs` 行内打印（左对齐到 24 字符宽）。
    /// 显式 `&'static str` 映射，避免依赖 `Display` impl，便于后续阶段为不同状态
    /// 加入额外字段时不破坏格式契约。
    pub fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Running => "Running",
            JobStatus::Done => "Done",
        }
    }
}

/// 单个后台作业的元信息。
///
/// 字段与题面「Tracking Background Jobs」四要素一一对应：
/// - `id`：作业编号（`[N]` 中的 N）。Stage「Recycling Job Numbers」起，
///   由 [`allocate_job_id`] 在每次后台 spawn 时基于当前作业表计算「最小可用正整数」
///   （表空→1、`[1,3]`→2、`[2,3]`→1），不再单调递增；
/// - `pid`：单进程作业的 PID；**pipeline 作业**为最后一段（last stage）的 PID，
///   与 bash 的 `$!` 行为一致。
/// - `command`：命令字符串，单进程为 `parsed.argv.join(" ")`；**pipeline 作业**
///   为各段 argv 用 `" | "` 拼接（zsh 风格、无尾 `&`、无重定向片段）；
/// - `status`：当前状态（`Running` / `Done`）；
/// - `children`：作业关联的全部 [`Child`] 句柄。
///   单进程作业为 `vec![child]`；**pipeline 作业**为 `vec![child1, child2, ...]`
///   按管道顺序排列。状态推进与 reap 时**遍历全部** child：任一仍 `Running` 即整个
///   Job 仍 `Running`；全部退出（`Ok(Some(_))` 或 `Err(_)`）才转 `Done`。
///   Job 被 `Vec::retain` 移除时所有 Child 一同 drop——`try_wait` 成功收尾后无僵尸残留。
///
/// 注：因为 `Child` 既不 `Clone` 也不 `Debug`，本结构去除了 `#[derive(Debug, Clone)]`。
/// 本阶段无 Debug 输出与克隆依赖，无需手写 impl。
///
/// `pid` 在本阶段不参与 `jobs` 行内输出（题面格式未要求 PID 列），但保留字段以便
/// 后续阶段 `jobs -l` flag 与 SIGCHLD reap 路径直接复用；`#[allow(dead_code)]`
/// 显式抑制本阶段编译警告。
pub struct Job {
    pub id: u32,
    #[allow(dead_code)]
    pub pid: u32,
    pub command: String,
    pub status: JobStatus,
    pub children: Vec<Child>,
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

/// 非阻塞推进作业表中所有 `Running` 项的状态。
///
/// 实现细节：对每个 `Running` 项**遍历其所有 `children`** 调用 `Child::try_wait()`
/// （`waitpid(WNOHANG)` 风格）：
/// - 任一 child `Ok(None)`（仍存活）→ 整个 Job 保持 `Running`，跳到下一 Job；
/// - 所有 children 均 `Ok(Some(_))` 或 `Err(_)` → 标记为 `Done`。
///   `Ok(Some(_))` 表示已退出且本调用完成 reap；`Err(_)`（极罕见，如已被外部信号
///   处理器 reap 导致 ECHILD）防御性视为 Done，避免僵尸或卡死表项。
///
/// **pipeline 语义**：N 段 pipeline 中任一段（如 `tail -f`）可能因 SIGPIPE 退出，
/// 而其他段（如 `head -n 5`）则因正常退出条件结束。本函数等所有段都终止才转 Done，
/// 与 bash `wait` pipeline 的行为一致。
///
/// 本函数为「Reaping Before Each Prompt」三函数原子拆分中的第一步：仅推进状态，
/// 不渲染任何字节、不修改 Vec 长度。渲染由 [`render_done_jobs`] 承担，移除由
/// [`retain_running_jobs`] 承担。两条调用路径（REPL prompt 前自动 reap、
/// `run_jobs` 入口兜底）都先调本函数完成状态推进。
pub fn advance_job_status(jobs: &mut [Job]) {
    for job in jobs.iter_mut() {
        if job.status != JobStatus::Running {
            continue;
        }
        // 遍历该 Job 的所有 child；任一仍 Running 即保持 Running，
        // 全部退出（Ok(Some) 或 Err）才转 Done。
        let mut all_finished = true;
        for child in job.children.iter_mut() {
            match child.try_wait() {
                Ok(Some(_)) => {}                  // 已退出并 reap
                Ok(None) => all_finished = false,  // 仍存活
                Err(_) => {}                       // 防御性：ECHILD，视为已结束
            }
        }
        if all_finished {
            job.status = JobStatus::Done;
        }
    }
}

/// 仅渲染作业表中已 `Done` 项的标准 Done 行；不修改 Vec。
///
/// **关键：marker 基于「Done + 仍 Running 全集」的索引计算**——遍历完整 `&[Job]`，
/// 对每个 idx 算 mark（`last_idx → '+'`、`last_idx-1 → '-'`、其他 → ` `），
/// 但仅当 `status == Done` 时 `writeln!` 输出。Running 项贡献索引基线但不输出。
/// 这复刻 bash 行为：题面示例「`sleep 5 &; sleep 100 &`，job 1 完成时输出
/// `[1]-  Done                    sleep 5`」——彼时 job 2 是 last，job 1 是
/// last-1 故 `-`。
///
/// 行格式（与 [`run_jobs`] 完全一致）：
/// ```text
/// [<id><mark>  Done                    <command>\n
/// ```
/// - mark 后 2 空格分隔
/// - status 字段总宽 24（`{:<24}`，`"Done"` 4 字符 + 20 空格）
/// - Done 行**不**追加尾 ` &`（与 Running 行的尾 ` &` 区分）
///
/// 调用方：
/// - **REPL prompt 前自动 reap 路径**：`sink = io::stdout().lock()`，写完 flush
///   让 Done 行先于 prompt 落盘
/// - `run_jobs` **不**调用本函数（避免 sink + stdout 双写重复）
///
/// 空表 / 全 Running 时不写任何字节，返回 `Ok(())`。
pub fn render_done_jobs(sink: &mut dyn Write, jobs: &[Job]) -> io::Result<()> {
    if jobs.is_empty() {
        return Ok(());
    }
    let last_idx = jobs.len() - 1;
    for (idx, job) in jobs.iter().enumerate() {
        if job.status != JobStatus::Done {
            continue;
        }
        let mark = if idx == last_idx {
            '+'
        } else if idx + 1 == last_idx {
            '-'
        } else {
            ' '
        };
        writeln!(
            sink,
            "[{}]{}  {:<24}{}",
            job.id,
            mark,
            job.status.as_str(),
            job.command,
        )?;
    }
    Ok(())
}

/// 分配下一个后台作业编号：返回当前 `jobs` 表中**最小未被占用**的正整数。
///
/// codecrafters「Recycling Job Numbers」阶段语义：
/// - 表空 → `1`
/// - `[1, 2]` → `3`（无间隙时仍是「下一个」）
/// - `[1, 3]` → `2`（中间间隙优先复用）
/// - `[2, 3]` → `1`（首位空缺也复用）
///
/// 设计要点：
/// - **纯函数、无副作用**：仅查询 `jobs` 切片，不修改任何状态。
///   作业编号不再由独立计数器持有——`jobs_table` 是唯一权威，
///   分配是它的派生函数。这彻底消除「计数器与表脱节」的双源真理风险。
/// - **算法**：线性扫描 `(1u32..)` 返回首个不在 `jobs.iter().map(|j| j.id)` 集合中的值。
///   最坏 O(n²)（n = 表长），但本阶段后台作业表典型 ≤ 几十项，
///   常量因子极小，远低于 `Command::spawn` 的毫秒级成本，YAGNI 不引入 HashSet。
/// - **空表 → 1**：`(1u32..)` 首次循环 `n=1`，闭包 `!jobs.iter().any(...)`
///   在空表上恒为 `true`，直接返回 1，无需特判。
/// - **`unwrap` 安全**：u32 上界 4G，作业表实际不可能填满；
///   理论上 `(1u32..)` 是有限范围，但实践中不可达。
pub fn allocate_job_id(jobs: &[Job]) -> u32 {
    (1u32..)
        .find(|n| !jobs.iter().any(|j| j.id == *n))
        .expect("allocate_job_id: u32 range exhausted (unreachable)")
}

/// 一次性 retain 移除作业表中所有 `Done` 项。
///
/// `Child` 已被 [`advance_job_status`] 中的 `try_wait` 成功收尾，drop 即无僵尸残留。
/// 与渲染解耦——调用方需自行确保渲染（如有）已完成再调本函数；本阶段两条路径
/// （prompt 前自动 reap、`run_jobs`）都在渲染后才调用。
///
/// 空表场景下 `retain` 自然 no-op。
pub fn retain_running_jobs(jobs: &mut Vec<Job>) {
    jobs.retain(|j| j.status != JobStatus::Done);
}

/// `jobs` 内建：列出当前 shell 已知的、仍在运行或刚刚完成的后台作业。
///
/// codecrafters「Manage Jobs」阶段要求按 bash 兼容格式输出已完成作业一次，
/// 然后从作业表中移除：
///
/// ```text
/// [1]+  Running                 sleep 10 &
/// [1]+  Done                    cat /tmp/fifo
/// ```
///
/// ## 格式契约（精确）
///
/// 单条作业一行，格式 `"[{id}]{mark}  {status:<24}{cmd}{tail}\n"`：
/// - `[<id>]`：方括号紧贴 job 编号，无空格
/// - `<mark>`：`+` 表示**最近一个**后台作业，`-` 表示次新，更早的作业无标记。
/// - **2 个空格**分隔 mark 与 status 字段
/// - `status` 字段总宽 **24 字符**（`{:<24}` 左对齐填充）：
///   - `"Running"` 7 字符 + 17 空格 = 24
///   - `"Done"` 4 字符 + 20 空格 = 24
/// - `cmd`：命令字符串，本阶段用 `parsed.argv.join(" ")` 风格（无尾 `&`、无重定向片段）
/// - `tail`：`Running` 行追加 `" &"`（与 bash 行为一致），`Done` 行不追加
///
/// ## 一次性移除（Done 行）
///
/// 渲染完毕后单次 `Vec::retain(|j| j.status != Done)` 一次性删除所有 `Done` 项；
/// `Child` 已被 `try_wait` 成功收尾，随 Job 从 Vec 中 drop 时无僵尸残留。
/// 下一次 `jobs` 调用不再列出该项。
///
/// ## 信道与重定向语义
///
/// 输出全部走 `sink`（stdout），可被 `>` / `1>` / `>>` / `1>>` 重定向到文件。
/// 本阶段无错误路径——遍历空表也只是 0 行输出，不向 `err_sink` 写任何字节。
/// `args` 暂未使用（题面未规定 flag），保留参数位以与 `run_type` / `run_complete`
/// 签名风格对齐，便于后续阶段加入 `jobs -l` 等 flag 时零调用点改动。
///
/// 注：本函数同时承担「入口处再次 reap」的兜底职责，确保即便 prompt 前 reap
/// 未触发也能在 `jobs` 时即时反映状态变化。
pub fn run_jobs(
    sink: &mut dyn Write,
    _err_sink: &mut dyn Write,
    _args: &[String],
    jobs: &mut Vec<Job>,
) -> io::Result<()> {
    // 入口处再做一次状态推进（兜底）：覆盖「prompt 前 reap → 立即 jobs」之间的
    // 边界窗口，与 REPL prompt 前自动 reap 路径共享同一原子函数，行为一致。
    advance_job_status(jobs);

    if jobs.is_empty() {
        return Ok(());
    }
    let last_idx = jobs.len() - 1;
    for (idx, job) in jobs.iter().enumerate() {
        // mark 计算：最近作业 `+`，次新 `-`，更早 ` ` 占位。
        // 渲染顺序使用「retain 之前」的索引，与 bash 在单作业完成场景下的输出一致。
        let mark = if idx == last_idx {
            '+'
        } else if idx + 1 == last_idx {
            '-'
        } else {
            ' '
        };
        // 一次性 writeln! 写整行——避免分多次 write 在 `>` 重定向下被其他写入
        // 穿插造成字节顺序问题。`{:<24}` 左对齐填充到 24 字符总宽。
        // Running 行追加 " &"（与 bash 行为一致），Done 行不追加。
        let tail = match job.status {
            JobStatus::Running => " &",
            JobStatus::Done => "",
        };
        writeln!(
            sink,
            "[{}]{}  {:<24}{}{}",
            job.id,
            mark,
            job.status.as_str(),
            job.command,
            tail,
        )?;
    }
    // 一次性移除所有 Done：Child 已 reap，drop 即无残留。
    // 共用 retain_running_jobs 与自动 reap 路径保持单一移除来源。
    retain_running_jobs(jobs);
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

    // ---- Stage「Manage Jobs」：run_jobs 格式契约 + reap 行为用例 ----

    /// 构造一个仍在运行的 Job：spawn `sleep 30`，长存活足够覆盖测试窗口。
    /// 用例结束时由 RAII guard 兜底 `kill` + `wait`，避免子进程残留。
    fn spawn_running_job(id: u32, command: &str) -> Job {
        let child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep 30");
        let pid = child.id();
        Job {
            id,
            pid,
            command: command.to_string(),
            status: JobStatus::Running,
            children: vec![child],
        }
    }

    /// 构造一个已退出但尚未 reap 的 Job：spawn `true`，主动 `wait()` 让进程退出。
    /// 注意：此处直接调 `wait` 让进程退出并 reap，但状态仍标记 Running——
    /// 这模拟 reap_finished_jobs 调用前的初始态。
    /// 由于 `wait` 会消费内部 ExitStatus，后续 `try_wait` 在已 reap 的子进程上
    /// 会返回 `Err(ECHILD)` —— 这正好命中「Err 视为 Done」防御性分支。
    /// 真实场景下走的是 `try_wait` 的 `Ok(Some(_))` 分支（进程退出但未被 wait），
    /// 此处用 `true` + `wait` 仅为单测可控性；reap 推进的语义等价。
    fn spawn_exited_job(id: u32, command: &str) -> Job {
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn true");
        // 阻塞等子进程退出 + reap，确保 try_wait 之后必然返回 Err 或 Ok(Some)
        let _ = child.wait();
        let pid = child.id();
        Job {
            id,
            pid,
            command: command.to_string(),
            status: JobStatus::Running,
            children: vec![child],
        }
    }

    /// 杀掉 Running Job 的所有 child，避免测试遗留。Done Job 已被 reap，无需处理。
    fn kill_job(job: &mut Job) {
        for child in job.children.iter_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// 跑 `run_jobs` 的薄封装：返回 (stdout, stderr) 字符串对，便于断言。
    fn invoke_jobs(jobs: &mut Vec<Job>) -> (String, String) {
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
        // 题面 tester 场景：单作业、Running、`+` 标记、24 宽 status 填充、尾 `&`
        let mut jobs = vec![spawn_running_job(1, "sleep 10")];
        let (out, err) = invoke_jobs(&mut jobs);
        // 完整逐字节匹配：`[1]+` + 2 空格 + "Running" + 17 空格 + "sleep 10 &\n"
        assert_eq!(out, "[1]+  Running                 sleep 10 &\n");
        assert!(err.is_empty(), "jobs 不向 stderr 写字节");
        // Running 不被 retain 移除
        assert_eq!(jobs.len(), 1);
        for j in &mut jobs {
            kill_job(j);
        }
    }

    #[test]
    fn jobs_status_field_padded_to_24_chars() {
        // 显式验证：mark 后的 2 空格分隔之后，status + 填充共 24 字符宽，紧接 cmd
        let mut jobs = vec![spawn_running_job(1, "x")];
        let (out, _) = invoke_jobs(&mut jobs);
        // 行格式：`[1]+  ` (6 字节) + status_field (24 字节) + "x &\n"
        let prefix = "[1]+  ";
        let suffix = "x &\n";
        assert!(out.starts_with(prefix));
        assert!(out.ends_with(suffix));
        let status_field = &out[prefix.len()..out.len() - suffix.len()];
        assert_eq!(status_field.len(), 24, "status 字段总宽必须为 24");
        assert!(status_field.starts_with("Running"));
        // 7 字符 "Running" + 17 空格 = 24
        assert_eq!(&status_field[7..], &" ".repeat(17));
        for j in &mut jobs {
            kill_job(j);
        }
    }

    #[test]
    fn jobs_empty_table_no_output() {
        // 空作业表：sink / err_sink 均零字节，Ok(())
        let mut jobs: Vec<Job> = Vec::new();
        let (out, err) = invoke_jobs(&mut jobs);
        assert!(out.is_empty(), "空表不写 stdout");
        assert!(err.is_empty(), "空表不写 stderr");
    }

    #[test]
    fn jobs_done_renders_without_trailing_amp_and_retained_out() {
        // 已退出作业：reap 推进至 Done，渲染一次（无尾 `&`、24 宽 Done），
        // 渲染后从 Vec 中移除——再调一次 invoke_jobs 即空输出。
        let mut jobs = vec![spawn_exited_job(1, "true")];
        let (out, err) = invoke_jobs(&mut jobs);
        // Done 行：`[1]+` + 2 空格 + "Done" + 20 空格 + "true\n"（无尾 `&`）
        assert_eq!(out, "[1]+  Done                    true\n");
        assert!(err.is_empty());
        // retain 后 Vec 为空
        assert!(jobs.is_empty(), "Done 项渲染后必须被一次性移除");
        // 再次调用：无任何输出
        let (out2, err2) = invoke_jobs(&mut jobs);
        assert!(out2.is_empty(), "二次 jobs 不再列出已 Done 项");
        assert!(err2.is_empty());
    }

    #[test]
    fn jobs_mixed_running_and_done_retain_only_running() {
        // 混合：一条 Running + 一条 Done。渲染两行，retain 后仅留 Running。
        // 注意构造顺序：先 Running 再 Done，则 last_idx 指向 Done（idx=1 → `+`），
        // Running（idx=0）为次新 → `-`。
        let mut jobs = vec![
            spawn_running_job(1, "sleep 10"),
            spawn_exited_job(2, "true"),
        ];
        let (out, _) = invoke_jobs(&mut jobs);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        // idx=0 Running → `-`、尾 ` &`
        assert_eq!(lines[0], "[1]-  Running                 sleep 10 &");
        // idx=1 Done → `+`、无尾 `&`
        assert_eq!(lines[1], "[2]+  Done                    true");
        // retain 后仅剩 Running
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, 1);
        assert_eq!(jobs[0].status, JobStatus::Running);
        for j in &mut jobs {
            kill_job(j);
        }
    }

    // ---- Stage「Reaping Before Each Prompt」：三函数原子拆分契约用例 ----

    #[test]
    fn advance_job_status_only_promotes_running_to_done_no_len_change() {
        // 契约：advance_job_status 只对 Running 项调 try_wait 推进状态；
        // 不渲染、不修改 Vec 长度；Done 项保持 Done。
        let mut jobs = vec![
            spawn_running_job(1, "sleep 30"),  // 仍 Running
            spawn_exited_job(2, "true"),       // 已退出，进入函数前 status 仍是 Running
        ];
        let len_before = jobs.len();
        advance_job_status(&mut jobs);
        // len 不变
        assert_eq!(jobs.len(), len_before);
        // job 1 仍 Running（sleep 30 远未结束）
        assert_eq!(jobs[0].status, JobStatus::Running);
        // job 2 被推进为 Done（spawn_exited_job 已 wait 过，try_wait Err 命中 Done 分支）
        assert_eq!(jobs[1].status, JobStatus::Done);
        // 清理 Running
        for j in &mut jobs {
            kill_job(j);
        }
    }

    #[test]
    fn render_done_jobs_marker_uses_union_view() {
        // 契约：render_done_jobs 的 marker 基于「Done + Running 全集」索引计算，
        // 仅对 Done 项 writeln，Running 项贡献索引基线但不输出。
        //
        // 题面示例 1 关键场景：jobs = [Done(id=1, "sleep 5"), Running(id=2, "sleep 100")]
        // last_idx = 1 → Running；idx=0 (Done) 是 last-1 → mark 为 `-`。
        // 期望仅渲染 1 行（Done 的）：`[1]-  Done                    sleep 5\n`
        let mut jobs = vec![
            spawn_exited_job(1, "sleep 5"),
            spawn_running_job(2, "sleep 100"),
        ];
        // 必须先推进，否则 spawn_exited_job 的 status 还是 Running
        advance_job_status(&mut jobs);
        assert_eq!(jobs[0].status, JobStatus::Done);
        assert_eq!(jobs[1].status, JobStatus::Running);

        let mut sink: Vec<u8> = Vec::new();
        render_done_jobs(&mut sink, &jobs).expect("render_done_jobs");
        let out = String::from_utf8(sink).expect("utf8");
        // 仅 1 行 Done，marker 是 `-`（不是 `+`），无尾 ` &`
        assert_eq!(out, "[1]-  Done                    sleep 5\n");
        // 不修改 Vec 长度
        assert_eq!(jobs.len(), 2);
        for j in &mut jobs {
            kill_job(j);
        }
    }

    #[test]
    fn render_done_jobs_empty_or_all_running_no_output() {
        // 边界：空表 / 全 Running 时 render_done_jobs 不写任何字节。

        // 空表
        let jobs_empty: Vec<Job> = Vec::new();
        let mut sink: Vec<u8> = Vec::new();
        render_done_jobs(&mut sink, &jobs_empty).expect("empty ok");
        assert!(sink.is_empty(), "空表不写任何字节");

        // 全 Running
        let mut jobs = vec![
            spawn_running_job(1, "sleep 30"),
            spawn_running_job(2, "sleep 30"),
        ];
        let mut sink: Vec<u8> = Vec::new();
        render_done_jobs(&mut sink, &jobs).expect("all running ok");
        assert!(sink.is_empty(), "全 Running 不写任何字节");
        for j in &mut jobs {
            kill_job(j);
        }
    }

    #[test]
    fn retain_running_jobs_removes_all_done_in_one_pass() {
        // 契约：retain_running_jobs 一次性删除所有 Done 项；保留所有非 Done。
        let mut jobs = vec![
            spawn_exited_job(1, "true"),
            spawn_running_job(2, "sleep 30"),
            spawn_exited_job(3, "true"),
        ];
        // 先推进，让 1/3 进入 Done
        advance_job_status(&mut jobs);
        assert_eq!(jobs[0].status, JobStatus::Done);
        assert_eq!(jobs[1].status, JobStatus::Running);
        assert_eq!(jobs[2].status, JobStatus::Done);

        retain_running_jobs(&mut jobs);
        // 仅 id=2 仍在
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, 2);
        assert_eq!(jobs[0].status, JobStatus::Running);

        // 再次调用：no-op
        retain_running_jobs(&mut jobs);
        assert_eq!(jobs.len(), 1);

        for j in &mut jobs {
            kill_job(j);
        }
    }

    // ---- Stage「Recycling Job Numbers」：allocate_job_id 最小可用分配契约 ----

    #[test]
    fn allocate_empty_table_returns_one() {
        // 表空 → 1：`(1u32..).find(...)` 首次循环 n=1，空表上 any() 恒为 false，
        // 闭包返回 true，命中。
        let jobs: Vec<Job> = Vec::new();
        assert_eq!(allocate_job_id(&jobs), 1);
    }

    #[test]
    fn allocate_sequential_after_running_jobs() {
        // [1, 2] → 3：无间隙时返回「下一个」编号，与单调递增计数器在该场景下等价。
        let mut jobs = vec![
            spawn_running_job(1, "sleep 30"),
            spawn_running_job(2, "sleep 30"),
        ];
        assert_eq!(allocate_job_id(&jobs), 3);
        for j in &mut jobs {
            kill_job(j);
        }
    }

    #[test]
    fn allocate_reuse_smallest_gap() {
        // [1, 3] → 2：中间间隙优先复用，复刻题面示例「if you have jobs [1] and [3]
        // running, the next job gets [2], not [4]」。
        let mut jobs = vec![
            spawn_running_job(1, "sleep 30"),
            spawn_running_job(3, "sleep 30"),
        ];
        assert_eq!(allocate_job_id(&jobs), 2);
        for j in &mut jobs {
            kill_job(j);
        }
    }

    #[test]
    fn allocate_reuse_after_first_removed() {
        // [2, 3] → 1：首位空缺也回收。复刻 tester 流程 A 中
        // 「全部完成 + reap 移除 + 表空」之外的另一边界——表非空但 1 号空缺。
        let mut jobs = vec![
            spawn_running_job(2, "sleep 30"),
            spawn_running_job(3, "sleep 30"),
        ];
        assert_eq!(allocate_job_id(&jobs), 1);
        for j in &mut jobs {
            kill_job(j);
        }
    }
}
