---
name: declare-store-and-print
overview: 为 declare 内建接入 shell 变量存储后端，实现 declare NAME=VALUE 写入、declare NAME 声明空值、declare -p NAME 命中分支按 bash 转义规则打印 declare -- NAME="VALUE"，同时保留既有 not-found 分支与 dispatch arm 模板。
todos:
  - id: add-vars-store
    content: "在 src/main.rs 新增 shell_vars: Rc<RefCell<HashMap>> 定义，注释复用 completions 框架并改写为变量存储语义"
    status: completed
  - id: rewrite-run-declare
    content: 在 src/builtins.rs 重写 run_declare 签名追加 vars 参数，实现写入/空值/-p 打印/not-found/静默五路分派，新增 escape_for_double_quote helper 处理 4 字符转义
    status: completed
    dependencies:
      - add-vars-store
  - id: wire-dispatch
    content: 在 src/main.rs declare arm 注入 &mut shell_vars.borrow_mut()，升级注释反映本阶段新行为表
    status: completed
    dependencies:
      - rewrite-run-declare
  - id: update-tests
    content: 改写 src/builtins.rs mod tests 中既有 3 个 declare 单测适配新签名，删除被推翻的 foo=bar 静默断言，新增写入回读/重赋值/空值/4 字符转义/含等号 VALUE/not-found 保留共 6 个用例
    status: completed
    dependencies:
      - rewrite-run-declare
  - id: verify-build-and-e2e
    content: 运行 cargo build / cargo test 验证零警告零回归，端到端 printf 注入题面 tester 用例序列验证 stdout/stderr 严格匹配
    status: completed
    dependencies:
      - wire-dispatch
      - update-tests
---

## 用户需求

codecrafters「Storing and displaying shell variables」阶段：让 shell 支持通过 `declare` 内建命令存储 shell 变量，并通过 `declare -p NAME` 打印变量描述。

## 核心功能

- **变量存储**：`declare NAME=VALUE` 把变量写入 shell 内部存储；同名变量重复 `declare` 后值被覆盖
- **变量打印**：
- `declare -p NAME` 命中 → stdout 输出 `declare -- NAME="VALUE"\n`
- `declare -p NAME` 未命中 → stderr 输出 `declare: NAME: not found\n`（沿用既有行为）
- **VALUE 转义**：打印时对 VALUE 中的 `\` `"`  ``` ` `` 四个特殊字符前加反斜杠，对齐 bash `declare -p` 真实行为
- **空值声明**：`declare NAME`（无 `=`）等价 `declare NAME=""`
- **静默兜底**：`declare`（无参）/ `declare -p`（缺 NAME）/ `declare -x` 等其它形态保持静默 Ok，不污染 stdout/stderr，也不走 PATH 兜底

## 验收契约（题面 verbatim）

```
$ declare foo=bar
$ declare -p foo
declare -- foo="bar"
$ declare -p missing_variable
declare: missing_variable: not found
$ declare foo=bar2
$ declare -p foo
declare -- foo="bar2"
```

## 技术栈

沿用现有 Rust 1.x 项目栈，零新增依赖。

## 实现策略

### 整体方案

1. **数据结构**：在 `src/main.rs` 新增 `shell_vars: Rc<RefCell<HashMap<String, String>>>`，与既有 `completions` 注册表完全同形；REPL 主循环外定义、dispatch 层借 `borrow_mut()` 注入 `run_declare`。
2. **`run_declare` 签名升级**：第 4 参追加 `vars: &mut HashMap<String, String>`，与 `run_complete` 同形，`Rc<RefCell<...>>` 在调用点解引用；函数体按 args 形态分派四条路径（写入 / 空值 / -p 打印 / 静默）。
3. **转义 helper**：新增私有 `escape_for_double_quote(s: &str) -> String`，按 bash 语义对 `\` `"` ` `` ` `` 前加反斜杠；其它字符（含空格、单引号、感叹号）原样输出。
4. **测试**：改写既有 3 个 declare 单测以适配新签名，新增覆盖写入/重赋值/空值/转义/含等号 VALUE/not-found 共 6 个用例。

### 关键技术决策

**为何选 `Rc<RefCell<HashMap>>` 而非裸 `HashMap`**

- 与既有 `completions` / `jobs_table` 注册表风格 100% 对齐，零新模式引入
- 为后续阶段「`$VAR` 展开」预留读端共享路径（`ShellHelper` / parser 都可拿 `Rc::clone()` 持有读端引用），不需要改 `run_declare` 签名
- 单线程 REPL 串行节奏天然不并发借用，借用作用域到 match arm 结束即释放

**为何 VALUE 解析仅用 `splitn(2, '=')`（q1 决策）**

