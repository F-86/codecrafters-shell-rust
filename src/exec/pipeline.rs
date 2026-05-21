//! N 段管线执行：`PrevOutput` 三态枚举驱动段间数据流。
//!
//! 语义契约：
//! - 每段独立保留重定向（`>` / `>>` / `2>` / `2>>`），优先级高于 pipe
//! - 末尾 `&` 只作用于整条 pipeline；后台作业 PID 取最后一段（与 bash `$!` 一致）
//! - 任一段可以是 builtin（`is_pipeline_builtin` 范围内）；其它 builtin 按
//!   `command not found` 处理避免污染 shell 状态
//! - SIGPIPE 自然回收：依赖内核默认信号语义
//!
//! 关键正确性：中间所有 `ChildStdout` 必须 move 进下一段 Command（通过 `Stdio::from`），
//! 父进程**禁止**保留任何残留引用——否则父进程持 pipe 写端 fd，下游永远收不到
//! EOF。代码层通过 [`PrevOutput`] 单变量 + `std::mem::replace` 强制 move 保证。
//!
//! 详见 [docs/DESIGN_DECISIONS.md#pipeline-prev-output](../../docs/DESIGN_DECISIONS.md#pipeline-prev-output)。

use std::cell::RefCell;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::rc::Rc;

use crate::builtins::{allocate_job_id, find_in_path, run_echo, run_pwd, run_type, Job, JobStatus};
use crate::parser::Pipeline;
use crate::redirect::open_file_for_redirect;

/// pipeline 段中支持的 builtin 子集：限定为「纯输出 + 无 shell 状态副作用」。
///
/// 仅 `echo` / `pwd` / `type`。排除原因：
/// - `cd` / `exit` 会修改父 shell 状态（cwd / 终止）→ 灾难性副作用
/// - `complete` / `jobs` / `history` / `declare` 依赖 registry / jobs_table / editor 上下文
///
/// 排除的 builtin 在 pipeline 段中按 `command not found` 处理。
fn is_pipeline_builtin(name: &str) -> bool {
    matches!(name, "echo" | "pwd" | "type")
}

/// pipeline 中上一段产出的「数据载体」，作为下一段 stdin 的来源。
///
/// 关键正确性：当 `ChildPipe(child_stdout)` move 进下一段 Command 后，父进程内
/// 不再持有该 pipe 写端 fd 的任何句柄——下游收到 EOF 即可正常退出。
enum PrevOutput {
    /// 首段、或上一段输出被丢弃（如上一段是 builtin 但本段不是首段）。
    None,
    /// 上一段是 builtin：输出全量缓冲在内存，下一段需要 `Stdio::piped()` stdin 后写入。
    /// 典型场景：`echo hello | wc -c`。
    Buffer(Vec<u8>),
    /// 上一段是 external：输出走真正的 OS pipe，下一段直接 `Stdio::from(child_stdout)`。
    /// 典型场景：`cat file | wc -l`。
    ChildPipe(ChildStdout),
}

