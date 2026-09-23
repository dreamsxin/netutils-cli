//! 目标清单解析：从文件或标准输入读取待探测目标。
//!
//! 格式刻意做得宽容，因为清单通常是人手维护的：整行 `#` 注释、空行、
//! Windows 记事本留下的 BOM 与 CRLF 都要能吃下去。

use std::io::Read;

use crate::i18n::{t, t1};

/// 从 `spec` 读取目标清单。`-` 表示标准输入，其余按文件路径处理。
///
/// 返回的顺序与清单一致，且**不去重**——重复目标是合法的，用户可能有意
/// 对同一个目标多采几次。
pub fn load_targets(spec: &str) -> Result<Vec<String>, String> {
    let raw = if spec == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| t1("targets.read_fail", &format!("stdin: {e}")))?;
        buf
    } else {
        std::fs::read_to_string(spec)
            .map_err(|e| t1("targets.read_fail", &format!("{spec}: {e}")))?
    };

    let targets = parse_targets(&raw);
    if targets.is_empty() {
        return Err(t("targets.empty"));
    }
    Ok(targets)
}

/// 纯解析，便于测试：不碰文件系统。
fn parse_targets(raw: &str) -> Vec<String> {
    raw.lines()
        .map(|line| {
            // 记事本存下的清单带 UTF-8 BOM，它只会出现在第一行行首；
            // CRLF 的 \r 由 lines() 留在行尾。
            line.trim_start_matches('\u{feff}')
                .trim_end_matches('\r')
                .trim()
        })
        // 只认整行注释：目标字符串里可能出现 `#`，行内截断会改变语义
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_comments_and_blank_lines() {
        let raw = "# 生产入口\napi.example.com:443\n\n   \nweb.example.com:443\n# 尾注释\n";
        assert_eq!(
            parse_targets(raw),
            vec!["api.example.com:443", "web.example.com:443"]
        );
    }

    #[test]
    fn handles_crlf_and_bom() {
        let raw = "\u{feff}api.example.com:443\r\n10.0.0.10:8080\r\n";
        assert_eq!(
            parse_targets(raw),
            vec!["api.example.com:443", "10.0.0.10:8080"]
        );
    }

    #[test]
    fn keeps_order_and_duplicates() {
        // 重复不是错误：同一个目标多采几次是合理用法
        let raw = "b:1\na:1\nb:1\n";
        assert_eq!(parse_targets(raw), vec!["b:1", "a:1", "b:1"]);
    }

    #[test]
    fn keeps_hash_inside_a_target() {
        assert_eq!(parse_targets("http://h/x#frag\n"), vec!["http://h/x#frag"]);
    }

    #[test]
    fn yields_nothing_for_an_all_comment_file() {
        assert!(parse_targets("# a\n\n# b\n").is_empty());
    }

    #[test]
    fn reports_a_missing_file() {
        let missing = std::env::temp_dir().join("netutils-targets-does-not-exist.txt");
        let _ = std::fs::remove_file(&missing);
        assert!(load_targets(&missing.to_string_lossy()).is_err());
    }

    #[test]
    fn reports_a_file_with_no_usable_target() {
        let path = std::env::temp_dir().join("netutils-targets-all-comments.txt");
        std::fs::write(&path, "# only comments\n\n").expect("failed to write temp file");

        assert!(load_targets(&path.to_string_lossy()).is_err());

        let _ = std::fs::remove_file(path);
    }
}