- args 已被 parser 完成空白拆分 + 引号脱壳；`run_declare` 视角下每个 arg 都是一个独立 token
- `splitn(2, '=')` 仅切首个 `=`，正确处理 `declare foo=a=b`（VALUE = `a=b`）
- 简单可预测，能完整覆盖 tester 用例（`foo=bar` / `foo=bar2`）

**为何对 4 个字符做转义（q2 决策）**

- 与 bash `declare -p` 真实输出格式对齐，避免后续阶段（如 VALUE 含双引号或 `）回归
- 实现成本极低（一次扫描 + 4 字符 match），无性能顾虑
- 题面 tester 用例不含特殊字符，但额外转义不会破坏 tester 期望（`bar` → `bar` 原样）

**为何 `declare NAME` 写空串（q3 决策）**

- 对齐 bash 行为，提升内部一致性；后续 `declare -p NAME` 能正确命中并打印 `declare -- NAME=""`
- 等价语义：`declare NAME` ≡ `declare NAME=""`，分派路径统一收敛到 `vars.insert`

### 性能与复杂度

- 写入 O(1) 平均（HashMap insert）；查 O(1) 平均
- 转义 O(|VALUE|) 一次扫描，无回溯
- 无 N+1、无热路径回归；REPL 命令级吞吐远低于单次 HashMap 操作开销

### 实现注意（基于探索发现）

**复用既有模式（零新模式引入）**

- Registry 风格完全照抄 `completions`：`src/main.rs:75-84` 的注释框架（写端 dispatch、读端预留、单线程不并发借用）可平移到 `shell_vars` 注释
- 调用点模板照抄 `complete` arm：`src/main.rs:299-308` 的 `run_complete(... &mut completions.borrow_mut())` 模式
- 测试封装照抄 `invoke`：`src/builtins.rs:632-645` 的 `(stdout, stderr)` 字符串对断言模板

**dispatch arm 必须留在 `_ => run_external` 之前**

- 既有注释（`src/main.rs:502-504`）已强调此约束；本阶段保留并升级注释内容反映新行为表

**爆炸半径控制**

- 不触碰 parser（`$VAR` 展开 / 命令前缀赋值不在本阶段范围）
- 不触碰 BUILTINS 注册表（`"declare"` 已存在）
- 不触碰 import 行（`run_declare` 已在 `use builtins::{...}` 中）
- 既有 3 个 declare 单测仅签名适配 + 删除一条被推翻的断言（`foo=bar` 静默），其它正断言全部保留

**日志策略**

- 沿用既有 `eprintln!("shell: write error: {}", e)` IO 错误包裹模板（与 `run_history` / `run_complete` 调用点一字不差）
- `run_declare` 内部不做 trace 日志（REPL 内建命令历来无日志，避免污染 stderr 干扰 tester）

## 目录结构

```
src/
├── main.rs        # [MODIFY] 主循环。L75-82 之后追加 shell_vars 定义（Rc<RefCell<HashMap<String,String>>>，注释复用 completions 框架并改写为「shell 变量存储 / 写端 declare / 读端为后续 $VAR 展开预留」）；L494-511 declare arm 改为 run_declare(... &mut shell_vars.borrow_mut())，注释升级反映本阶段四条分派路径（写入 / 空值 / -p 打印含转义 / not-found / 其它静默）
└── builtins.rs    # [MODIFY] L611-626 重写 run_declare 函数体并升级文档注释；签名追加 vars: &mut HashMap<String, String>；新增私有 helper escape_for_double_quote(s: &str) -> String 处理 4 字符转义；mod tests 内改写 3 个既有 declare 单测适配新签名，新增 6 个用例覆盖写入回读 / 重赋值 / 空值声明 / 4 字符转义 / VALUE 含等号 / not-found 保留
```

## 关键代码契约

```rust
// src/builtins.rs - 函数签名（不含函数体）
pub fn run_declare(
    sink: &mut dyn Write,
    err_sink: &mut dyn Write,
    args: &[String],
    vars: &mut HashMap<String, String>,
) -> io::Result<()>;

// 私有 helper：对 \  "  $  ` 四字符前加反斜杠，其它原样
fn escape_for_double_quote(s: &str) -> String;
```

分派逻辑（伪码契约）：

- `args[0]` 不以 `-` 开头 → 取 `args[0]`，按首个 `=` 切分；含 `=` 则 `vars.insert(NAME, VALUE)`，不含 `=` 则 `vars.insert(NAME, "")`
- `args[0] == "-p"` 且 `args.len() >= 2` → 查 `vars.get(&args[1])`：Some(v) → stdout `declare -- {NAME}="{escape(v)}"\n`；None → stderr `declare: {NAME}: not found\n`
- 其它（空 args / 仅 `-p` / `-x` 等）→ 静默 Ok