/// 执行一条 pipeline（N 段命令以 `|` 串联）。
///
/// ## Spawn 失败处理
///
/// 任一段（包括 PATH 解析 / Command::spawn / 文件打开）失败：
/// 1. 把错误信息写入父进程 stderr（pipeline 路径不接 sink）；
/// 2. 把已 spawn 的 Child 全部 `kill()` + `wait()` 回收，避免僵尸；
/// 3. 直接 return，不入 jobs_table（避免半成品作业污染）。
///
/// # Examples
///
/// ```ignore
/// use std::cell::RefCell;
/// use std::rc::Rc;
///
/// // 在 main 的 REPL 主循环中：
/// let jobs_table: Rc<RefCell<Vec<Job>>> = Rc::new(RefCell::new(Vec::new()));
/// let pipeline = parser::parse_pipeline("cat file | wc -l", &vars).unwrap();
/// if pipeline.stages.len() > 1 {
///     run_pipeline(&pipeline, &jobs_table);
/// }
/// ```
pub fn run_pipeline(pipeline: &Pipeline, jobs_table: &Rc<RefCell<Vec<Job>>>) {
    let stages = &pipeline.stages;
    if stages.is_empty() {
        return;
    }
    // 空 argv 段视为语法错误（单段路径由 main.rs 兜底跳过）。
    if stages.iter().any(|s| s.argv.is_empty()) {
        eprintln!("shell: pipeline stage with empty command");
        return;
    }

    let n = stages.len();
    let mut children: Vec<Child> = Vec::with_capacity(n);
    let mut prev: PrevOutput = PrevOutput::None;

    for (i, stage) in stages.iter().enumerate() {
        let is_last = i == n - 1;
        let cmd_name = stage.argv[0].as_str();
        let args = &stage.argv[1..];

        // ---- 段内 stderr 物化：所有段共享（builtin / external 都用同一份）----
        // 段内 `2>` / `2>>` 优先级高于 pipe stderr；未指定 → 继承父进程 stderr。
        let stderr_file = match stage.stderr_redirect.as_deref() {
            Some(target) => match open_file_for_redirect(target, stage.stderr_append) {
                Ok(f) => Some(f),
                Err(e) => {
                    eprintln!("{}: {}: {}", cmd_name, target, e);
                    cleanup_pipeline_children(&mut children);
                    return;
                }
            },
            None => None,
        };

        if is_pipeline_builtin(cmd_name) {
            // ---------- builtin 段：父进程同步执行 ----------
            //
            // builtin 不读 stdin（echo / pwd / type 均无 stdin 消费）：
            // - 上游 prev 为 ChildPipe → drop ChildStdout 关闭读端
            // - 上游 prev 为 Buffer → 直接丢弃缓冲
            //
            // 输出去向（按优先级）：
            //   段内 stdout_redirect → 文件
            //   末段 + 无重定向     → 父进程 stdout（终端）
            //   中段 + 无重定向     → 缓冲到 Vec<u8> 作为下一段 prev
            prev = PrevOutput::None;

            let mut err_sink: Box<dyn Write> = match stderr_file {
                Some(f) => Box::new(f),
                None => Box::new(std::io::stderr()),
            };

            let writes_to_buffer = !is_last && stage.stdout_redirect.is_none();
            let exec_result: std::io::Result<()> = if let Some(target) =
                stage.stdout_redirect.as_deref()
            {
                // 段内 stdout 重定向：File 作 sink
                match open_file_for_redirect(target, stage.stdout_append) {
                    Ok(mut f) => match cmd_name {
                        "echo" => run_echo(&mut f, args),
                        "pwd" => run_pwd(&mut f, &mut *err_sink),
                        "type" => run_type(&mut f, &mut *err_sink, args),
                        _ => unreachable!(),
                    },
                    Err(e) => {
                        eprintln!("{}: {}: {}", cmd_name, target, e);
                        cleanup_pipeline_children(&mut children);
                        return;
                    }
                }
            } else if writes_to_buffer {
                // 中段无重定向：缓冲到 Vec<u8>，下一段 stdin 喂入
                let mut buf: Vec<u8> = Vec::new();
                let r = match cmd_name {
                    "echo" => run_echo(&mut buf, args),
                    "pwd" => run_pwd(&mut buf, &mut *err_sink),
                    "type" => run_type(&mut buf, &mut *err_sink, args),
                    _ => unreachable!(),
                };
                prev = PrevOutput::Buffer(buf);
                r
            } else {
                // 末段无重定向：父进程 stdout 直写
                let stdout = std::io::stdout();
                let mut handle = stdout.lock();
                match cmd_name {
                    "echo" => run_echo(&mut handle, args),
                    "pwd" => run_pwd(&mut handle, &mut *err_sink),
                    "type" => run_type(&mut handle, &mut *err_sink, args),
                    _ => unreachable!(),
                }
            };
            if let Err(e) = exec_result {
                eprintln!("{}: write error: {}", cmd_name, e);
            }
            continue;
        }

        // ---------- external 段：spawn 子进程 ----------
        let Some(path) = find_in_path(cmd_name) else {
            eprintln!("{}: command not found", cmd_name);
            cleanup_pipeline_children(&mut children);
            return;
        };

        // stdin Stdio：i==0 → inherit；上游有数据 → 接管
        // Buffer 路径需要 Stdio::piped() 让父进程 spawn 后向 child.stdin 写入。
        let (stdin_stdio, buffer_to_write): (Stdio, Option<Vec<u8>>) =
            match std::mem::replace(&mut prev, PrevOutput::None) {
                PrevOutput::None => (Stdio::inherit(), None),
                PrevOutput::ChildPipe(cs) => (Stdio::from(cs), None),
                PrevOutput::Buffer(buf) => (Stdio::piped(), Some(buf)),
            };

        // stdout Stdio：段内重定向 > pipe > inherit（末段）
        let stdout_stdio: Stdio = if let Some(target) = stage.stdout_redirect.as_deref() {
            match open_file_for_redirect(target, stage.stdout_append) {
                Ok(f) => Stdio::from(f),
                Err(e) => {
                    eprintln!("{}: {}: {}", cmd_name, target, e);
                    cleanup_pipeline_children(&mut children);
                    return;
                }
            }
        } else if is_last {
            Stdio::inherit()
        } else {
            Stdio::piped()
        };

        // stderr Stdio：段内 stderr_redirect > inherit
        let stderr_stdio: Stdio = match stderr_file {
            Some(f) => Stdio::from(f),
            None => Stdio::inherit(),
        };

        let mut command = Command::new(&path);
        command
            .arg0(cmd_name)
            .args(args)
            .stdin(stdin_stdio)
            .stdout(stdout_stdio)
            .stderr(stderr_stdio);

        let mut child = match command.spawn() {
            Ok(c) => c,
            Err(_) => {
                eprintln!("{}: command not found", cmd_name);
                cleanup_pipeline_children(&mut children);
                return;
            }
        };

        // Buffer 路径：spawn 后立即 write_all 缓冲数据并 drop child.stdin（关闭写端）
        if let (Some(buf), Some(mut stdin)) = (buffer_to_write, child.stdin.take()) {
            let _ = stdin.write_all(&buf);
            drop(stdin); // 显式 drop 强调时序意图
        }

        // 取出本段 ChildStdout（若 piped）作为下一段的 prev
        if !is_last
            && stage.stdout_redirect.is_none()
            && let Some(cs) = child.stdout.take()
        {
            prev = PrevOutput::ChildPipe(cs);
        }

        children.push(child);
    }

    // 全部 spawn 完成。中间所有 ChildStdout 已 move 进下一段 Command 完成 dup2，
    // 父进程不持有 pipe 写端 fd，下游 EOF 链可正常触发。
    drop(prev);

    if pipeline.background {
        // 后台分支：通知行 `[N] PID` 走父进程 stdout（PID 取最后一段，与 bash $! 一致）
        if children.is_empty() {
            // 整条 pipeline 全 builtin：无 Child 入表，直接返回
            return;
        }
        let last_pid = children.last().map(|c| c.id()).unwrap_or(0);
        let id = allocate_job_id(&jobs_table.borrow());
        let _ = writeln!(std::io::stdout(), "[{}] {}", id, last_pid);
        let command_str = stages
            .iter()
            .map(|s| s.argv.join(" "))
            .collect::<Vec<_>>()
            .join(" | ");
        jobs_table.borrow_mut().push(Job {
            id,
            pid: last_pid,
            command: command_str,
            status: JobStatus::Running,
            children,
        });
        return;
    }

    // 前台分支：依次 wait 全部子进程，记录最后一段退出码（暂不暴露 $?）。
    let mut _last_status: Option<std::process::ExitStatus> = None;
    for mut child in children {
        match child.wait() {
            Ok(status) => _last_status = Some(status),
            Err(_) => { /* 防御性：极罕见的 ECHILD 等，忽略 */ }
        }
    }
}

/// pipeline spawn 失败时，把已 spawn 的子进程 kill + wait 回收，避免僵尸。
fn cleanup_pipeline_children(children: &mut Vec<Child>) {
    for child in children.iter_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
    children.clear();
}
