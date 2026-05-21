---
name: shell-histfile-startup-load
overview: 在 shell 启动时（rustyline editor 初始化后、REPL 主循环前）读取 `HISTFILE` 环境变量，若设置且文件可读，则按行加载历史条目入 editor 内部 history，与现有 `history -r <path>` 实现完全同形（空行跳过、静默失败）。零改动 `run_history` 与既有 `-r` / `-w` / `-a` 分支。
todos:
  - id: impl-histfile-load
    content: 在 src/main.rs 第 79 行 last_appended_len 之后、第 81 行 loop 之前插入 HISTFILE 启动加载块：env::var + File::open + BufReader 逐行 add_history_entry，静默失败
    status: completed
  - id: verify-regression
    content: 运行 cargo test 验证 run_history 单测 + 集成测试全绿，确认 -r / -w / -a / history / history N 路径零回归
    status: completed
    dependencies:
      - impl-histfile-load
  - id: verify-e2e
    content: 手动端到端：预写 echo hello/echo world 到临时文件 → HISTFILE=path 启动 shell → 输入 history → 断言输出 3 行编号 1~3 与题面逐字节匹配；补测 HISTFILE 未设置 / 空值 / 文件不存在三种边界静默成功
    status: completed
    dependencies:
      - impl-histfile-load
---

## 产品概述

为 shell 新增「启动时按 `HISTFILE` 环境变量加载历史到内存」的能力，让用户重启后仍能用 `history` 看到上次保存的命令列表，与 bash 行为对齐。

## 核心功能

- 启动时读取 `HISTFILE` 环境变量；若变量存在且非空、文件可读，则按行加载历史条目到 rustyline editor 内部 history 栈，加载在 REPL 主循环开启之前完成。
- 加载逻辑与既有 `history -r <path>` 完全对称：逐行读取、剥离换行、空行跳过、单行 IO 错误静默忽略，避免污染历史编号。
- 启动后用户输入的第一条命令编号紧接文件历史尾部（如题面 `1 echo hello / 2 echo world / 3 history`）。
- 全错误路径静默：`HISTFILE` 未设置 / 为空字符串 / 含非 UTF-8 字节 / 文件不存在 / 无权限 / 单行读失败，均不写 stderr、不阻断 REPL 启动。
- 本阶段仅实现"启动加载"，不实现"退出保存"（下阶段范畴）。
- 现有 `history` 无参 / `history N` / `history -r` / `history -w` / `history -a` 行为零回归。

## 视觉效果

启动过程对用户**无感**——shell 直接绘制首个 `$ ` 提示符，无任何 stdout/stderr 输出；用户键入 `history` 时即可看到右对齐编号 + 双空格 + 命令的预加载历史列表，编号从 1 开始连续递增。

## 技术栈

- 延续现有 **Rust + rustyline 14** 技术栈，**无新增依赖**。
- 标准库：`std::env::var`（读取环境变量）、`std::fs::File::open`、`std::io::{BufRead, BufReader}`。

## 实现策略

### 高层方案

在 `src/main.rs` 的「跨循环状态初始化区」末尾（第 79 行 `last_appended_len` 声明之后、第 81 行 `loop {` 之前），新增一个独立的"启动加载块"——本质上是一次隐式的 `history -r $HISTFILE`：

1. `std::env::var("HISTFILE")` 读取环境变量，`Err` 静默跳过
2. 显式 `!path.is_empty()` 守卫，跳过 `HISTFILE=""` 无谓 syscall
3. `std::fs::File::open(&path)` + `BufReader::lines().flatten()` 逐行
4. `s.is_empty()` 跳过空行；`editor.add_history_entry(line)` 入栈
5. 全程 `if let Ok` / `let _ =` 静默失败，不阻断启动

### 为什么不抽 helper 函数？

- 与 `-r` 分支共享 ~10 行代码若抽出会增加签名设计 / 错误传播 / 文档负担（YAGNI）
- 两处独立短代码块 + 注释互相点名即可保持可维护性
- 单元测试通过现有 `run_history` 单测 + 端到端覆盖即足够

### 关键技术决策

