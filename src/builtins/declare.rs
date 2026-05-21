//! `declare` 内建：shell 变量存储 + `-p NAME` 描述打印 + NAME 合法性校验 + 值转义。
//!
//! NAME 字符判定基于 `parser::{is_name_start, is_name_cont}` 字符级 helper，
//! 与 `$VAR` 展开期 NAME 扫描 **100% 同源**。

use std::collections::HashMap;
use std::io::{self, Write};

/// 对 VALUE 中 4 个 bash「双引号上下文敏感字符」前加反斜杠（`\` / `"` / `$` / `` ` ``），
/// 便于 `declare -p` 输出可被 shell 直接 re-eval 还原同一变量。其它字符（空格、单引号、
/// `!`、`\n`、中文等）原样输出——bash 在双引号上下文中对它们也不做转义。
fn escape_for_double_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' | '"' | '$' | '`' => {
                out.push('\\');
                out.push(ch);
            }
            other => out.push(other),
        }
    }
    out
}

/// 校验 `name` 是否是合法 shell 变量标识符：`^[A-Za-z_][A-Za-z0-9_]*$`，ASCII-only。
///
/// 字符级判定与 `parser::tokenize` 的 `$VAR` 展开 NAME 扫描同源（共用
/// `is_name_start` / `is_name_cont`），跨 stage 100% 一致。
/// 详见 [docs/DESIGN_DECISIONS.md#parser-architecture](../../docs/DESIGN_DECISIONS.md#parser-architecture)。
fn is_valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !crate::parser::is_name_start(first) {
        return false;
    }
    chars.all(crate::parser::is_name_cont)
}

