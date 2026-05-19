---
name: shell-type-builtin
overview: "新增 `type` 内建命令：判断目标命令是否为 shell builtin（echo/exit/type），是则打印 `<cmd> is a shell builtin`，否则打印 `<cmd>: not found`。"
todos:
  - id: impl-type-builtin
    content: "在 src/main.rs 顶层加 BUILTINS 常量，并在 match cmd 新增 \"type\" 分支：命中输出 \"X is a shell builtin\"，否则 \"X: not found\""
    status: completed
  - id: verify-type
    content: 本地验证：type echo/exit/type 输出 builtin 提示，type invalid_command 输出 not found，echo/exit/未知命令/空行/EOF 无回归
    status: completed
    dependencies:
      - impl-type-builtin
---

## 产品概述

在已有 Rust shell（`$ ` 提示符 + REPL + `echo`/`exit` 内建 + 未知命令报错）基础上，新增 `type` 内建命令，用于查询某个命令名的来源类型。本阶段仅处理两种情形：内建命令 与 未识别命令，可执行文件查找留待后续阶段。

## 核心功能

- 新增 `type` 内建命令：接收单个参数 `target`，依据 `target` 是否属于 shell 内建打印不同结果。
- 若 `target` 命中内建（`echo` / `exit` / `type` 自身），输出 `{target} is a shell builtin`
- 否则输出 `{target}: not found`
- `type` 本身视作 builtin，因此 `type type` 输出 `type is a shell builtin`
- 引入统一的 builtin 名称清单作为 `type` 查询的单一数据源，便于后续阶段（`pwd`/`cd` 等）扩展

## 验收要点

- `type echo` → `echo is a shell builtin`
- `type exit` → `exit is a shell builtin`
- `type type` → `type is a shell builtin`
- `type invalid_command` → `invalid_command: not found`
- 既有行为无回归：`echo` 正常输出、`exit` 正常终止、未知命令仍打印 `{cmd}: command not found`、空行跳过、EOF 自然退出

## 技术栈

- 语言：Rust（edition = "2024", rust-version = "1.95"）
- 仅使用标准库；不引入新依赖
- 单文件结构：在 `src/main.rs` 内联 REPL 与内建分发（builtin 数刚到 3，仍未达到拆模块阈值）

## 实现思路

1. 在 `src/main.rs` 文件顶层定义常量数组：

```rust
const BUILTINS: &[&str] = &["echo", "exit", "type"];
```

作为 builtin 命中判断的单一数据源；后续阶段新增内建时仅需在此数组追加即可。

2. 在现有 `match cmd { ... }` 中新增 `"type"` 分支：

- 通过 `parts.next()` 取首个参数 `target`
- `if BUILTINS.contains(&target)` → `println!("{} is a shell builtin", target)`
- 否则 → `println!("{}: not found", target)`
- 无参（`parts.next()` 为 `None`）时不输出，进入下一轮 REPL（题目未要求，保守处理）

3. 既有 `exit` / `echo` / fallback 分支与控制流（提示符、flush、读取、trim、空行、EOF、错误）保持原样，blast radius 仅 ~10 行。

## 关键技术决策

- **集中维护 builtin 清单**：用 `&[&str]` 常量数组而非在 type 分支内联硬编码字符串，避免后续每加一个内建都要改两处（dispatch + type 查询）。这是 SoC 与 DRY 的体现，也为后续阶段新增 `pwd`/`cd` 留好扩展点。
- **`contains(&target)` 的复杂度**：builtins 长度极小（个位数），线性查找成本可忽略；不必使用 HashSet，避免运行时初始化开销与依赖。
- **错误信息严格区分**：fallback 分支使用 `command not found`，`type` 分支使用 `not found`，两个字符串差异显著但语义相近，必须按题目要求严格区分，避免“便利复用”造成 grader 判失败。
- **不抽模块**：当前内建数 = 3，仍在合理内联范围；待新增 `pwd`/`cd`/外部命令执行后再考虑抽 `builtins.rs` + `Builtin` trait，避免过早设计。
- **不处理多参数**：题目场景 `type` 始终带单参；多参时仅消费第一个 token，剩余忽略，与 bash `type` 的常见用法一致（虽然 bash `type` 支持多参，但本阶段无测试覆盖，YAGNI）。

## 实施注意事项

- **输出通道**：所有 `type` 输出走 stdout，与 `echo`、`command not found` 一致；不要写到 stderr。
- **格式严格匹配**：
- 命中：`{target} is a shell builtin`（中间是单空格，无冒号）
- 未命中：`{target}: not found`（冒号后单空格）
- **保持向后兼容**：未知命令（非 type 分支）仍输出 `{line}: command not found`，与 fallback 共用同一行原始 line（保留多 token 输入的原貌）；type 分支只回显 target 单 token，不要混淆。
- **flush**：循环顶部已有 `stdout.flush()`，type 分支由 `println!` 行缓冲在下一次提示符前自然 flush，无需额外处理。
- **避免不必要重构**：不动既有 `parts` 拆分逻辑，type 分支直接复用迭代器消费首个参数即可。

## 架构设计

```
const BUILTINS = ["echo", "exit", "type"]

loop:
  print "$ " + flush
  read_line
    Ok(0) -> break (EOF)
    Ok(_) -> trim
              if empty: continue
              split_whitespace -> (cmd, rest)
              match cmd:
                "exit"  -> parse rest[0] as i32 (default 0) -> process::exit(code)
                "echo"  -> println!("{}", rest.collect::<Vec<_>>().join(" "))
                "type"  -> let t = rest.next();                                 # NEW
                           if let Some(t) = t {
                              if BUILTINS.contains(&t):
                                 println!("{} is a shell builtin", t)
                              else:
                                 println!("{}: not found", t)
                           }
                _       -> println!("{line}: command not found")
```

## 目录结构

```
codecrafters-shell-rust/
└── src/
    └── main.rs   # [MODIFY] 顶层新增 const BUILTINS: &[&str] = &["echo", "exit", "type"];
                  #          在现有 match cmd 中新增 "type" 分支：
                  #          - 用 parts.next() 取 target
                  #          - BUILTINS.contains(&target) 命中 → println!("{} is a shell builtin", target)
                  #          - 否则 → println!("{}: not found", target)
                  #          其他分支与流程保持不变。
```