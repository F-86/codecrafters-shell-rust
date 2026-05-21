---
name: declare-validate-identifier
overview: "为 declare 内建增加 NAME 合法标识符校验：路径 1/2（赋值/空值声明）与路径 3（-p 查询）共用同一校验函数，非法 NAME 走 stderr `declare: \\`<原 arg>': not a valid identifier` 且不写入 store / 不报 not-found，合法路径行为完全保持现状。"
todos:
  - id: add-validator-helper
    content: 在 src/builtins.rs 新增 is_valid_identifier 私有 helper，紧邻 escape_for_double_quote，含完整文档注释（规则/空串/ASCII-only）
    status: completed
  - id: inject-validation-path12
    content: 改造 run_declare 路径 1/2：在 splitn 拆出 NAME 后插入校验，非法时 stderr 回显原 arg 全文（first）并 return Ok，移除原 name.is_empty 守卫
    status: completed
    dependencies:
      - add-validator-helper
  - id: inject-validation-path3
    content: 改造 run_declare 路径 3：在 let name=&args[1] 后、match vars.get 前插入校验，非法时 stderr 回显 args[1] 并 return Ok，避免误走 not-found
    status: completed
    dependencies:
      - add-validator-helper
  - id: upgrade-doc-comments
    content: 升级 run_declare 函数级文档：删除「不做合法标识符校验」旧条款，新增「NAME 合法性校验」节说明规则与回显源差异，更新行为表
    status: completed
    dependencies:
      - inject-validation-path12
      - inject-validation-path3
  - id: fix-and-add-tests
    content: 修正 declare_p_any_unset_name_is_not_found 的 NAME 列表为合法标识符，新增 8 个校验用例覆盖数字开头/含-/空 NAME/-p 非法/合法下划线/合法字母数字/非法不污染后续合法写入
    status: completed
    dependencies:
      - inject-validation-path12
      - inject-validation-path3
  - id: verify-build-and-e2e
    content: 运行 cargo build/test 验证零警告零回归，端到端 printf 注入题面 verbatim 用例（declare 23=x / _FOO=bar / -p _FOO）严格匹配 stdout/stderr
    status: completed
    dependencies:
      - fix-and-add-tests
      - upgrade-doc-comments
---

## 用户需求

为 `declare` 内建增加 **shell 变量名合法性校验**：变量名必须以字母或下划线开头，后续可跟字母、数字、下划线（即 `^[A-Za-z_][A-Za-z0-9_]*，ASCII-only）。非法名称时，shell 输出错误信息 `` declare: `<原 arg 全文>': not a valid identifier `` 到 stderr，且**不创建变量**；合法名称沿用既有写入/查询行为。

## 核心功能

