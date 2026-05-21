//! `codecrafters-shell` 主入口：REPL 主循环 + dispatch 调度。
//!
//! 详细架构见 `docs/ARCHITECTURE.md`；关键设计决策见 `docs/DESIGN_DECISIONS.md`。
//!
//! 本文件刻意保持「极薄」：
//! - 共享状态构造（completions / jobs_table / shell_vars / last_appended_len）
//! - prompt 前后台作业 reaping
//! - readline → parse_pipeline → dispatch match → builtin / exec / history_io
//!
//! 实现细节完全下沉到各子模块（builtins/ / completion/ / exec/ / history_io）。

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{self, Write};
use std::rc::Rc;

mod builtins;
mod completion;
mod exec;
mod history_io;
mod parser;
mod redirect;

use builtins::{
    advance_job_status, render_done_jobs, retain_running_jobs, run_cd, run_complete, run_declare,
    run_echo, run_history, run_jobs, run_pwd, run_type, Job,
};
use completion::ShellHelper;
use exec::{run_external, run_pipeline};
use history_io::{
    collect_history_entries, load_history_from_envfile, run_history_append, run_history_read,
    run_history_write, save_history_to_envfile, ShellEditor,
};
use redirect::{open_err_sink, open_sink};
use rustyline::error::ReadlineError;
use rustyline::{CompletionType, Config, Editor};

fn main() {
    // 1. 初始化 rustyline Editor + 自定义 Helper（TAB 补全）。
    //    `CompletionType::List`：单候选时直接用 `replacement` 替换 line buffer，与
    //    我们 Helper 已经手算好 `replacement`（含尾空格 / 尾 `/`）的语义对齐。
    //    详见 docs/DESIGN_DECISIONS.md#completion-state-machine。
    let config = Config::builder()
        .completion_type(CompletionType::List)
        .build();
    let mut editor: ShellEditor = match Editor::with_config(config) {
        Ok(ed) => ed,
        Err(e) => {
            eprintln!("shell: failed to init readline: {}", e);
            std::process::exit(1);
        }
    };

    // 跨 REPL 存活的共享状态。所有 `Rc<RefCell<_>>` 用法详见
    // docs/DESIGN_DECISIONS.md#rc-refcell-vs-arc-mutex。
    let completions: Rc<RefCell<HashMap<String, String>>> = Rc::new(RefCell::new(HashMap::new()));
    editor.set_helper(Some(ShellHelper::new(completions.clone())));

    // 后台作业表：`run_external` / `run_pipeline` 写端、`run_jobs` / 自动 reap 读端。
    let jobs_table: Rc<RefCell<Vec<Job>>> = Rc::new(RefCell::new(Vec::new()));

    // shell 变量存储：`declare` 写端、parser `$VAR` 展开读端。
    let shell_vars: Rc<RefCell<HashMap<String, String>>> = Rc::new(RefCell::new(HashMap::new()));

    // `history -a` 增量追加游标。详见 docs/DESIGN_DECISIONS.md。
    let mut last_appended_len: usize = 0;

    // 启动加载：若 $HISTFILE 设置且文件可读，按行加载历史条目。
    load_history_from_envfile(&mut editor);

    loop {
        // 每轮 prompt 前对后台作业做三步原子操作：状态推进 → 渲染 Done → 移除 Done。
        // 详见 docs/DESIGN_DECISIONS.md#background-reaping。
        {
            let mut tbl = jobs_table.borrow_mut();
            advance_job_status(&mut tbl);
            let mut out = io::stdout().lock();
            let _ = render_done_jobs(&mut out, &tbl);
            let _ = out.flush();
            retain_running_jobs(&mut tbl);
        }

        // 读取一行输入
        let line = match editor.readline("$ ") {
            Ok(s) => s,
            Err(ReadlineError::Eof) => break,             // Ctrl-D
            Err(ReadlineError::Interrupted) => continue,  // Ctrl-C
            Err(e) => {
                eprintln!("read error: {}", e);
                break;
            }
        };

        let line = line.trim_end_matches(['\n', '\r']);
        if line.is_empty() {
            continue;
        }

        // 记入 rustyline 内部 history（rustyline 14 默认 auto_add_history=false）
        let _ = editor.add_history_entry(line);

        // 词法 + 结构化解析
        let pipeline = match parser::parse_pipeline(line, &shell_vars.borrow()) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{}", e);
                continue;
            }
        };

        // 多段 pipeline 分支
        if pipeline.stages.len() > 1 {
            run_pipeline(&pipeline, &jobs_table);
            continue;
        }

        // 单段路径：取唯一段并回填 background 标志
        let mut parsed = pipeline.stages.into_iter().next().expect("stages.len() >= 1");
        parsed.background = pipeline.background;

        if parsed.argv.is_empty() {
            continue;
        }

        let cmd = parsed.argv[0].as_str();
        let args: &[String] = &parsed.argv[1..];

        if cmd.is_empty() {
            eprintln!("{}: command not found", line);
            continue;
        }

        // 准备 sink / err_sink
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

        // 内建分发；外部命令走 `_ => run_external`
        match cmd {
            "exit" => {
                let code = args.first().and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
                // Rust 不支持 atexit，process::exit 不运行 Drop——必须显式保存
                save_history_to_envfile(&editor);
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
                    &mut completions.borrow_mut(),
                ) {
                    eprintln!("shell: write error: {}", e);
                }
            }
            "jobs" => {
                // `run_jobs` 入口再做一次 reap（防御性兜底）
                let mut view = jobs_table.borrow_mut();
                if let Err(e) = run_jobs(&mut *sink, &mut *err_sink, args, &mut view) {
                    eprintln!("shell: write error: {}", e);
                }
            }
            "history" => {
                // -r / -w / -a 三个文件 IO 子命令优先嗅探（写端需要 &mut Editor）
                if args.first().map(|s| s.as_str()) == Some("-r") {
                    if let Some(path) = args.get(1) {
                        run_history_read(&mut editor, path);
                    }
                    continue;
                }
                if args.first().map(|s| s.as_str()) == Some("-w") {
                    if let Some(path) = args.get(1) {
                        run_history_write(&editor, path);
                    }
                    continue;
                }
                if args.first().map(|s| s.as_str()) == Some("-a") {
                    if let Some(path) = args.get(1) {
                        run_history_append(&editor, path, &mut last_appended_len);
                    }
                    continue;
                }

                // 默认路径：渲染历史列表（带可选 N 参数）
                let entries = collect_history_entries(&editor);
                if let Err(e) = run_history(&mut *sink, &mut *err_sink, args, &entries) {
                    eprintln!("shell: write error: {}", e);
                }
            }
            "declare" => {
                if let Err(e) = run_declare(
                    &mut *sink,
                    &mut *err_sink,
                    args,
                    &mut shell_vars.borrow_mut(),
                ) {
                    eprintln!("shell: write error: {}", e);
                }
            }
            _ => {
                run_external(cmd, line, args, &parsed, sink, err_sink, &jobs_table);
            }
        }
    }

    // Ctrl-D / 读错误 / 其他 break 退出路径
    save_history_to_envfile(&editor);
}
