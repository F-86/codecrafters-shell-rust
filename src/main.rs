use std::io::{self, BufRead, Write};

/// shell 内建命令清单，作为 `type` 命令查询的单一数据源。
/// 后续阶段新增内建（如 pwd/cd）时只需在此处追加。
const BUILTINS: &[&str] = &["echo", "exit", "type"];

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

        // 5. 拆分命令和参数
        let mut parts = line.split_whitespace();
        let cmd = match parts.next() {
            Some(c) => c,
            None => continue,
        };

        // 6. 内建命令分发
        match cmd {
            "exit" => {
                // 可选退出码，解析失败回退为 0
                let code = parts.next().and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
                std::process::exit(code);
            }
            "echo" => {
                // 将剩余参数用单空格连接后打印
                let output = parts.collect::<Vec<&str>>().join(" ");
                println!("{}", output);
            }
            "type" => {
                // 取首个参数作为查询目标；无参时不输出，进入下一轮 REPL
                if let Some(target) = parts.next() {
                    if BUILTINS.contains(&target) {
                        println!("{} is a shell builtin", target);
                    } else {
                        println!("{}: not found", target);
                    }
                }
            }
            _ => {
                println!("{}: command not found", line);
            }
        }
    }
}
