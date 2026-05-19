---
name: shell-external-exec
overview: 扩展 REPL 默认分支：非内建命令时通过 PATH 查找可执行文件并以原命令名作为 argv[0] 执行，继承 stdio 透传输出，未命中则保持 `command not found`。
todos:
  - id: impl-external-exec
    content: 改造 src/main.rs 默认分支：use CommandExt，find_in_path 命中后 Command::new(path).arg0(cmd).args(parts).status()，未命中保留 command not found
    status: completed
  - id: verify-external-exec
    content: 本地验证：ls /tmp 等命中 PATH 命令正常输出；自建脚本验证 argv[0] 为命令名而非完整路径；多参数透传；未知命令、echo/exit/type/空行/EOF 无回归
    status: completed
    dependencies:
      - impl-external-exec
---

## 产品概述

扩展 shell：当用户输入的命令不是 builtin 时，复用 `find_in_path` 在 PATH 中查找可执行文件，找到则以子进程方式执行该程序，并透传参数与 stdout/stderr；未找到则维持原有 `command not found` 行为。

## 核心功能

- 外部程序执行：命中 PATH 内可执行文件后，启动子进程并阻塞等待其结束
- argv[0] 显式为用户输入的命令名（而非命中后的完整路径），与 bash 行为一致
- 参数透传：命令行剩余 token 全部以独立参数传给子程序
- stdio 继承：子程序的标准输出/错误输出直接显示在 shell 中
- 未命中保持原有 `<line>: command not found` 错误输出
- 既有 builtin（echo/exit/type）、空行、EOF 行为完全保持不变

## 技术栈

- 语言：Rust（edition = "2024", rust-version = "1.95"）
- 仅使用标准库：`std::process::Command`、`std::os::unix::process::CommandExt`（提供 `arg0`）
- 不引入新依赖；继续单文件实现

## 实现思路

在 `src/main.rs` 既有 `match cmd` 的默认分支 `_ =>` 中插入"PATH 查找 + 子进程执行"逻辑：

1. 调用既有 `find_in_path(cmd)` 判断命令是否为 PATH 内可执行文件
2. 命中：用 `Command::new(<完整路径>)` 构造子进程，调用 `.arg0(cmd)` 强制把 argv[0] 设为用户输入的命令名（不是完整路径），再 `.args(parts)` 透传剩余参数，然后 `.status()` 阻塞执行
3. 未命中：保留原 `println!("{}: command not found", line)` 输出
4. `.status()` 返回 `Err` 时降级为 `not found` 行为，避免崩溃

stdio 默认继承父进程即可（`Command` 默认行为），子程序直接向 shell 的 stdout/stderr 写入，符合测试器期望。

## 关键技术决策

- **`arg0` 而非 `Command::new(cmd)` 让系统隐式解析**：题面明确要求 argv[0] 是用户输入的命令名，且测试器会把可执行文件放在随机目录；既然 `find_in_path` 已得到完整路径，用完整路径 `exec` + 显式 `arg0` 覆写命令名是最可靠的组合。
- **复用 `find_in_path`**：与 `type` 分支共用查找语义，保证"`type X` 报告的位置就是真正会执行的文件"。
- **阻塞 `.status()` 而非 `.spawn() + wait()`**：API 更简洁，语义等价；本阶段不需要保留 `Child` 句柄做信号处理。
- **不改动既有 `command not found` 字面量**：保持上阶段的 `<line>: command not found`（含完整行），避免引入回归——题目本阶段未要求改变此输出格式。
- **失败降级保守**：`spawn`/`status` 失败（罕见情况，如文件被并发删除）走 `command not found` 分支，保证 REPL 不退出。

## 实施注意事项

- **必须 `use std::os::unix::process::CommandExt;`**，否则 `.arg0` 不可见。
- **子进程错误输出由其自身控制**：父进程仅提供 stdio 继承，不要主动捕获/拦截，否则测试器看不到 "Program was passed N args…"。
- **`parts` 是迭代器，已被 `parts.next()` 消费过 `cmd`**：当前 `_` 分支可直接 `.args(parts)` 透传剩余 token，零拷贝。
- **保持 blast radius 极小**：仅修改 `_ =>` 默认分支约 10 行；不动 `find_in_path`、`BUILTINS`、其他 builtin 分支与 REPL 控制流。

## 架构设计

```
loop:
  read line / trim / split_whitespace -> (cmd, parts*)
  match cmd:
    "exit" / "echo" / "type" -> 既有 builtin 分支（不变）
    _ =>
      if let Some(path) = find_in_path(cmd):                # NEW
          Command::new(path)
            .arg0(cmd)
            .args(parts)
            .status()
            .map(|_| ())                                    # 阻塞等待
            .unwrap_or_else(|_| println!("{line}: command not found"))
      else:
          println!("{line}: command not found")             # 原有行为
```

## 目录结构

```
codecrafters-shell-rust/
└── src/
    └── main.rs   # [MODIFY] 顶部 use 增补 std::os::unix::process::CommandExt;
                  #          改造 match cmd 的 "_" 默认分支：
                  #            1) find_in_path(cmd) 命中 -> Command::new(path).arg0(cmd).args(parts).status()
                  #            2) status 返回 Err 或未命中 -> 保留 "<line>: command not found"
                  #          其他分支与 REPL 流程保持不变；find_in_path 不修改、BUILTINS 不修改。
```