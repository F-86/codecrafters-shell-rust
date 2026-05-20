---
name: quoted-executable-name-stage
overview: codecrafters shell「带引号的可执行文件名」stage：当前实现已天然支持（tokens[0] 与 args 共享同一套引号/转义解析），业务代码 0 改动；仅在 parser.rs 测试模块新增 ≥3 个针对 spec 样例的单测，并通过 /tmp shim 端到端验证 REPL 行为。
todos:
  - id: add-quoted-exec-tests
    content: 在 src/parser.rs 测试模块末尾新增 4 个 quoted_executable_* 单测，覆盖 spec 全部 4 类样例
    status: completed
  - id: run-cargo-test
    content: 运行 cargo test 验证新增用例通过且既有 32 个单测无回归，cargo build --release 无警告
    status: completed
    dependencies:
      - add-quoted-exec-tests
  - id: e2e-shim-verify
    content: 在 /tmp 创建两个含空格/引号的可执行 shim，临时 PATH 注入跑通 spec 两条样例命令，验证后清理
    status: completed
    dependencies:
      - run-cargo-test
---

## 用户需求

实现 codecrafters shell stage：**支持执行带引号的可执行文件名**（quoted executable names）。

## 核心功能

- 第一个 token（可执行文件名）与参数共享同一套引号 / 转义解析逻辑：
- `'my program' arg1` → 执行名为 `my program` 的程序
- `"exe with spaces" file.txt` → 执行名为 `exe with spaces` 的程序
- `"my 'program'"` → 执行名为 `my 'program'` 的程序
- 用「去引号后的纯名」在 PATH 中查找可执行文件并 spawn，其余 token 作为 argv 透传。
- 测试样例（renamed cat shim）：
- `'exe with "quotes"' file` → `content1`
- `"exe with 'single quotes'" file` → `content2`

## 范围与约束（已澄清）

- **业务代码零改动**：当前 `main.rs` 的 `cmd = tokens[0]` + `find_in_path(cmd)` + `Command::new(&path).arg0(cmd).args(args)` 已天然满足 spec 全部 3 条要求；本 stage 仅新增测试 + 端到端验证。
- **必须做端到端真机回放**：在 `/tmp` 创建带空格 / 引号的可执行 shim，按 spec 两条样例命令跑通后清理。
- **必须新增单测**：在 `src/parser.rs` 测试模块新增覆盖「引号可执行文件名作为 token[0]」的回归用例。

## Tech Stack

- 语言：Rust（沿用现有 `src/main.rs` + `src/parser.rs` 双文件结构）
- 测试：`cargo test`（沿用 `src/parser.rs` 末尾 `mod tests` 风格，`tokenize(r#"..."#).unwrap()` + `vec![...]` 断言）
- 端到端验证：bash + printf + 临时 PATH 注入 + 临时 shim 文件

## Implementation Approach

**核心判断**：spec 三条要求与当前实现的对应关系（已逐字核对 `src/main.rs:64-160`）：

| spec 要求 | 现有实现位置 | 状态 |
| --- | --- | --- |
| parser 正确剥离引号形成可执行文件名 | `parser::tokenize(line)` → `tokens[0]` | 上一 stage 已实现单 / 双引号 + 反斜杠转义 |
| 用 unquoted 名字在 PATH 中查找 | `find_in_path(cmd)` 接受 `&str`，对带空格 / 引号字节透明 | 已支持 |
| 找到后 spawn 并透传剩余 token 作 argv | `Command::new(&path).arg0(cmd).args(args)` | 已支持 |


→ **业务代码 0 改动**，本 stage 只补单测与 e2e 验证，确保未来重构不回归。

## Implementation Notes

- **测试用例命名**：沿用 snake_case，且 `quoted_executable_*` 前缀清晰标记本 stage 归属。
- **断言要点**：每个用例既验证 `tokens[0]`（可执行名去引号正确）也验证 `tokens[1..]`（剩余参数完整），避免单边遗漏。
- **e2e shim 设计**：shim 内容用 `#!/bin/sh\nexec cat "$@"`，通过 `chmod +x` + 临时 PATH 注入；文件名含双引号需 shell-escape 谨慎处理（`'exe with "quotes"'` 字面创建 + 单引号包裹路径）。
- **资源清理**：e2e 验证脚本无论成功失败都 `rm -f` 清理 `/tmp/exe with "quotes"` 与 `/tmp/exe with 'single quotes'` 两个 shim 及其辅助 fixture 文件。
- **零回归保证**：先跑 `cargo test` 确认既有 32 个单测仍全过，再跑新增 4 个用例。
- **不修改 doc-comment**：上一 stage 已声明 tokenize 完整支持单 / 双引号 + 转义，无需重复声明。

## Architecture Design

不引入任何新模块或新抽象。仅在以下位置追加内容：

```
src/parser.rs  # [MODIFY] 测试模块尾部追加 4 个 quoted_executable_* 单测
（无新文件、无新函数、无新依赖）
```

## Directory Structure

```
codecrafters-shell-rust/
└── src/
    └── parser.rs  # [MODIFY] 在文件末尾 `backslash_inside_single_quote_unchanged` 测试之后，
                   #          追加 4 个 quoted_executable_* 单元测试，覆盖：
                   #          1) 单引号包裹含空格的可执行名（spec 入门样例 'my program' argument1）
                   #          2) 双引号包裹含空格的可执行名（spec 入门样例 "exe with spaces" file.txt）
                   #          3) 双引号内嵌单引号字面的可执行名（spec 测试样例 "exe with 'single quotes'" file）
                   #          4) 单引号内嵌双引号字面的可执行名（spec 测试样例 'exe with "quotes"' file）
                   #          每个用例都断言完整 Vec<String>（含 tokens[0] 与剩余参数），
                   #          沿用既有 tokenize(r#"..."#).unwrap() + vec![...] 风格。
```

业务代码（`src/main.rs`、`src/parser.rs` 主体）不修改。