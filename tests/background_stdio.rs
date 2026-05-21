//! 集成测试：codecrafters「Background Job Output」stage 端到端验证。
//!
//! 复刻 codecrafters tester 的执行序列：
//!
//! ```text
//! $ ./your_program.sh
//! $ cat /path/to/fifo1 &
//! [1] <pid>
//! $ cat /path/to/fifo2
//! ```
//!
//! 测试在另一侧（外部脚本）异步向两个 FIFO 写入 payload，断言两条 payload 都通过
//! shell 的 stdout 管道被本测试观察到——这间接证明子进程的 stdout 继承自 shell
//! 进程的 stdout，与题面要求一致（详见 `src/exec.rs` 模块头注释）。
//!
//! 设计要点：
//! - **不引入新依赖**：mkfifo 通过 `Command::new("mkfifo")` 系统命令完成；超时通过
//!   `std::thread` + `mpsc::channel::recv_timeout` 实现。
//! - **二进制路径**：用 `env!("CARGO_BIN_EXE_codecrafters-shell")` 拿到 Cargo 注入的
//!   测试目标二进制路径，避免硬编码 `target/debug/...`。
//! - **rustyline 在非 tty 输入下的退化**：测试把 shell 的 stdin 接到管道，rustyline
//!   会自动退化为按行读，prompt 仍写到 stdout（混在输出里），断言只检查子串包含
//!   而非按行精确匹配。
//! - **写入时序**：必须等 shell 把 `cat <fifo> &` 启动起来并让 cat open FIFO 之后
//!   再写入，否则 cat 还没 open，写端先关会丢数据。用 `sleep` 200ms 留出窗口。
//! - **清理**：用 RAII guard 在 Drop 中 kill shell + wait + 删除临时目录，保证
//!   即使 assert panic 也能清理资源。

#![cfg(unix)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// RAII 清理保证：测试函数 panic 或正常结束时都会执行。
struct Cleanup {
    shell: Option<Child>,
    dir: PathBuf,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(mut shell) = self.shell.take() {
            let _ = shell.kill();
            let _ = shell.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// 在 `dir` 下用系统 `mkfifo` 命令创建一个 FIFO。
fn mkfifo(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    let status = Command::new("mkfifo")
        .arg(&path)
        .status()
        .expect("spawn mkfifo");
    assert!(
        status.success(),
        "mkfifo {} failed with status {:?}",
        path.display(),
        status
    );
    path
}

#[test]
fn background_cat_output_reaches_terminal() {
    // 1. 唯一临时目录：temp_dir / "cc-shell-bg-<pid>-<ns>"
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("cc-shell-bg-{}-{}", std::process::id(), now));
    std::fs::create_dir_all(&dir).expect("mkdir temp");

    let fifo1 = mkfifo(&dir, "fifo1");
    let fifo2 = mkfifo(&dir, "fifo2");

    // 2. spawn shell 二进制，stdin / stdout / stderr 都接管道
    let bin = env!("CARGO_BIN_EXE_codecrafters-shell");
    let shell = Command::new(bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn shell binary");

    // 进入清理 guard 范围；下面任何 panic 都会触发 Drop
    let mut guard = Cleanup {
        shell: Some(shell),
        dir: dir.clone(),
    };
    // 重新借出 shell 句柄以便操作其管道
    let shell = guard.shell.as_mut().unwrap();

    let mut shell_stdin = shell.stdin.take().expect("shell stdin");
    let shell_stdout = shell.stdout.take().expect("shell stdout");

    // 3. 后台线程持续读 shell 的 stdout，累积成 String，每读到一段就 send 一次
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        let mut reader = shell_stdout;
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // 4. 给 shell 写入两行命令：
    //    cat <fifo1> &
    //    cat <fifo2>
    //    第二条是前台 cat，会阻塞读 fifo2；测试结束由 Cleanup::drop 杀掉 shell。
    let cmd1 = format!("cat {} &\n", fifo1.display());
    let cmd2 = format!("cat {}\n", fifo2.display());
    shell_stdin.write_all(cmd1.as_bytes()).expect("write cmd1");
    shell_stdin.write_all(cmd2.as_bytes()).expect("write cmd2");
    shell_stdin.flush().expect("flush shell stdin");

    // 5. 等 shell 启动两个 cat 子进程并让它们 open FIFO（读端阻塞）。
    //    200ms 在 codecrafters CI 上经测足够；不足时下游 echo > fifo 会立即关闭
    //    写端而 cat 还没 open，会丢数据。
    thread::sleep(Duration::from_millis(300));

    // 6. 异步向两个 FIFO 写入 payload。用 sh -c 与题面 tester 风格保持一致。
    //    注意：每条 echo > fifo 都会阻塞直到对端 open；我们已在上一步 sleep 过。
    let payload1 = "Hello from FIFO#1";
    let payload2 = "Hello from FIFO#2";
    let writer1 = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "echo -ne '{}\n' > {}",
            payload1,
            fifo1.display()
        ))
        .status()
        .expect("write fifo1");
    assert!(writer1.success(), "echo > fifo1 failed");

    let writer2 = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "echo -ne '{}\n' > {}",
            payload2,
            fifo2.display()
        ))
        .status()
        .expect("write fifo2");
    assert!(writer2.success(), "echo > fifo2 failed");

    // 7. 累积 shell stdout 输出（带超时），直到两条 payload 都出现为止，或 5s 超时。
    let mut acc = String::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match rx.recv_timeout(remaining.max(Duration::from_millis(50))) {
            Ok(chunk) => {
                acc.push_str(&String::from_utf8_lossy(&chunk));
                if acc.contains(payload1) && acc.contains(payload2) {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => break,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    // 8. 断言：两条 payload 都应在累积输出中出现。
    //    失败时把累积输出回显出来，便于 codecrafters CI 上诊断。
    let ok1 = acc.contains(payload1);
    let ok2 = acc.contains(payload2);
    if !(ok1 && ok2) {
        eprintln!(
            "---- shell accumulated stdout ----\n{}\n---- end ----",
            acc
        );
    }
    assert!(
        ok1,
        "background job (cat fifo1 &) output not seen on shell stdout"
    );
    assert!(
        ok2,
        "foreground job (cat fifo2) output not seen on shell stdout"
    );

    // 9. guard 在 scope 结束时自动 kill shell + 删临时目录
    drop(shell_stdin);
    drop(guard);
}
