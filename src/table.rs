//! 表格渲染模块，支持中英文混排对齐。

use unicode_width::UnicodeWidthChar;

/// 计算字符串的显示宽度（使用 unicode-width 精确计算，自动剥离 ANSI 转义码）
pub fn display_width(s: &str) -> usize {
    // 先剥离 ANSI 转义序列（colored crate 产生的 \x1b[...m）
    let stripped = strip_ansi(s);
    let mut width = 0;
    for ch in stripped.chars() {
        width += ch.width().unwrap_or(0);
    }
    width
}

/// 剥离 ANSI 转义序列
fn strip_ansi(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // 跳过 ESC[ ... m 序列
            if chars.peek() == Some(&'[') {
                chars.next(); // 消费 '['
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                // 其他 ESC 序列，跳过
            }
        } else {
            result.push(ch);
        }
    }
    result
}

/// 生成一行表格（数据行，统一用显示宽度对齐）
fn format_row(cells: &[String], widths: &[usize]) -> String {
    let parts: Vec<String> = cells
        .iter()
        .zip(widths.iter())
        .map(|(cell, w)| {
            let visible = display_width(cell);
            let padding = (*w).saturating_sub(visible);
            format!(" {}{} ", cell, " ".repeat(padding))
        })
        .collect();
    format!("|{}|", parts.join("|"))
}

/// 生成一行表格（表头，&str）
fn format_header_row(headers: &[&str], widths: &[usize]) -> String {
    let parts: Vec<String> = headers
        .iter()
        .zip(widths.iter())
        .map(|(h, w)| {
            let visible = display_width(h);
            let padding = (*w).saturating_sub(visible);
            format!(" {}{} ", h, " ".repeat(padding))
        })
        .collect();
    format!("|{}|", parts.join("|"))
}

/// 打印表格（自动计算列宽，支持中英文混排）
pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    if rows.is_empty() {
        return;
    }

    // 计算每列显示宽度
    let mut widths: Vec<usize> = headers.iter().map(|h| display_width(h)).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(display_width(cell));
            }
        }
    }

    // 分隔线
    let separator: String = widths
        .iter()
        .map(|w| "-".repeat(w + 2))
        .collect::<Vec<_>>()
        .join("+");
    println!("+{}+", separator);

    // 表头
    println!("{}", format_header_row(headers, &widths));
    println!("+{}+", separator);

    // 数据行
    for row in rows {
        println!("{}", format_row(row, &widths));
    }
    println!("+{}+", separator);
}
