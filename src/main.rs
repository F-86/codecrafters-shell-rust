use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::rc::Rc;

mod builtins;
mod completion;
mod exec;
mod parser;
mod redirect;

use builtins::{run_cd, run_complete, run_echo, run_jobs, run_pwd, run_type, Job};
use completion::ShellHelper;
use exec::run_external;
use redirect::{open_err_sink, open_sink};
use rustyline::error::ReadlineError;
use rustyline::{Config, CompletionType, Editor};

fn main() {
    // 1. 初始化 rustyline Editor，并安装自定义 Helper（提供 TAB 补全）。
    //    - `CompletionType::List`：单候选时直接用 `replacement` 替换 line buffer 并刷新；
    //      与我们 Helper 已经手算好 `replacement`（含尾空格 / 尾 `/`）的语义对齐。
    //      默认的 `Circular` 在单候选时会进入"按 TAB 切下一候选"的内部循环，第二次 TAB
    //      不会再调 `Completer::complete`，直接把 line 回退到 backup —— 与「`dir/<TAB>` 进入
    //      下一层」的链式补全语义冲突，故必须显式选 `List`。
    //    - 其余配置走 rustyline 默认值。
    //    - 若构造失败（极少见，如终端能力探测异常），打印错误后直接退出。
    let config = Config::builder()
        .completion_type(CompletionType::List)
        .build();
    let mut editor: Editor<ShellHelper, _> = match Editor::with_config(config) {
        Ok(ed) => ed,
        Err(e) => {
            eprintln!("shell: failed to init readline: {}", e);
            std::process::exit(1);
        }
    };
    // `complete -C <path> <cmd>` 注册的补全脚本表（command → completer path）。
    // 放在 REPL 主循环外以跨命令存活。
    //
    // 用 `Rc<RefCell<...>>` 而非裸 `HashMap`：
    // - dispatch 写端（`complete -C ...` 命令）需要 `&mut HashMap`，沿用 `run_complete` 现签名；
    // - 读端在 `ShellHelper`（TAB 补全路径）内部，需要在 `&self` 方法里查 registry。
    // 两端走同一份 Rc 克隆，单线程 REPL 串行节奏天然不并发借用。
    let completions: Rc<RefCell<HashMap<String, String>>> = Rc::new(RefCell::new(HashMap::new()));

    editor.set_helper(Some(ShellHelper::new(completions.clone())));

    // 后台任务编号计数器：跨 REPL 循环存活，从 1 起递增。
    // 仅在 `run_external` 的后台分支 spawn 成功后 `+= 1`；前台分支与
    // spawn 失败时保持不变（与 bash 仅在成功后台启动后分配 job 编号一致）。
    // 本阶段单线程串行 REPL 无需 `Rc<RefCell<...>>`，`&mut u32` 直接传参即可。
    let mut next_job_id: u32 = 1;

    // 后台作业表：跨 REPL 循环存活，记录已 spawn 但尚未回收的 Job。
    // 用 `Rc<RefCell<Vec<Job>>>` 复刻 `completions` 注册表风格：
    // - 写端 `run_external`（后台 spawn 成功后 push）；
    // - 读端 `run_jobs`（遍历列出）；
    // - 为未来 SIGCHLD 异步回收（reaper 线程或 signalfd）预留共享路径。
    // 单线程 REPL 串行节奏天然不并发借用。
    let jobs_table: Rc<RefCell<Vec<Job>>> = Rc::new(RefCell::new(Vec::new()));

    loop {
        // 2. 读取一行输入（rustyline 内部处理提示符绘制、回显、TAB 补全、行编辑）。
        let line = match editor.readline("$ ") {
            Ok(s) => s,
            Err(ReadlineError::Eof) => break,           // Ctrl-D：等同原 Ok(0) 分支
            Err(ReadlineError::Interrupted) => continue, // Ctrl-C：丢弃当前行，进入下一轮
            Err(e) => {
                eprintln!("read error: {}", e);
                break;
            }
        };

        // 3. 去除可能残留的行尾换行（rustyline 通常已剥除，这里防御性保留）。
        let line = line.trim_end_matches(['\n', '\r']);

        // 4. 空行跳过，继续显示下一个提示符
        if line.is_empty() {
            continue;
        }

        // 5. 词法 + 结构化解析：支持引号、转义与 `>` / `1>` 重定向
        //    解析失败（未闭合引号、孤立反斜杠、`>` 后无目标）打印错误后继续 REPL，不中断进程
        let parsed = match parser::parse(line) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{}", e);
                continue;
            }
        };

        // 仅含空白 / 只有重定向无命令时，argv 为空：跳过下一轮
        if parsed.argv.is_empty() {
            continue;
        }

        // 拆分命令与参数；cmd 用 &str 便于 match，args 保留 &[String]
        let cmd = parsed.argv[0].as_str();
        let args: &[String] = &parsed.argv[1..];

        // 极端情况：纯 `''` 这类输入会得到空字符串 cmd，按 not found 处理
        if cmd.is_empty() {
            // 此处不走 err_sink（line 实际不可能是非空但 cmd 为空的常规命令；
            // 即便如此，按 bash 行为「command not found」走 stderr，但因为还没
            // 准备 sink 也无重定向语义关键路径，直接 eprintln! 即可）。
            eprintln!("{}: command not found", line);
            continue;
        }

        // 6. 准备 sink / err_sink：根据 stdout_redirect / stderr_redirect 与对应的
        //    append 标志决定打开方式（截断 vs 追加）。打开失败按错误打印到 stderr 后
        //    跳过本轮，REPL 不中断。
        let mut sink: Box<dyn Write> =
            match open_sink(parsed.stdout_redirect.as_deref(), parsed.stdout_append) {
                Ok(s) => s,
                Err(e) => {
                    let target = parsed.stdout_redirect.as_deref().unwrap_or("?");
                    eprintln!("{}: {}: {}", cmd, target, e);
                    continue;
                }
            };
        let mut err_sink: Box<dyn Write> =
            match open_err_sink(parsed.stderr_redirect.as_deref(), parsed.stderr_append) {
                Ok(s) => s,
                Err(e) => {
                    let target = parsed.stderr_redirect.as_deref().unwrap_or("?");
                    eprintln!("{}: {}: {}", cmd, target, e);
                    continue;
                }
            };

        // 7. 内建命令分发；外部命令交由 exec::run_external 处理（接管 sink / err_sink 所有权）
        match cmd {
            "exit" => {
                // 可选退出码，解析失败回退为 0
                let code = args.first().and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
                std::process::exit(code);
            }
            "echo" => {
                if let Err(e) = run_echo(&mut *sink, args) {
                    eprintln!("shell: write error: {}", e);
                }
            }
            "pwd" => {
                if let Err(e) = run_pwd(&mut *sink, &mut *err_sink) {
                    eprintln!("shell: write error: {}", e);
                }
            }
            "cd" => {
                run_cd(&mut *err_sink, args);
            }
            "type" => {
                if let Err(e) = run_type(&mut *sink, &mut *err_sink, args) {
                    eprintln!("shell: write error: {}", e);
                }
            }
            "complete" => {
                if let Err(e) = run_complete(
                    &mut *sink,
                    &mut *err_sink,
                    args,
                    &mut *completions.borrow_mut(),
                ) {
                    eprintln!("shell: write error: {}", e);
                }
            }
            "jobs" => {
                // 本阶段：列出唯一的运行中后台作业。借出 `&[Job]` 切片传给 run_jobs；
                // borrow 作用域到本 match arm 结束即释放，不与 dispatch 后续路径冲突。
                let view = jobs_table.borrow();
                if let Err(e) = run_jobs(&mut *sink, &mut *err_sink, args, &view) {
                    eprintln!("shell: write error: {}", e);
                }
            }
            _ => {
                run_external(cmd, line, args, &parsed, sink, err_sink, &mut next_job_id, &jobs_table);
            }
        }
    }
}

