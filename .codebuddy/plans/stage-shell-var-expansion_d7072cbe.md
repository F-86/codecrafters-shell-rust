---
name: stage-shell-var-expansion
overview: 在 tokenize 词法层增加 `$NAME` 变量展开：引号外与双引号内展开、单引号内不展开、未定义变量为空串、NAME 复用 `^[A-Za-z_][A-Za-z0-9_]*` 规则；`$` 后非合法首字符按字面量保留。
todos:
  - id: extract-name-helpers
    content: 在 src/parser/tokenize.rs 抽出 is_name_start / is_name_cont 字符级 helper（pub(crate)），并改写 src/builtins.rs 的 is_valid_identifier 基于二者实现，跨 stage 同源
    status: completed
  - id: extend-tokenize-signature
    content: "为 tokenize 增加 vars: &HashMap<String, String> 参数，并同步更新 parse_pipeline / parse / parser/mod.rs 文档与 re-export"
    status: completed
    dependencies:
      - extract-name-helpers
  - id: impl-dollar-expansion
    content: 在 tokenize 的 Normal 与 InDoubleQuote 两态主 ch 分支前插入 `$` 优先匹配子分支：合法 NAME 则贪婪扫描并查 vars push 值/空串、非法则 push '$' 字面降级；InSingleQuote 态保持原样
    status: completed
    dependencies:
      - extend-tokenize-signature
  - id: wire-main-call-site
    content: main.rs 第 225 行 parse_pipeline 调用点注入 &shell_vars.borrow()，并约束 borrow 作用域避免与下游 borrow_mut 冲突
    status: completed
    dependencies:
      - extend-tokenize-signature
  - id: audit-and-extend-tests
    content: 审计 src/parser/tests.rs 现有用例的 tokenize/parse 调用点适配新签名（多数传空表），修正含 `$` 字面预期的用例，并新增约 12 个 $VAR 展开覆盖用例（引号外/双引号/单引号/转义/边界/紧邻）
    status: completed
    dependencies:
      - impl-dollar-expansion
      - wire-main-call-site
  - id: verify-and-e2e
    content: 运行 cargo build / cargo test 确认零警告零回归，并端到端 printf 注入题面 verbatim 序列（declare Variable_1=Value_1 / Variable_2=Value2 / echo $Variable_1 $Variable_2）严格匹配 stdout 与 argv
    status: completed
    dependencies:
      - audit-and-extend-tests
---

## 产品概述

为 shell 添加 `$VAR` 形式的参数展开（parameter expansion）。当命令行包含 `$NAME` 且 `NAME` 已通过 `declare` 写入 shell 变量表时，shell 在调用 builtin 或 fork 外部程序之前，将其替换为变量值并作为独立 argv 词传递；变量本身的存储不受影响。

## 核心特性

- **引号外 `$VAR` 展开**：`echo $Variable_1 $Variable_2` → `echo` 收到展开后的两个独立词。
- **双引号内 `$VAR` 展开**：`echo "x$Y z"` 中 `$Y` 被替换，前后字面拼接成同一个词（对齐 bash）。
- **单引号内 `$VAR` 字面保留**：`echo '$Y'` 输出字面 `$Y`，不展开。
- **未定义变量 → 空串**：`echo $UNSET` 在 token 中保留为空串 token（与 `echo ""` 等价）。
- **NAME 字符集**：`^[A-Za-z_][A-Za-z0-9_]*`，与 `declare` 的 `is_valid_identifier` 同源。
- **` 后非法首字符 → 字面保留**：`$1abc`、`$-`、`$<空格>`、行尾孤立 ` 均按字面输出 ` + 后续原文。
- **`\ 转义**：`Normal` 与 `InDoubleQuote` 态下 `\ 已被现有反斜杠分支吃成字面 ` push，天然不再触发展开（无需新增标记位）。

## 技术栈

- 复用现有项目栈：Rust 2021 + std + rustyline，无新增依赖。

## 实现策略

**核心思路**：在词法层 `tokenize` 内为 `Normal` / `InDoubleQuote` 两态的主字符分支前插入「` 优先识别」子分支：

1. 当前字符是  `且下一字符满足 `is_name_start`（`[A-Za-z_]`）→ 进入 NAME 扫描子例程，贪婪消费 `is_name_cont`（`[A-Za-z0-9_]`）字符直到第一个非合法字符为止；用扫描出的 NAME 查 `vars`，命中则 push 值、未命中 push 空串；并置 `in_token = true`。
2. 否则（ `后是数字/`-`/空白/EOF/`" `等）：把 ` 当普通字面字符 push 到 `current`，进入下一轮主循环按原规则处理后续字符（即「` 字面降级」）。

**状态机不变量**：`InSingleQuote` 态完全不动 —— 现有 fall-through 已天然把 ` 按字面量保留。

**变量上下文注入**：`tokenize` / `parse_pipeline` / `parse`（`#[cfg(test)]` wrapper）签名新增 `vars: &HashMap<String, String>` 参数（不可变借用，O(1) HashMap 查询），从 REPL 主路径 `main.rs` 第 225 行 `parse_pipeline(line)` 调用点把 `&shell_vars.borrow()` 传入。展开发生在 token 生成阶段，下游 parse 层、redirect 层、exec 层零改动。

**关键设计决策**：