- **路径 1/2（`declare NAME=VALUE` / `declare NAME`）**：校验拆出的 NAME 段。非法 → stderr 报错（引号内回显原 arg 全文，例如 `67=x`），不写入 store；合法 → 维持既有 `vars.insert` 行为。
- **路径 3（`declare -p NAME`）**：在查 store 之前先校验 NAME。非法 → stderr 报错（引号内回显 `args[1]`），不进 not-found 分支；合法 → 维持既有命中/未命中两路输出。
- **空 NAME 统一处理**：`declare =foo` 等空 NAME 情形归并到非法分支（与正则首字符规则天然一致），原「空 NAME 静默 Ok」占位逻辑被推翻。
- **错误信息格式 verbatim**：`` declare: `<arg>': not a valid identifier\n ``，左反引号 + 右单引号严格按题面。
- **不污染既有行为**：合法路径的存储/打印/转义/重赋值/not-found 全部保留；REPL 不中断。

## 技术栈

- **语言**：Rust（沿用现有项目）
- **改动范围**：`src/builtins.rs` 单文件（实现 + 测试）；`src/main.rs` 与 parser 不动
- **测试**：`cargo test` 单元测试（沿用 `invoke_declare(args, &mut HashMap)` 薄封装样板）

## 实现方案

### 整体策略

本阶段是对既有 `run_declare` 的**前置校验注入**，不引入新的数据结构、不改函数签名、不触碰 dispatch 层。所有逻辑封装在 `run_declare` 内部，权责清晰、爆炸半径最小。

### 关键设计决策

1. **新增私有 helper `is_valid_identifier(name: &str) -> bool`**：纯字符级扫描，O(n)，零分配。空串返回 false（首字符不存在），首字符要求 `is_ascii_alphabetic() || == '_'`，后续字符要求 `is_ascii_alphanumeric() || == '_'`。**不引入正则依赖**——避免 `regex` crate 的编译/二进制开销，bash 的 valid-identifier 规则是固定的小型 DFA，手写更高效。
2. **校验插入点选择**：

- 路径 3 在 `let name = &args[1]` 与 `match vars.get(name)` **之间**插入校验；非法时 `return Ok(())` 短路，避免误走 not-found 分支
- 路径 1/2 在 `splitn(2, '=')` 拆出 NAME 之后、`vars.insert` 之前插入校验；移除原 `if !name.is_empty()` 守卫（已被 `is_valid_identifier` 覆盖）

3. **错误信息回显源区分**：

- 路径 1/2：回显**原 arg 全文 `first`**（含 `=VALUE`，如 `67=x`）
- 路径 3：回显 **`args[1]`**（NAME 本身，因为 `-p` 路径下 args[1] 就是用户输入的 NAME）
- 题面 verbatim 用例 `declare 67=x` → ``declare: `67=x': not a valid identifier`` 已对齐

4. **既有 `escape_for_double_quote` helper 不动**：它服务于 `-p` 命中时的 VALUE 转义，与 NAME 校验正交。

### 性能与可靠性

- 校验函数 O(n) 字符扫描，n 通常 < 32（bash 习惯短变量名），零堆分配
- 校验失败 `return Ok(())`，IO 错误用 `?` 透传，REPL 不中断
- 单线程 REPL 无并发问题

### 避免技术债

- 复用既有 `run_declare` 5 路分派结构，仅前置插入校验，不重构
- 复用既有 `invoke_declare` 单测封装、HashMap 直注入模式，新增测试与既有 9 个用例同形

## 实现要点

### 性能

- `is_valid_identifier` 用 `chars().next()` + `chars().skip(1).all(...)` 或迭代器一次扫描；优先用 byte-level 判断（`as_bytes()`）因为 ASCII-only 校验下 byte 扫描比 char 扫描更快，无需考虑多字节 UTF-8 边界

### 日志

- 错误信息直接 `writeln!(err_sink, ...)` 到调用方注入的 stderr sink，与既有 not-found / numeric-argument-required 错误同形；不引入新的 logger，不打印调试日志

### 爆炸半径控制

- 改动文件 1 个（`src/builtins.rs`）
- `run_declare` 公开签名零变化，调用点（`src/main.rs` dispatch arm）零改动
- 既有 9 个 declare 单测中 8 个完全保留；仅 1 个 `declare_p_any_unset_name_is_not_found` 需把 NAME 列表收窄到合法标识符（`weird-name` / `0bad` 现在归非法分支，本就是错误期望）
- 不影响其它内建（`run_complete` / `run_history` 等）和集成测试

## 目录结构

```
src/
├── builtins.rs   # [MODIFY] run_declare 前置校验：新增 is_valid_identifier helper；路径 3 在查 store 前校验 NAME；路径 1/2 在 splitn 后校验 NAME 段，错误信息回显原 arg；升级文档注释；mod tests 修正 1 个既有用例 + 新增 ~8 个校验用例
└── main.rs       # [UNCHANGED] dispatch arm 签名兼容，无需触碰

tests/             # [UNCHANGED] 集成测试套件无需新增
```

## 关键代码结构

```rust
// 私有 helper：bash valid-identifier 校验（^[A-Za-z_][A-Za-z0-9_]*$，ASCII-only）
fn is_valid_identifier(name: &str) -> bool;
```