/// `declare` 内建：shell 变量存储 + `-p NAME` 描述打印。
///
/// 5 路分派（按顺序判定）：
/// 1. `declare NAME=VALUE` → 校验 NAME → 合法则 `vars.insert(NAME, VALUE)`，VALUE
///    来自 `splitn(2, '=')` 第二段（正确处理 `foo=a=b` → VALUE=`a=b`）
/// 2. `declare NAME`（不含 `=`） → 等价 `NAME=""`
/// 3. `declare -p NAME` → 先校验 NAME；命中 stdout `declare -- NAME="<escaped>"\n`，
///    未命中 stderr `declare: NAME: not found\n`
/// 4. 其它形态（空 args / `-p` 缺 NAME / `-x` / `-r` 等未实现 flag）→ 静默 Ok
///
/// NAME 非法（路径 1/2 引号内回显**原 arg 全文**；路径 3 回显 NAME 本身）→ stderr
/// `` declare: `<arg>': not a valid identifier\n `` 后短路返回 Ok。
///
/// 注：dispatch arm 必须留在 `_ => run_external` 之前，避免 `declare foo=bar` 误走 PATH。
pub fn run_declare(
    sink: &mut dyn Write,
    err_sink: &mut dyn Write,
    args: &[String],
    vars: &mut HashMap<String, String>,
) -> io::Result<()> {
    // 空 args（裸 `declare`）：静默 Ok，与「非主路径全静默」契约对齐。
    let first = match args.first() {
        Some(s) => s.as_str(),
        None => return Ok(()),
    };

    // 路径 3：`-p NAME ...` 查询打印
    if first == "-p" {
        if args.len() >= 2 {
            let name = &args[1];
            if !is_valid_identifier(name) {
                writeln!(err_sink, "declare: `{}': not a valid identifier", name)?;
                return Ok(());
            }
            match vars.get(name) {
                Some(value) => {
                    let escaped = escape_for_double_quote(value);
                    writeln!(sink, "declare -- {}=\"{}\"", name, escaped)?;
                }
                None => {
                    writeln!(err_sink, "declare: {}: not found", name)?;
                }
            }
        }
        return Ok(());
    }

    // 路径 4：未知 flag（`-x` / `-r` / `--` 等）：静默 Ok。
    if first.starts_with('-') {
        return Ok(());
    }

    // 路径 1 / 2：写入 store
    let mut iter = first.splitn(2, '=');
    let name = iter.next().unwrap_or("");
    let value = iter.next().unwrap_or("");
    if !is_valid_identifier(name) {
        writeln!(err_sink, "declare: `{}': not a valid identifier", first)?;
        return Ok(());
    }
    vars.insert(name.to_string(), value.to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invoke_declare(
        args: &[&str],
        vars: &mut HashMap<String, String>,
    ) -> (String, String) {
        let mut sink: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        run_declare(&mut sink, &mut err, &owned, vars).expect("run_declare");
        (
            String::from_utf8(sink).expect("utf8 stdout"),
            String::from_utf8(err).expect("utf8 stderr"),
        )
    }

    #[test]
    fn declare_p_missing_variable_writes_stderr() {
        let mut vars: HashMap<String, String> = HashMap::new();
        let (out, err) = invoke_declare(&["-p", "missing_variable"], &mut vars);
        assert!(out.is_empty(), "-p 未命中不写 stdout");
        assert_eq!(err, "declare: missing_variable: not found\n");
    }

    #[test]
    fn declare_p_any_unset_name_is_not_found() {
        let mut vars: HashMap<String, String> = HashMap::new();
        for name in &["FOO", "x", "Some_Var123", "alpha_1"] {
            let (out, err) = invoke_declare(&["-p", name], &mut vars);
            assert!(out.is_empty(), "-p {} 未命中不写 stdout", name);
            assert_eq!(err, format!("declare: {}: not found\n", name));
        }
    }

    #[test]
    fn declare_silent_paths_no_output() {
        let mut vars: HashMap<String, String> = HashMap::new();
        for args in &[
            &[][..],
            &["-p"][..],
            &["-x"][..],
            &["-r"][..],
        ] {
            let (out, err) = invoke_declare(args, &mut vars);
            assert!(out.is_empty(), "args {:?} 不应写 stdout，实际 {:?}", args, out);
            assert!(err.is_empty(), "args {:?} 不应写 stderr，实际 {:?}", args, err);
        }
        assert!(vars.is_empty(), "静默路径不应写入 store");
    }

    #[test]
    fn declare_assign_then_print_roundtrip() {
        let mut vars: HashMap<String, String> = HashMap::new();

        let (out, err) = invoke_declare(&["foo=bar"], &mut vars);
        assert!(out.is_empty(), "写入路径不写 stdout");
        assert!(err.is_empty(), "写入路径不写 stderr");
        assert_eq!(vars.get("foo").map(String::as_str), Some("bar"));

        let (out, err) = invoke_declare(&["-p", "foo"], &mut vars);
        assert_eq!(out, "declare -- foo=\"bar\"\n");
        assert!(err.is_empty(), "-p 命中不写 stderr");
    }

    #[test]
    fn declare_reassign_overwrites_value() {
        let mut vars: HashMap<String, String> = HashMap::new();
        invoke_declare(&["foo=bar"], &mut vars);
        invoke_declare(&["foo=bar2"], &mut vars);
        let (out, err) = invoke_declare(&["-p", "foo"], &mut vars);
        assert_eq!(out, "declare -- foo=\"bar2\"\n");
        assert!(err.is_empty());
    }

    #[test]
    fn declare_bare_name_declares_empty_value() {
        let mut vars: HashMap<String, String> = HashMap::new();
        invoke_declare(&["foo"], &mut vars);
        assert_eq!(vars.get("foo").map(String::as_str), Some(""));
        let (out, err) = invoke_declare(&["-p", "foo"], &mut vars);
        assert_eq!(out, "declare -- foo=\"\"\n");
        assert!(err.is_empty());
    }

    #[test]
    fn declare_p_escapes_special_chars() {
        let cases: &[(&str, &str)] = &[
            ("a\\b", "a\\\\b"),
            ("a\"b", "a\\\"b"),
            ("a$b", "a\\$b"),
            ("a`b", "a\\`b"),
            ("\\\"$`", "\\\\\\\"\\$\\`"),
            ("a b 'c' !d", "a b 'c' !d"),
            ("中文 ok", "中文 ok"),
        ];
        for (i, (value, escaped)) in cases.iter().enumerate() {
            let mut vars: HashMap<String, String> = HashMap::new();
            let name = format!("v{}", i);
            let assign = format!("{}={}", name, value);
            invoke_declare(&[assign.as_str()], &mut vars);
            let (out, err) = invoke_declare(&["-p", &name], &mut vars);
            assert_eq!(
                out,
                format!("declare -- {}=\"{}\"\n", name, escaped),
                "VALUE={:?} 期望转义 {:?}",
                value,
                escaped
            );
            assert!(err.is_empty(), "case {}: -p 命中不写 stderr", i);
        }
    }

    #[test]
    fn declare_value_with_equals_sign_preserved() {
        let mut vars: HashMap<String, String> = HashMap::new();
        invoke_declare(&["foo=a=b"], &mut vars);
        assert_eq!(vars.get("foo").map(String::as_str), Some("a=b"));
        let (out, err) = invoke_declare(&["-p", "foo"], &mut vars);
        assert_eq!(out, "declare -- foo=\"a=b\"\n");
        assert!(err.is_empty());
    }

    #[test]
    fn declare_p_after_set_then_unset_path_still_hit() {
        let mut vars: HashMap<String, String> = HashMap::new();
        let (_, err) = invoke_declare(&["-p", "x"], &mut vars);
        assert_eq!(err, "declare: x: not found\n");
        invoke_declare(&["x=1"], &mut vars);
        let (out, err) = invoke_declare(&["-p", "x"], &mut vars);
        assert_eq!(out, "declare -- x=\"1\"\n");
        assert!(err.is_empty());
    }

    // ---- Stage「Validating identifiers」：NAME 合法性校验用例 ----

    #[test]
    fn declare_invalid_name_starts_with_digit_with_value() {
        let mut vars: HashMap<String, String> = HashMap::new();
        let (out, err) = invoke_declare(&["67=x"], &mut vars);
        assert!(out.is_empty(), "非法 NAME 不写 stdout");
        assert_eq!(err, "declare: `67=x': not a valid identifier\n");
        assert!(vars.is_empty(), "非法 NAME 不应写入 store");
    }

    #[test]
    fn declare_invalid_name_starts_with_digit_no_value() {
        let mut vars: HashMap<String, String> = HashMap::new();
        let (out, err) = invoke_declare(&["67"], &mut vars);
        assert!(out.is_empty());
        assert_eq!(err, "declare: `67': not a valid identifier\n");
        assert!(vars.is_empty());
    }

    #[test]
    fn declare_invalid_name_with_dash() {
        let mut vars: HashMap<String, String> = HashMap::new();
        let (out, err) = invoke_declare(&["weird-name=v"], &mut vars);
        assert!(out.is_empty());
        assert_eq!(err, "declare: `weird-name=v': not a valid identifier\n");
        assert!(vars.is_empty());
    }

    #[test]
    fn declare_empty_name_is_invalid() {
        let mut vars: HashMap<String, String> = HashMap::new();
        let (out, err) = invoke_declare(&["=foo"], &mut vars);
        assert!(out.is_empty());
        assert_eq!(err, "declare: `=foo': not a valid identifier\n");
        assert!(vars.is_empty(), "空 NAME 不应写入 store（含空串键）");

        let (out, err) = invoke_declare(&["="], &mut vars);
        assert!(out.is_empty());
        assert_eq!(err, "declare: `=': not a valid identifier\n");
        assert!(vars.is_empty());
    }

    #[test]
    fn declare_p_invalid_name_reports_invalid_not_not_found() {
        let mut vars: HashMap<String, String> = HashMap::new();
        let (out, err) = invoke_declare(&["-p", "67"], &mut vars);
        assert!(out.is_empty());
        assert_eq!(err, "declare: `67': not a valid identifier\n");
        assert!(
            !err.contains("not found"),
            "非法 NAME 不应走 not-found 分支，实际 err = {:?}",
            err
        );
    }

    #[test]
    fn declare_valid_underscore_prefix_accepted() {
        let mut vars: HashMap<String, String> = HashMap::new();
        let (out, err) = invoke_declare(&["_FOO=BAR"], &mut vars);
        assert!(out.is_empty(), "合法写入路径不写 stdout");
        assert!(err.is_empty(), "合法写入路径不写 stderr");
        assert_eq!(vars.get("_FOO").map(String::as_str), Some("BAR"));

        let (out, err) = invoke_declare(&["-p", "_FOO"], &mut vars);
        assert_eq!(out, "declare -- _FOO=\"BAR\"\n");
        assert!(err.is_empty());
    }

    #[test]
    fn declare_valid_alpha_then_alnum_underscore() {
        let valid_names = ["a", "A_1", "foo_bar_123", "_", "_1"];
        for name in &valid_names {
            let mut vars: HashMap<String, String> = HashMap::new();
            let assign = format!("{}=v", name);
            let (out, err) = invoke_declare(&[assign.as_str()], &mut vars);
            assert!(out.is_empty(), "合法 NAME {:?} 不应写 stdout", name);
            assert!(err.is_empty(), "合法 NAME {:?} 不应写 stderr", name);
            assert_eq!(
                vars.get(*name).map(String::as_str),
                Some("v"),
                "合法 NAME {:?} 应被写入 store",
                name
            );
        }
    }

    #[test]
    fn declare_invalid_then_valid_with_same_name_root() {
        let mut vars: HashMap<String, String> = HashMap::new();

        let (out, err) = invoke_declare(&["1foo=x"], &mut vars);
        assert!(out.is_empty());
        assert_eq!(err, "declare: `1foo=x': not a valid identifier\n");
        assert!(vars.is_empty(), "非法路径不应写入 store");

        let (out, err) = invoke_declare(&["foo=x"], &mut vars);
        assert!(out.is_empty(), "后续合法写入不写 stdout");
        assert!(err.is_empty(), "后续合法写入不写 stderr");
        assert_eq!(vars.get("foo").map(String::as_str), Some("x"));
        assert_eq!(vars.len(), 1, "store 应仅含合法键 foo");
    }
}