- **why tokenize 层而非更上层**：bash 真实语义中变量展开发生在 word-splitting 之前，而本项目「双引号 token 内字面拼接」逻辑也在 tokenize 内。上移到 parse 层会导致引号语义需重新设计，违背 KISS。
- **why HashMap 而非 trait 抽象**：当前唯一变量后端就是 `shell_vars`，引入 trait 是过度抽象（YAGNI）。未来若加环境变量回退，可在 main 层先 merge 再传同一个 HashMap。
- **why 复用 `is_valid_identifier` 字符集**：跨 stage 一致性 —— `declare` 拒绝 `67=x`、`$VAR` 也按同一字符集判定，用户心智模型零分裂。
- **why 不引入新 `ParseError`**：所有边界（未命中、` 后非法字符）都按字面量降级，REPL 用户无报错噪音。

## 性能与可靠性

- 时间复杂度：tokenize 整体仍 O(n)，` 分支的 NAME 扫描每个字符仅访问一次，不回溯。HashMap 查询 O(1)。
- 内存：`vars` 借用零拷贝；展开值 `clone` 一次写入 `current`（与现有引号字面 push 同形）。
- 无 panic 路径：HashMap 查询用 `get`，NAME 扫描用 `chars.clone().next()` 安全 peek。

## 实现注意事项

- **`\ 路径不变**：现有 `Normal` 态 `\` 分支与 `InDoubleQuote` 态 `\` 分支在 `\ 时一次性消费 ` 并 push 字面 —— 下一轮主循环看到的是 ` 之后的字符，**绝不会进入新增的 ` 展开分支**。无需任何状态位区分「字面 `」与「展开触发 `」。
- **未定义变量 → 空串 token 保留**：与 `echo ""` 行为一致，不在 argv 中过滤空串元素（避免影响显式 `""`）。
- **现有单测兼容**：`src/parser/tests.rs` 中含  `字面预期的用例需要审计 —— 凡传空 vars 表后 `$XXX` 会被展开为空串导致预期失配的，需就地修正测试预期或显式构造 vars 表。本阶段同步处理，零回归。
- **字符级 helper 共享**：抽出 `fn is_name_start(c: char) -> bool` 与 `fn is_name_cont(c: char) -> bool` 到 `src/parser/tokenize.rs`（或 `src/parser/mod.rs` 内私有模块），并让 `src/builtins.rs` 的 `is_valid_identifier` 改为基于这两个 helper 实现，跨 stage 同源。
- **日志策略**：tokenize 是热路径，新分支不打日志、不用 `dbg!`；错误以静默字面降级处理。
- **爆炸半径**：`tokenize` 签名变更涉及 4 处调用点（main.rs、parse.rs 内 `parse_pipeline`、parse.rs 内 `parse` wrapper、tests.rs 内单测）—— 全在 parser 模块内 + 一处 main，集中可控。

## 架构设计

完全沿用现有 parser 三层架构（tokenize / parse / mod），仅扩展 tokenize 层语义；上层契约（`Pipeline`/`ParsedCommand` 字段）零变更。

## 目录结构

```
src/
├── parser/
│   ├── tokenize.rs   # [MODIFY] 新增 is_name_start/is_name_cont 字符级 helper；
│   │                 #          tokenize 签名增加 vars: &HashMap<String, String> 参数；
│   │                 #          Normal 态主 ch 分支前插入 $ 优先匹配分支（命中 → 扫描 NAME → 查 vars push 值/空串；
│   │                 #          未命中 → push '$' 字面）；
│   │                 #          InDoubleQuote 态对称插入同款 $ 分支（仅扫描位置不同，逻辑一致）；
│   │                 #          InSingleQuote 态保持不变（天然字面）；
│   │                 #          模块头注释更新「$ 不再字面、改为变量展开」语义条款。
│   ├── parse.rs      # [MODIFY] parse_pipeline 与 parse 签名同步增加 vars: &HashMap<String, String>；
│   │                 #          内部 tokenize(input) 调用改为 tokenize(input, vars)；
│   │                 #          函数级 doc 增补「展开发生在 tokenize 层」说明。
│   ├── mod.rs        # [MODIFY] 模块头注释中关于 $ 字面的旧条款删除，新增「$VAR 展开」语义条款；
│   │                 #          re-export 不变。
│   └── tests.rs      # [MODIFY] 现有所有 tokenize/parse 调用点适配新签名（多数传 &HashMap::new()）；
│                     #          审计含 $ 字面预期的用例，按需调整；
│                     #          新增 ~12 个 $VAR 展开测试覆盖：引号外命中/未命中、双引号内命中
│                     #          /拼接/未命中、单引号内字面、\$ 引号外/双引号内字面、$1abc / $- /
│                     #          $<空格> / 行尾 $ 字面、$VAR$VAR2 紧邻、_underscore / 数字尾合法 NAME。
├── builtins.rs       # [MODIFY] is_valid_identifier 改写为基于 parser 模块新暴露的
│                     #          is_name_start/is_name_cont（pub(crate)）实现，零行为变化；
│                     #          函数级 doc 注明「字符集 helper 现由 parser 共享」。
└── main.rs           # [MODIFY] 第 225 行 parser::parse_pipeline(line) → parse_pipeline(line, &shell_vars.borrow())；
                      #          注意 borrow 作用域：把 borrow 收敛在临时绑定内或表达式内，
                      #          避免与下方 dispatch 中 shell_vars.borrow_mut() 冲突
                      #          （parse_pipeline 调用结束即 drop borrow，借用区间不重叠）。
```

## 关键代码结构

仅暴露 2 个字符级 helper 与 `tokenize` 新签名（其余实现细节走文本描述）：

```rust
// src/parser/tokenize.rs
pub(crate) fn is_name_start(c: char) -> bool;  // ASCII A-Za-z_ 首字符
pub(crate) fn is_name_cont(c: char) -> bool;   // ASCII A-Za-z0-9_ 后续字符
pub fn tokenize(input: &str, vars: &HashMap<String, String>) -> Result<Vec<String>, ParseError>;
```