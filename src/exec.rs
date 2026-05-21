//! 外部命令分支：在 PATH 命中后用 `Command` spawn 子进程，按 `ParsedCommand` 物化
//! stdout / stderr 的 Stdio（重定向文件或 inherit 终端），失败时降级回错误提示。
//!
//! 把这段从 REPL 主循环抽出后，`main.rs` 不再需要 `std::process::Command` / `Stdio`
//! / `CommandExt` 等 use；外部命令未命中 PATH 时仍通过传入的 `err_sink` 写
//! "command not found"（可被 `2>` 捕获），保持与拆分前完全一致的语义。
//!
//! ## 后台执行（`&` 末尾标记）
//!
//! 当 [`ParsedCommand::background`] 为 `true` 时，本函数走「后台路径」：用
//! [`Command::spawn`] 启动子进程后**不调 `wait`/`status`**，立即向父进程 stdout
//! 打印 `[<job>] <pid>` 通知，并把 [`Child`] 句柄 move 进 [`Job`] 字段、入
//! `jobs_table` 持有，然后返回。后续由主循环 prompt 前与 `run_jobs` 入口的
//! `reap_finished_jobs` 通过 `Child::try_wait()` 非阻塞推进状态——退出后的子进程
//! 由 try_wait 完成 reap，无僵尸残留；仍在运行的子进程其 Child 句柄保留在 Job
//! 中，与 bash 作业控制语义对齐。
//!
//! 通知行 `[N] PID` 走父进程 stdout（直接 `println!`），**不复用** `sink`——
//! 这与 bash 真实行为一致：job 控制信息属于 shell 自身的元信息，不应被用户的
//! `>` / `1>` 重定向捕获到文件。
//!
//! ## Background Job stdio 继承（Stage: Background Job Output）
//!
//! codecrafters「Background Job Output」阶段要求后台进程的 stdout / stderr 仍连接
//! 到 shell 终端，使 `cat /path/to/fifo &` 在 FIFO 收到写入后能直接把内容打到用户屏幕。
//!
//! 本模块当前实现**已满足该要求，无需任何代码改动**——关键事实链：
//!
//! 1. 后台分支与前台分支共用 L71-90 的 stdio 物化逻辑：未配置 `stdout_redirect` /
//!    `stderr_redirect` 时一律使用 [`Stdio::inherit()`]，即「让子进程继承父 shell
//!    当前的 stdout / stderr fd」。
//! 2. [`Command::spawn`] 在 fork 后通过 `dup2(2)` 把父进程当前 stdout / stderr 复制
//!    到子进程的同号 fd 上。**fd 在复制后即独立存活**，其可写性与父进程后续是否
//!    `wait`、[`Child`] 句柄是否 `drop` **完全无关**——这是 POSIX fd 继承的标准
//!    语义。
//! 3. 由于 shell 启动时 stdout / stderr 直接挂在控制终端（tty）上，子进程通过 dup2
//!    拿到的就是同一终端 fd 的副本；后台 `cat` 阻塞在 FIFO 读时，shell 已返回到下
//!    一轮 readline；FIFO 一旦被写入，`cat` 唤醒后写自己的 fd1 / fd2 直达终端。
//! 4. 与 rustyline raw mode 的边界：raw mode 改变的是 shell **自身读 stdin** 的回
//!    显与行缓冲行为，**不影响** 其他进程**写**终端 fd 的可见性——子进程的 `write(2)`
//!    系统调用对 tty 而言无视 raw / cooked 模式，输出立即可见（可能与 prompt 交错）。
//!
//! 因此 codecrafters tester（`cat /path/to/fifo1 &` 后接 `cat /path/to/fifo2`，再异步
//! 向两个 FIFO 写入）所要求的「前后台输出均直达 shell 终端」直接由现有 `Stdio::inherit()`
//! 满足。集成测试见 `tests/background_stdio.rs`。

use std::cell::RefCell;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::rc::Rc;