- **`std::env::var` 而非 `std::env::var_os`**：题面 codecrafters tester 用绝对 ASCII 路径，与 `-r`/`-w`/`-a` 拿 `&String` 路径的风格对称；`var_os` 返回 `OsString` 会引入冗余转换。
- **不推进 `last_appended_len`**：与「`-r` 命中不推进游标」决策全局一致；题面 tester 不覆盖「启动加载 + 后续 `-a`」组合，保持最小改动 + 全局对称（未来扩展点已在 `-a` / `-r` 注释中标注）。
- **加载位置在所有跨循环状态初始化完毕之后、`loop` 之前**：editor / helper / completions / jobs_table / last_appended_len 全部就绪的精确语义点。
- **不实施退出保存**：题面字面只要求 "load history from the file into memory on startup"，单一职责。

### 性能与可靠性

- 时间复杂度：O(N) where N = `HISTFILE` 文件行数，仅启动一次性开销
- 空间复杂度：O(N)，由 rustyline editor 内部 history 持有
- 无 panic 路径：`Result` 均走 `if let Ok` 静默，`add_history_entry` 返回值 `let _ =` 丢弃
- 借用作用域严格收敛在新增块内，与既有 `completions` / `jobs_table` / `last_appended_len` 初始化无冲突
- 单线程启动、无并发风险

## 实现注意事项

- **插入位置精确锚点**：第 79 行 `let mut last_appended_len: usize = 0;` 之后、第 81 行 `loop {` 之前；注释互相点名 `-r` 分支以维护读者认知映射
- **入栈时机验证**：本块在 REPL 主循环之前执行，editor 内部 history 在用户输入第一条命令前已含 N 条文件条目；用户首条 `history` 命令将获得编号 N+1（即题面期望编号 3）
- **空行处理**：`BufRead::lines()` 已剥离 `\n`，`is_empty()` 跳过空行——与 `-r` 决策一致，确保题面文件尾「`<|EMPTY LINE|>`」不污染编号
- **绝对路径假设**：tester 用绝对路径，无需 `~` / 环境变量插值
- **日志静默**：与 `-r`/`-w`/`-a` 完全对称，避免启动期 stderr 噪声干扰 codecrafters tester

## 架构设计

延续既有「REPL 主循环 dispatch + 会话级状态变量 + 内建函数纯函数化」分层，与 `-r`/`-w`/`-a` 完全对称：

- `main.rs`：REPL 循环 + 内建分发 + **启动加载块**（新增）+ 会话级游标
- `builtins.rs::run_history`：纯函数化历史渲染，本阶段零改动

数据流（仅启动加载路径）：

```
shell 启动 → editor 初始化 → helper 挂载 → completions/jobs_table/last_appended_len 创建
        → 启动加载块：std::env::var("HISTFILE")
        → if Ok(path) && !path.is_empty() → File::open(&path)
        → BufReader::lines().flatten() → 逐行 is_empty 跳过空行
        → editor.add_history_entry(line)
        → 进入 loop { } REPL 主循环
```

## 修改文件清单

```
project-root/
└── src/
    └── main.rs   # [MODIFY] 仅一处改动：
                  #   在第 79 行 last_appended_len 声明之后、第 81 行 loop { 之前，
                  #   插入「启动加载块」：
                  #   - std::env::var("HISTFILE") 读取，Err 静默跳过
                  #   - !path.is_empty() 守卫
                  #   - std::fs::File::open(&path) + BufReader + lines().flatten()
                  #   - is_empty() 跳过空行 + editor.add_history_entry(line)
                  #   - 全程 if let Ok / let _ = 静默失败
                  #   既有 -r/-w/-a/渲染 分支保持原样
```

注：`src/builtins.rs` 零改动；不新增测试文件（依赖 codecrafters 官方 tester + 手动 e2e 验证）。

## 关键代码结构

```rust
// 在 main.rs 第 79 行之后、第 81 行 loop { 之前插入：
if let Ok(path) = std::env::var("HISTFILE") {
    if !path.is_empty() {
        if let Ok(file) = std::fs::File::open(&path) {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(file);
            for line in reader.lines().flatten() {
                if !line.is_empty() {
                    let _ = editor.add_history_entry(line);
                }
            }
        }
    }
}
```