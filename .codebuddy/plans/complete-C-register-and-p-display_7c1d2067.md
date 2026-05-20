---
name: complete-C-register-and-p-display
overview: 为 `complete` 内建实现 `-C <path> <cmd>` 注册能力，并扩展 `-p` 在命中时打印归一化格式 `complete -C '<path>' <cmd>`；未命中沿用上阶段错误。
todos:
  - id: extend-run-complete
    content: 扩展 src/builtins.rs 的 run_complete 签名与三分支逻辑（-C 注册、-p 命中查询、-p 未命中错误）
    status: completed
  - id: wire-registry-and-dispatch
    content: 在 src/main.rs 主循环外声明 completions HashMap 并更新 dispatch 调用点传入 sink/registry
    status: completed
    dependencies:
      - extend-run-complete
  - id: verify-end-to-end
    content: cargo build 并非交互验证：-C 注册无输出、-p 命中归一化输出、多空格归一化、重复覆盖、未注册回退、其他 builtin 无回归
    status: completed
    dependencies:
      - wire-registry-and-dispatch
---

## Product Overview

为 CodeCrafters Rust Shell 的 `complete` 内建增加 `-C` 注册能力，并扩展 `-p` 标志：能从注册表中读出并以归一化格式打印已注册的补全脚本路径；保留上阶段未注册时的错误回退。

## Core Features

- `complete -C <path> <command>`：把 `<command> -> <path>` 写入跨命令存活的 registry，无任何输出
- `complete -p <command>` 命中 registry：向 stdout 输出 `complete -C '<path>' <command>`（单引号为字面字符，参数间精确单空格）
- `complete -p <command>` 未命中 registry：向 stderr 输出 `complete: <command>: no completion specification`（保留上阶段行为）
- 注册时多空格输入自动归一化（依托既有 tokenizer 的免费收益）
- 同一 command 重复注册自动覆盖
- `type complete` 等已通过阶段无回归

## Tech Stack

- 沿用现有项目：Rust + 标准库，无新增依赖；不修改 `Cargo.toml`，不新建文件
- 跨命令状态：`std::collections::HashMap<String, String>`（command → completer path）

## Implementation Approach

### 核心思路

在 `main.rs` REPL 主循环外部声明 `completions: HashMap<String, String>` 作为单一注册表；扩展 `run_complete` 接收 `&mut HashMap` 参数；按 args 形态分发到三个分支（注册 / 查询命中 / 查询未命中），其他形态静默 `Ok(())` 与上阶段风格一致。

### 关键决策

1. **registry 放在 main 的 loop 外，不引入 static/Lazy/Mutex**：单线程 REPL，无并发；放 loop 外天然跨命令存活；零额外依赖；与现有 `editor` 等局部变量同生命周期范围，符合最小侵入原则。
2. **`run_complete` 新签名加 `sink`**：`-p` 命中是**成功输出走 stdout**（与 bash 一致，可被 `>` 重定向），未命中是**错误走 stderr**（与上阶段一致）。`-C` 不写 sink。新签名 `(sink, err_sink, args, registry)` 与 `run_type` 风格对齐。
3. **依赖 tokenizer 完成空格归一化**：已确认 `parser::parse` 把 `complete   -C   /p   git` 拆成 `["complete", "-C", "/p", "git"]`，dispatch 收到的 args 已天然单空格无歧义——无需在 `run_complete` 内部再做空白处理。
4. **三分支精确匹配 + 其他静默**：避免误打分 codecrafters 后续阶段。

### 性能

- 注册/查询均为 HashMap O(1)，无热路径风险；空间复杂度 O(N)，N 为已注册命令数（REPL 会话级，量级极小）。

## Implementation Notes

- **复用既有 err_sink/sink 信道**：成功输出经 `*sink`（受 `>` 影响），错误信息经 `*err_sink`（受 `2>` 影响），与 `run_type`/`run_pwd` 完全一致；天然支持重定向，无需新增逻辑。
- **单引号是字面 ASCII 0x27**：直接写入 `"complete -C '{path}' {cmd}"`，不是 shell 转义产物。
- **blast radius 控制**：仅扩展 `run_complete` 签名 + main 调用点 + 新增 registry 变量；不动 tokenizer / parser / 其他 builtin / 外部命令路径；上阶段 `-p` 未命中行为完整保留作为查询未命中分支的回退。
- **不做之事（YAGNI）**：不校验 path 是否存在；不处理 path 内含空格/引号的转义（题目示例与测试均用简单路径）；不持久化 registry 跨进程；不动 `completion.rs`（TAB 补全候选源与 complete 注册表是两套独立体系）。

## Directory Structure

```
codecrafters-shell-rust/
└── src/
    ├── builtins.rs   # [MODIFY] 扩展 run_complete 签名为 
    │                 # (sink: &mut dyn Write, err_sink: &mut dyn Write, args: &[String],
    │                 #  registry: &mut HashMap<String, String>) -> io::Result<()>
    │                 # 三分支：
    │                 #   args == ["-C", path, cmd] → registry.insert(cmd.clone(), path.clone())，无输出
    │                 #   args == ["-p", cmd] 且命中 → sink 写 "complete -C '{path}' {cmd}\n"
    │                 #   args == ["-p", cmd] 未命中 → err_sink 写 "complete: {cmd}: no completion specification\n"
    │                 #   其他形态 → Ok(()) 静默
    │                 # 文件顶端 use 列表追加 std::collections::HashMap
    └── main.rs       # [MODIFY] 第 37 行 loop 之前声明
                      #   let mut completions: HashMap<String, String> = HashMap::new();
                      # 文件顶端 use 列表追加 std::collections::HashMap
                      # 第 127 行 "complete" 分支调用改为
                      #   run_complete(&mut *sink, &mut *err_sink, args, &mut completions)
                      # 兜底 eprintln! "shell: write error: {}" 保持不变
```

## Key Code Structures

```rust
// src/builtins.rs 新签名（仅契约，不含完整实现）
pub fn run_complete(
    sink: &mut dyn Write,
    err_sink: &mut dyn Write,
    args: &[String],
    registry: &mut HashMap<String, String>,
) -> io::Result<()>;
```