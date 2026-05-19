use std::io::{self, BufRead, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

mod parser;

/// shell 内建命令清单，作为 `type` 命令查询的单一数据源。
/// 后续阶段新增内建（如 pwd/cd）时只需在此处追加。
const BUILTINS: &[&str] = &["echo", "exit", "type", "pwd", "cd"];

/// 按 PATH 顺序查找可执行文件。
/// 命中条件：文件存在、是普通文件、Unix 执行位（owner/group/other 任一）置位。
/// 目录不存在 / 无权限读取 / 非普通文件等场景静默跳过，与 bash 实际行为一致。
fn find_in_path(name: &str) -> Option<PathBuf> {
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

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut input = String::new();

    loop {
        // 1. 打印提示符并 flush，确保在阻塞读取前可见
        print!("$ ");
        stdout.flush().expect("failed to flush stdout");

        // 2. 读取一行输入
        input.clear();
        match stdin.lock().read_line(&mut input) {
            Ok(0) => break, // EOF (Ctrl-D)，正常退出
            Ok(_) => {}
            Err(e) => {
                eprintln!("read error: {}", e);
                break;
            }
        }

        // 3. 去除行尾换行符
        let line = input.trim_end_matches(['\n', '\r']);

        // 4. 空行跳过，继续显示下一个提示符
        if line.is_empty() {
            continue;
        }

        // 5. 词法分析：支持单引号的 token 切分
        //    解析失败（如未闭合引号）打印错误后继续 REPL，不中断进程
        let tokens = match parser::tokenize(line) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{}", e);
                continue;
            }
        };

        // 空 token 序列 = 仅空白行，继续下一轮
        if tokens.is_empty() {
            continue;
        }

        // 拆分命令与参数；cmd 用 &str 便于 match，args 保留 &[String]
        let cmd = tokens[0].as_str();
        let args: &[String] = &tokens[1..];

        // 极端情况：纯 `''` 这类输入会得到空字符串 cmd，按 not found 处理
        if cmd.is_empty() {
            println!("{}: command not found", line);
            continue;
        }

        // 6. 内建命令分发
        match cmd {
            "exit" => {
                // 可选退出码，解析失败回退为 0
                let code = args.first().and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
                std::process::exit(code);
            }
            "echo" => {
                // 将剩余参数用单空格连接后打印
                // 引号内空格已在 token 内部保留，此处单空格 join 是正确行为
                println!("{}", args.join(" "));
            }
            "pwd" => {
                // 打印当前工作目录的绝对路径；忽略多余参数（与 POSIX 宽松行为一致）
                // current_dir() 内部调用 getcwd(2)，由 OS 保证返回绝对路径
                match std::env::current_dir() {
                    Ok(path) => println!("{}", path.display()),
                    // 目录被删除 / 无权限等异常场景：报错但不中断 REPL
                    Err(e) => eprintln!("pwd: {}", e),
                }
            }
            "cd" => {
                // 取首个参数作为目标路径；支持绝对路径、相对路径（./、../、子目录名）与 ~
                // 相对路径由内核基于当前进程 cwd 解析，无需在此做字符串展开
                // 无参数场景本阶段不覆盖，静默跳过
                if let Some(target) = args.first() {
                    // ~ 展开：本阶段仅匹配精确 "~"，不处理 ~/subdir 或 ~user 形式
                    // HOME 缺失时按统一错误格式输出，避免 unwrap 导致 REPL 中断
                    let resolved = if target == "~" {
                        match std::env::var("HOME") {
                            Ok(home) => home,
                            Err(_) => {
                                println!("cd: {}: No such file or directory", target);
                                continue;
                            }
                        }
                    } else {
                        target.clone()
                    };
                    // 直接调用 chdir(2)：失败时 OS 保证 cwd 不变，无 TOCTOU 风险
                    // 不存在 / 非目录 / 无权限等失败统一打印同一错误信息以匹配测试期望
                    // 错误信息回显用户原始输入 target，不展开为 home 路径（与 bash 行为一致）
                    if std::env::set_current_dir(&resolved).is_err() {
                        println!("cd: {}: No such file or directory", target);
                    }
                }
            }
            "type" => {
                // 取首个参数作为查询目标；无参时不输出，进入下一轮 REPL
                if let Some(target) = args.first() {
                    let target = target.as_str();
                    if BUILTINS.contains(&target) {
                        println!("{} is a shell builtin", target);
                    } else if let Some(path) = find_in_path(target) {
                        println!("{} is {}", target, path.display());
                    } else {
                        println!("{}: not found", target);
                    }
                }
            }
            _ => {
                // 非内建：复用 type 的 PATH 查找，命中即作为外部程序执行
                if let Some(path) = find_in_path(cmd) {
                    // argv[0] 必须是用户输入的命令名而非完整路径（与 bash 行为一致）
                    // stdio 默认继承父进程，子程序输出会直接显示
                    let status = Command::new(&path).arg0(cmd).args(args).status();
                    if status.is_err() {
                        // spawn / wait 失败时降级，避免 REPL 中断
                        println!("{}: command not found", line);
                    }
                } else {
                    println!("{}: command not found", line);
                }
            }
        }
    }
}