use crate::builtins::{allocate_job_id, find_in_path, Job, JobStatus};
use crate::parser::ParsedCommand;
use crate::redirect::open_file_for_redirect;

/// 执行非内建命令。
///
/// 参数：
/// - `cmd`：用户输入的命令名（用作 `argv[0]`，与 bash 行为一致——不替换为绝对路径）；
/// - `line`：用户输入的整行原文，仅在 not-found / spawn 失败时拼接 "command not found" 错误使用；
/// - `args`：除命令名外的参数切片；
/// - `parsed`：完整解析结果，提供 stdout/stderr 重定向元信息与 `background` 标志；
/// - `sink` / `err_sink`：本轮已打开的写句柄。**所有权由调用方移交给本函数**——
///   在 spawn 子进程前 `drop` 掉，避免与子进程对同一文件 fd 产生竞态。
/// - `jobs_table`：跨 REPL 存活的后台作业表共享句柄（`Rc<RefCell<Vec<Job>>>`）。
///   仅在后台分支 spawn 成功后 `borrow_mut().push(Job{...})`；前台分支与 spawn 失败
///   时不触碰此表。
///
///   Stage「Recycling Job Numbers」起，作业编号改由 [`allocate_job_id`] 基于
///   当前表内容计算「最小可用正整数」（表空→1、`[1,3]`→2、`[2,3]`→1），
///   不再持有独立计数器——`jobs_table` 是唯一权威，分配是它的派生函数。
///   通知行 `[N] PID` 与 `Job.id` 共享同一 `let id = allocate_job_id(...)` 局部，
///   保证两处严格一致。
///
///   RefCell 借用规则：先 `let id = allocate_job_id(&jobs_table.borrow());`
///   （临时不可变借用，语句末即 drop），再 `jobs_table.borrow_mut().push(...)`，
///   两段借用区间不重叠，无双借 panic。
///
/// 关键路径注释：
/// - sink 走的是 `Box<dyn Write>` 抽象，运行时无法把它「拆回」具体类型，
///   故对外部命令重新从 `parsed.{stdout,stderr}_redirect` 物化一次 stdio。
///   共享 `open_file_for_redirect` helper 保证此处与 sink 打开模式一致：
///   append 标志为 true 时复用 `OpenOptions.append(true)`，文件原有内容保留；
///   为 false 时复用 `File::create`（截断）。
/// - err_sink 已在上游 `open_err_sink` 阶段打开过同一文件；这里再次打开属同一模式
///   （append / truncate），对 truncate 模式而言是再次 truncate 为空（语义不变），
///   对 append 模式而言只是另开一个 fd 共享 O_APPEND 语义（内核保证两 fd 写入都
///   原子追加到末尾，不会互相覆盖）。
/// - spawn / wait 失败时降级，避免 REPL 中断；此时 err_sink 已被 drop，
///   回退到父进程 stderr（仍可见但不被 `2>` 捕获——属罕见极端情况，可接受的退化语义）。
pub fn run_external(
    cmd: &str,
    line: &str,
    args: &[String],
    parsed: &ParsedCommand,
    sink: Box<dyn Write>,
    mut err_sink: Box<dyn Write>,
    jobs_table: &Rc<RefCell<Vec<Job>>>,
) {
    let Some(path) = find_in_path(cmd) else {
        // 未在 PATH 命中：command not found 走 err_sink（可被 `2>` 捕获）
        let _ = writeln!(&mut *err_sink, "{}: command not found", line);
        return;
    };

    // 在 spawn 子进程之前**释放** sink / err_sink 持有的文件 fd 拥有权——
    // 重定向时把 File 直接转移给 Command::stdout / .stderr，由子进程 inherit；
    // 无重定向时让父进程继承自己的 stdout / stderr（默认即 inherit）。
    let stdio = match parsed.stdout_redirect.as_deref() {
        Some(target) => match open_file_for_redirect(target, parsed.stdout_append) {
            Ok(f) => Stdio::from(f),
            Err(e) => {
                eprintln!("{}: {}: {}", cmd, target, e);
                return;
            }
        },
        None => Stdio::inherit(),
    };
    let err_stdio = match parsed.stderr_redirect.as_deref() {
        Some(target) => match open_file_for_redirect(target, parsed.stderr_append) {
            Ok(f) => Stdio::from(f),
            Err(e) => {
                eprintln!("{}: {}: {}", cmd, target, e);
                return;
            }
        },
        None => Stdio::inherit(),
    };
    // 提前丢弃父进程内 sink / err_sink 对同一文件的写句柄，避免与子进程
    // 的截断/写入产生竞态（虽然内核 fd 独立，但语义上更干净）。
    drop(sink);
    drop(err_sink);

    // argv[0] 必须是用户输入的命令名而非完整路径（与 bash 行为一致）
    let mut command = Command::new(&path);
    command
        .arg0(cmd)
        .args(args)
        .stdout(stdio)
        .stderr(err_stdio);

    if parsed.background {
        // 后台分支：spawn 不 wait，立即打印 `[N] PID` 通知后返回。
        // - `Child` 被 drop 不触发 wait（Rust 默认实现），子进程作为孤儿存活；
        // - 通知走父进程 stdout（`println!`），不复用已 drop 的 sink——这与 bash
        //   一致：job 控制信息不被用户 `>` 重定向捕获到文件；
        // - spawn 失败按既有路径走 `command not found`，**不**触发任何 jobs_table 写入。
        //
        // Stage「Background Job Output」语义锁定：进入此分支时 `stdio` / `err_stdio`
        // 在无重定向情况下已是 `Stdio::inherit()`（见上方 L71-90），spawn 时通过 dup2
        // 复制父进程的终端 fd 给子进程。fd 复制独立存活，与 `Child` 是否被 wait/drop
        // 无关——后台 `cat /path/to/fifo` 阻塞读时 shell 已返回 prompt，FIFO 写入到
        // 达后 cat 仍能直接写到继承来的终端 fd，输出对用户可见。详见模块头注释。
        match command.spawn() {
            Ok(child) => {
                let pid = child.id();
                // Stage「Recycling Job Numbers」：基于 jobs_table 当前内容算「最小可用」
                // 正整数作为本次后台作业编号。表空→1、`[1,3]`→2、`[2,3]`→1。
                // 借用即用即释放——`jobs_table.borrow()` 是临时表达式，语句末
                // drop，确保下方 `borrow_mut()` 不与之重叠（无 RefCell 双借 panic）。
                let id = allocate_job_id(&jobs_table.borrow());
                // 通知行格式严格 `[<job>] <pid>\n`，方括号紧贴数字，单空格分隔。
                // 失败（stdout 被关闭等罕见情形）静默吞掉，不阻断 REPL。
                let _ = writeln!(std::io::stdout(), "[{}] {}", id, pid);
                // 入表：使用同一 `id` 局部，与通知行 `[N] PID` 中的 N 严格一致。
                // command 字符串用 `parsed.argv.join(" ")` 风格（无尾 `&`、无重定向片段，
                // zsh 风格，tester 容忍）。
                //
                // Stage「Manage Jobs」：`child` 句柄 move 进 Job 字段，由 jobs_table
                // 持有跨 REPL 存活。后续 `reap_finished_jobs` 用 `Child::try_wait()`
                // 非阻塞推进状态，`run_jobs` 渲染 Done 后由 `Vec::retain` 移除——
                // Job drop 时 Child 随之 drop。`try_wait` 已对退出子进程完成 reap，
                // 因此 `Child::drop` 默认不 wait 也不会留下僵尸。
                jobs_table.borrow_mut().push(Job {
                    id,
                    pid,
                    command: parsed.argv.join(" "),
                    status: JobStatus::Running,
                    child,
                });
            }
            Err(_) => {
                eprintln!("{}: command not found", line);
            }
        }
        return;
    }

    // 前台分支：保持既有 `.status()` 同步等待语义不变。
    let status = command.status();
    if status.is_err() {
        eprintln!("{}: command not found", line);
    }
}
