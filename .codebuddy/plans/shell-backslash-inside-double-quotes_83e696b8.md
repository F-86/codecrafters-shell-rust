---
name: shell-backslash-inside-double-quotes
overview: 扩展 InDoubleQuote 分支处理反斜杠转义。仅 src/parser.rs 改动。
todos:
  - id: extend-double-quote-backslash
    content: 扩展 src/parser.rs 的 State::InDoubleQuote 分支：新增反斜杠入口，对 4 字符集合（双引号、反斜杠、美元符、反引号）转义吃掉反斜杠，其他字符保留反斜杠且不消费下一字符；同步更新顶部 doc-comment 与枚举注释
    status: completed
  - id: add-double-quote-backslash-tests
    content: 在 parser.rs 测试模块新增 ≥8 个单测：双反斜杠转义、反斜杠双引号字面、反斜杠+字母保留反斜杠、三段拼接 spec 示例、cat 三文件路径、反斜杠美元符提前到位、反斜杠反引号提前到位、单引号内反斜杠回归守护
    status: completed
    dependencies:
      - extend-double-quote-backslash
  - id: verify-stage
    content: 运行 cargo test 验证全部单测通过，REPL 端到端验证 spec 全部样例（包括 cat 三文件与拼接收尾），cargo build --release 确认无警告
    status: completed
    dependencies:
      - add-double-quote-backslash-tests
---

## 需求概述

在现有 shell tokenizer 中实现「双引号内反斜杠转义」语义，与已实现的「引号外反斜杠转义」形成对照。

## 核心功能

- 双引号内反斜杠仅对 4 个字符触发转义并吃掉自身：双引号、反斜杠、美元符、反引号
- 双引号内反斜杠后跟其他任意字符（含字母、空白等）时按字面量保留反斜杠，下一字符正常处理
- spec 明确覆盖反斜杠+双引号、反斜杠+反斜杠两类；用户拍板将反斜杠+美元符、反斜杠+反引号也按 bash 真实行为提前到位（吃掉反斜杠），与未来变量展开阶段无缝衔接
- 行尾孤立反斜杠在双引号内由现有 UnterminatedDoubleQuote 兜底
- 单引号与引号外反斜杠语义完全不动

## 关键示例验证

- 双引号内 A BSBS escapes itself 输出 A BS escapes itself
- 双引号内 A BSDQ inside double quotes 输出 A DQ inside double quotes
- 双引号内 just'one'BSBSn'backslash 输出 just'one'BSn'backslash（反斜杠+n 是两字符字面量，不是换行符）
- inside BSDQ literal_quote.outside BSDQ 经过双引号段+Normal 续接+引号外反斜杠转义三段协作合成单 token inside DQ literal_quote.outside DQ
- cat 命令拼接三个含转义的双引号路径：/tmp/number 1、/tmp/doublequote DQ 2、/tmp/backslash BS 3

## 技术栈

- Rust 2021 edition，纯标准库
- 现有项目结构：src/parser.rs（tokenizer）+ src/main.rs（REPL）

## 实现策略

**单分支扩展，零侵入设计**：仅在 src/parser.rs 的 State::InDoubleQuote 分支新增反斜杠入口，其他状态、函数签名、错误枚举、main.rs 一律不动。

**核心算法**：在 InDoubleQuote 态遇到反斜杠时，使用 `chars.clone().next()` 做 O(1) peek（std::str::Chars 实现了 Clone，仅复制底层切片指针）：

- 若下一字符属于集合 {双引号, 反斜杠, 美元符, 反引号}：`chars.next()` 消费并仅 push 下一字符（反斜杠被吃掉）
- 否则：push 反斜杠字面，不消费下一字符（让下一字符在循环正常分支处理）

**关键决策与权衡**：

