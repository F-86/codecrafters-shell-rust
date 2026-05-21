//! `complete` 内建：补全规格的注册 / 查询 / 删除（`-C` / `-p` / `-r`）。

use std::collections::HashMap;
use std::io::{self, Write};

/// `complete` 内建：补全规格的注册 / 查询 / 删除。
///
/// 4 路分派：
/// - `-C <path> <cmd>` → 注册 `cmd → path` 到 `registry`，无输出
/// - `-r <cmd>` → 从 `registry` 删除（未注册也静默成功）
/// - `-p <cmd>` 命中 → stdout `complete -C '<path>' <cmd>\n`
/// - `-p <cmd>` 未命中 → stderr `complete: <cmd>: no completion specification\n`
/// - 其它形态 → 静默 Ok
///
/// `registry` 是 `Rc<RefCell<HashMap>>` 跨命令存活：dispatch 写端 + TAB 补全（`completion::script`）读端共享。
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
}
