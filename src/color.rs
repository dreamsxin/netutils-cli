//! 颜色输出控制。
//!
//! 优先级（从高到低）：
//! 1. `--color always|never`
//! 2. JSON 输出模式强制关闭
//! 3. `NO_COLOR`（遵循 <https://no-color.org>：只要被设置为非空值即生效）
//! 4. `CLICOLOR_FORCE` 非 `0`
//! 5. `NETUTILS_COLOR=always|never|auto`
//! 6. `CLICOLOR=0`
//! 7. auto：stdout 是终端时启用

use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::ValueEnum;

use crate::output::OutputMode;

/// `init` 解析出的最终结果，供插件子进程环境变量复用。
static COLOR_ENABLED: AtomicBool = AtomicBool::new(false);

/// 颜色开关
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
#[value(rename_all = "lower")]
pub enum ColorChoice {
    /// 自动检测（默认）：stdout 为终端且未被环境变量禁用时启用
    #[default]
    Auto,
    /// 始终输出颜色，即使被重定向
    Always,
    /// 从不输出颜色
    Never,
}

impl ColorChoice {
    /// 传递给插件子进程的取值
    pub fn as_env_value(self) -> &'static str {
        match self {
            ColorChoice::Auto => "auto",
            ColorChoice::Always => "always",
            ColorChoice::Never => "never",
        }
    }
}

/// 环境变量读取抽象，便于测试
trait Env {
    fn get(&self, key: &str) -> Option<String>;
}

struct SystemEnv;

impl Env for SystemEnv {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

/// 应用颜色设置：解析后写入 `colored` 全局开关。
///
/// 解析结果同时缓存，供 [`effective`] 传递给插件子进程，使插件与核心行为一致。
pub fn init(choice: Option<ColorChoice>, mode: OutputMode) {
    let enabled = resolve(choice, mode, std::io::stdout().is_terminal(), &SystemEnv);
    colored::control::set_override(enabled);
    COLOR_ENABLED.store(enabled, Ordering::Relaxed);
}

/// 当前实际生效的颜色选择（`init` 之前为 [`ColorChoice::Never`]）。
pub fn effective() -> ColorChoice {
    if COLOR_ENABLED.load(Ordering::Relaxed) {
        ColorChoice::Always
    } else {
        ColorChoice::Never
    }
}

fn resolve(
    choice: Option<ColorChoice>,
    mode: OutputMode,
    stdout_is_terminal: bool,
    env: &dyn Env,
) -> bool {
    match choice {
        Some(ColorChoice::Always) => return true,
        Some(ColorChoice::Never) => return false,
        Some(ColorChoice::Auto) | None => {}
    }

    // JSON 是机器可读输出，任何 ANSI 转义都会破坏解析。
    if mode == OutputMode::Json {
        return false;
    }

    if env.get("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return false;
    }

    if env
        .get("CLICOLOR_FORCE")
        .is_some_and(|v| !v.is_empty() && v != "0")
    {
        return true;
    }

    match env
        .get("NETUTILS_COLOR")
        .map(|v| v.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("always") | Some("1") | Some("true") => return true,
        Some("never") | Some("0") | Some("false") => return false,
        _ => {}
    }

    if env.get("CLICOLOR").is_some_and(|v| v == "0") {
        return false;
    }

    stdout_is_terminal
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct MapEnv(HashMap<&'static str, &'static str>);

    impl MapEnv {
        fn new(pairs: &[(&'static str, &'static str)]) -> Self {
            MapEnv(pairs.iter().copied().collect())
        }
    }

    impl Env for MapEnv {
        fn get(&self, key: &str) -> Option<String> {
            self.0.get(key).map(|v| (*v).to_string())
        }
    }

    #[test]
    fn explicit_always_beats_no_color() {
        let env = MapEnv::new(&[("NO_COLOR", "1")]);

        assert!(resolve(
            Some(ColorChoice::Always),
            OutputMode::Json,
            false,
            &env
        ));
    }

    #[test]
    fn explicit_never_beats_terminal() {
        let env = MapEnv::new(&[]);

        assert!(!resolve(
            Some(ColorChoice::Never),
            OutputMode::Table,
            true,
            &env
        ));
    }

    #[test]
    fn json_mode_disables_color() {
        let env = MapEnv::new(&[]);

        assert!(!resolve(None, OutputMode::Json, true, &env));
    }

    #[test]
    fn no_color_env_disables_color() {
        let env = MapEnv::new(&[("NO_COLOR", "1")]);

        assert!(!resolve(None, OutputMode::Table, true, &env));
    }

    #[test]
    fn empty_no_color_is_ignored() {
        let env = MapEnv::new(&[("NO_COLOR", "")]);

        assert!(resolve(None, OutputMode::Table, true, &env));
    }

    #[test]
    fn clicolor_force_enables_without_terminal() {
        let env = MapEnv::new(&[("CLICOLOR_FORCE", "1")]);

        assert!(resolve(None, OutputMode::Table, false, &env));
    }

    #[test]
    fn no_color_beats_clicolor_force() {
        let env = MapEnv::new(&[("CLICOLOR_FORCE", "1"), ("NO_COLOR", "1")]);

        assert!(!resolve(None, OutputMode::Table, false, &env));
    }

    #[test]
    fn netutils_color_never_disables_color() {
        let env = MapEnv::new(&[("NETUTILS_COLOR", "Never")]);

        assert!(!resolve(None, OutputMode::Table, true, &env));
    }

    #[test]
    fn netutils_color_auto_falls_through_to_terminal() {
        let env = MapEnv::new(&[("NETUTILS_COLOR", "auto")]);

        assert!(resolve(None, OutputMode::Table, true, &env));
        assert!(!resolve(None, OutputMode::Table, false, &env));
    }

    #[test]
    fn clicolor_zero_disables_color() {
        let env = MapEnv::new(&[("CLICOLOR", "0")]);

        assert!(!resolve(None, OutputMode::Table, true, &env));
    }

    #[test]
    fn auto_follows_terminal_detection() {
        let env = MapEnv::new(&[]);

        assert!(resolve(None, OutputMode::Table, true, &env));
        assert!(!resolve(None, OutputMode::Table, false, &env));
    }
}
