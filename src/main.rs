use std::io::{self, BufRead, Write};

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

        // 5. 本阶段所有命令都视为未找到
        println!("{}: command not found", line);
    }
}
