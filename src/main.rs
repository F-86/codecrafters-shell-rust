use std::fs::File;
use std::io::{self, BufRead, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

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

/// `echo` 内建：把所有参数用单空格连接后写入 sink。
/// 引号内空格已在 token 内部保留，此处单空格 join 是正确行为。
fn run_echo(sink: &mut dyn Write, args: &[String]) -> io::Result<()> {
    writeln!(sink, "{}", args.join(" "))
}

/// `pwd` 内建：打印当前工作目录的绝对路径。
/// `current_dir()` 内部调用 `getcwd(2)`，由 OS 保证返回绝对路径。
/// 目录被删除 / 无权限等异常场景：错误信息固定写到 stderr（与 bash 一致），不进入 sink。
fn run_pwd(sink: &mut dyn Write) -> io::Result<()> {
    match std::env::current_dir() {
        Ok(path) => writeln!(sink, "{}", path.display()),
        Err(e) => {
            eprintln!("pwd: {}", e);
            Ok(())
        }
    }
}

/// `type` 内建：查询目标名是 builtin、PATH 中可执行文件还是 not found。
/// 三种结果都属于 stdout 输出，统一写入 sink。无参数时静默（与既有行为一致）。
fn run_type(sink: &mut dyn Write, args: &[String]) -> io::Result<()> {
    let Some(target) = args.first() else {
        return Ok(());
    };
    let target = target.as_str();
    if BUILTINS.contains(&target) {
        writeln!(sink, "{} is a shell builtin", target)
    } else if let Some(path) = find_in_path(target) {
        writeln!(sink, "{} is {}", target, path.display())
    } else {
        writeln!(sink, "{}: not found", target)
    }
}

/// 根据 `stdout_redirect` 准备 sink：
/// - `None` → 锁定的 stdout 句柄；
/// - `Some(path)` → `File::create(path)`（不存在则创建、存在则截断覆盖）。
///
/// 返回 `Box<dyn Write>` 让调用方对两种来源无感；打开失败则把错误反馈给调用方，
/// 由 REPL 主循环统一打印到 stderr 并跳过本轮命令执行（保证 REPL 不中断）。
fn open_sink(stdout_redirect: Option<&str>) -> io::Result<Box<dyn Write>> {
    match stdout_redirect {
        Some(path) => Ok(Box::new(File::create(path)?)),
        // 注：返回 `io::Stdout` 而非 `StdoutLock<'_>` 以规避借用生命周期约束。
        // 多线程场景下 `Stdout` 内部已有锁，对 REPL 单线程使用场景表现等价。
        None => Ok(Box::new(io::stdout())),
    }
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
            println!("{}: command not found", line);
            continue;
        }

        // 6. 准备 sink：根据 stdout_redirect 决定写到 stdout 还是文件。
        //    打开失败按错误打印到 stderr 后跳过本轮，REPL 不中断。
        let mut sink: Box<dyn Write> =
            match open_sink(parsed.stdout_redirect.as_deref()) {
                Ok(s) => s,
                Err(e) => {
                    // path 此处一定存在（None 不会失败）
                    let target = parsed.stdout_redirect.as_deref().unwrap_or("?");
                    eprintln!("{}: {}: {}", cmd, target, e);
                    continue;
                }
            };

        // 7. 内建命令分发
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
                if let Err(e) = run_pwd(&mut *sink) {
                    eprintln!("shell: write error: {}", e);
                }
            }
            "cd" => {
                // 取首个参数作为目标路径；支持绝对路径、相对路径（./、../、子目录名）与 ~
                // 相对路径由内核基于当前进程 cwd 解析，无需在此做字符串展开
                // 无参数场景本阶段不覆盖，静默跳过
                // 注：cd 不产 stdout 输出，故不接 sink；错误信息按既有约定打到 stdout
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
                if let Err(e) = run_type(&mut *sink, args) {
                    eprintln!("shell: write error: {}", e);
                }
            }
            _ => {
                // 非内建：复用 type 的 PATH 查找，命中即作为外部程序执行
                if let Some(path) = find_in_path(cmd) {
                    // 关键：在 spawn 子进程之前**释放** sink 持有的文件 fd 拥有权——
                    // 重定向时把 File 直接转移给 Command::stdout，由子进程 inherit；
                    // 无重定向时只需让父进程继承自己的 stdout（默认即 inherit）。
                    //
                    // sink 走的是 Box<dyn Write> 抽象，运行时无法把它「拆回」具体类型，
                    // 故对外部命令重新从 parsed.stdout_redirect 物化一次 stdio：
                    let stdio = match parsed.stdout_redirect.as_deref() {
                        Some(path) => match File::create(path) {
                            Ok(f) => Stdio::from(f),
                            Err(e) => {
                                eprintln!("{}: {}: {}", cmd, path, e);
                                continue;
                            }
                        },
                        None => Stdio::inherit(),
                    };
                    // 提前丢弃父进程内 sink 对同一文件的写句柄，避免与子进程 stdout
                    // 的截断/写入产生竞态（虽然内核 fd 独立，但语义上更干净）。
                    drop(sink);

                    // argv[0] 必须是用户输入的命令名而非完整路径（与 bash 行为一致）
                    // stderr 默认继承父进程（即终端），与 spec「错误不重定向」严格对齐
                    let status = Command::new(&path)
                        .arg0(cmd)
                        .args(args)
                        .stdout(stdio)
                        .status();
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
