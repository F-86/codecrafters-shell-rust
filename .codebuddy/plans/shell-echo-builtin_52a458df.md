---
name: shell-echo-builtin
overview: 在现有内建分发基础上新增 `echo` 内建：将其参数用单个空格连接后打印到 stdout，并以换行结尾。
todos:
  - id: impl-echo-builtin
    content: 在 src/main.rs 的 match cmd 中新增 "echo" 分支，使用 parts.collect + join(" ") + println! 实现
    status: completed
  - id: verify-echo
    content: 本地验证：echo hello world、echo pineapple strawberry、单参、无参均正确，且 exit / 未知命令无回归
    status: completed
    dependencies:
      - impl-echo-builtin
---

## 产品概述

在已有 Rust shell（提示符、REPL、未知命令报错、`exit` 内建）的基础上，新增 `echo` 内建命令：将其后的参数用单空格连接后打印到 stdout，并以换行结束。

## 核心功能

- **echo 内建**：识别命令名 `echo`，将剩余参数（按空白拆分）用单空格连接后整行输出，末尾自动换行。
- **保留既有行为**：`$ ` 提示符、`exit `终止、未知命令打印 `{cmd}: command not found`、空行跳过、EOF 自然退出，全部不变。

## 验收要点

- `echo hello world` → `hello world`
- `echo pineapple strawberry` → `pineapple strawberry`
- `echo foo` → `foo`
- 无参 `echo` → 输出空行
- 既有阶段功能无回归

## 技术栈

- 语言：Rust（edition = "2024", rust-version = "1.95"）
- 仅使用标准库；不引入新依赖
- 单文件结构：继续在 `src/main.rs` 内联 REPL 与内建分发

## 实现思路

在现有 `match cmd` 分发表中，新增 `"echo"` 分支：

1. 当前已有 `let mut parts = line.split_whitespace();` 并通过 `parts.next()` 取出 `cmd`，迭代器剩余即为参数序列。
2. echo 分支用 `parts.collect::<Vec<&str>>().join(" ")` 把剩余参数以单空格拼接，再 `println!` 输出（自动换行）。
3. 其余分支与控制流保持不变。

## 关键技术决策

- **复用 `split_whitespace` 拆分结果**：与 exit 分支共享同一拆分迭代器，避免二次解析；O(n) 完成，无额外开销。
- **使用 `join(" ")` 而非逐个 `print!`**：一次性写入 stdout，减少系统调用次数；语义清晰。
- **`println!` 自动换行**：满足"末尾换行"要求，无需手动 `\n`。
- **不处理引号/转义**：CodeCrafters 后续阶段才会引入；本阶段保持最简，避免过早设计；后续可在 echo 分支替换为更复杂的参数解析器。
- **无参兼容**：`parts.collect()` 为空时 `join` 得到 `""`，`println!("")` 输出空行，符合 POSIX `echo` 行为。

## 实施注意事项

- **blast radius 最小**：只在 `match cmd { ... }` 中插入一个新分支，约 3~4 行代码；不动既有 exit 分支与未知命令分支。
- **输出通道**：写入 stdout，与既有 `println!` 一致。
- **flush**：循环顶部已有 `stdout.flush()`，echo 输出由 println 行缓冲在下一次提示符前自然 flush；无需额外处理。
- **不抽模块**：仍保持单文件；待内建数量达到 3+ 个再考虑抽 `builtins.rs`，避免过度设计。

## 架构设计

```
loop:
  print "$ " + flush
  read_line
    Ok(0) -> break (EOF)
    Ok(_) -> trim
              if empty: continue
              split_whitespace -> (cmd, rest)
              match cmd:
                "exit"  -> parse rest[0] as i32 (default 0) -> process::exit(code)
                "echo"  -> println!("{}", rest.collect::<Vec<_>>().join(" "))   # NEW
                _       -> println!("{line}: command not found")
```

## 目录结构

```
codecrafters-shell-rust/
└── src/
    └── main.rs   # [MODIFY] 在现有 match cmd 中新增 "echo" 分支：
                  #          将 parts 剩余 token 收集为 Vec<&str>，用 " " join 后 println! 输出。
                  #          其他分支（exit、command not found）与提示符、读取、空行、EOF 等流程保持不变。
```