1. **不引入 Peekable<Chars>**：现有主循环已是 `while let Some(ch) = chars.next()` 显式迭代器，clone+next 做 peek 比包装 Peekable 改动更小，且 Chars 的 clone 是 O(1)（仅复制 &str 起止指针），无性能差异
2. **不合并两处反斜杠逻辑**：双引号内是「有条件转义」，引号外是「无条件转义」。例如 BS+n 在双引号内是 2 字符字面量、在引号外是 1 字符 n。强行抽公共函数会引入 mode 参数，反而降低可读性
3. **dollar 与 backtick 提前到位**：与 bash 真实行为一致，避免后续变量展开阶段回头改测试；当前 tokenizer 不展开变量，对外行为只是「反斜杠被吃掉」，零回归风险
4. **行尾兜底复用现有错误**：双引号内行尾孤立反斜杠 + EOF 仍是「双引号未闭合」（缺少闭合双引号），现有 UnterminatedDoubleQuote 自然覆盖，不新增错误变体

## 实现要点

- **复杂度**：仍为 O(n)，每字符最多被检视 2 次（peek + 正常消费）
- **不变量保护**：双引号内反斜杠+未识别字符必须保留反斜杠且**不消费**下一字符——例如 BS+n 时 peek 到 n 不在集合，push BS 并保留迭代器位置，下轮循环 n 走 `c => current.push(c)` 路径
- **拼接语义复用**：spec 示例 inside BSDQ literal_quote.outside BSDQ 的单 token 拼接靠现有 in_token 标记+三段协作天然达成（双引号段产出片段 → 闭合后 Normal 态续接 outside → 引号外反斜杠分支处理收尾 BSDQ），无需新增分支
- **doc-comment 同步**：文件顶部 doc 与 State::InDoubleQuote 枚举注释需更新，将「美元符与反斜杠在双引号内仍按字面量」改为「反斜杠仅对 4 个字符触发转义并吃掉自身，其他字符前反斜杠按字面量；美元符仍按字面量待变量展开阶段」
- **日志/错误风格一致**：复用现有 ParseError + Display + main.rs 的 eprintln!+continue 兜底，无新增 I/O 路径

## 目录结构

```
codecrafters-shell-rust/
├── src/
│   ├── parser.rs   # [MODIFY] 仅扩展 State::InDoubleQuote 分支新增反斜杠入口；同步更新顶部 doc-comment 与 InDoubleQuote 枚举注释；测试模块新增 ≥8 个单测
│   └── main.rs     # 无需改动
└── （其他文件不动）
```

### 文件修改详情

**src/parser.rs [MODIFY]**

- **职责**：命令行词法分析器，本次扩展双引号内反斜杠转义
- **改动点**：

1. 顶部模块 doc-comment（第 5-6 行）：更新双引号语义描述
2. State::InDoubleQuote 枚举上方注释（第 53 行附近）：同步更新
3. tokenize 函数 State::InDoubleQuote 分支：新增 `'\\' =>` 入口，分两路处理（在转义集合内则消费下一字符并 push、否则保留反斜杠不消费下一字符）
4. tests 模块：在末尾追加 ≥8 个单元测试，覆盖所有关键路径与回归守护

- **实现要求**：
- 仅修改 InDoubleQuote 分支，Normal 与 InSingleQuote 分支保持原状
- 使用 chars.clone().next() 做 peek，避免改动主循环迭代器类型
- 测试用例需验证 BS+n 是 2 字符字面量（区别引号外语义）
- 守护单引号内反斜杠仍按字面量（spec 范围之外）

## 关键代码骨架

```rust
State::InDoubleQuote => match ch {
    '"' => { state = State::Normal; }
    '\\' => {
        // 双引号内反斜杠：仅对 ", \, $, ` 这 4 个字符触发转义并吃掉自身；
        // 其他字符前反斜杠按字面量保留（与 Normal 态「无条件转义」的关键差异）。
        match chars.clone().next() {
            Some(next) if matches!(next, '"' | '\\' | '$' | '`') => {
                chars.next();          // 消费下一字符
                current.push(next);    // 仅 push 下一字符（反斜杠被吃掉）
            }
            _ => {
                current.push('\\');    // 保留反斜杠字面；不消费下一字符
            }
        }
    }
    c => { current.push(c); }
},
```