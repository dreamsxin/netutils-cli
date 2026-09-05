//! CI 断言引擎：把探测结果转成可判定的通过/失败，用退出码驱动流水线。
//!
//! 表达式语法 `<metric><op><value>`，例如：
//!
//! ```text
//! status=200          status!=500        status<400
//! latency<500ms       latency<0.5s       p95<800ms
//! success_rate>=99%   body*=healthy      final_url*=https://
//! ```
//!
//! 支持的运算符：`=`/`==`、`!=`、`<`、`<=`、`>`、`>=`、`*=`（包含子串）。
//! 数值可带单位 `ms`、`s`、`%`；时延类指标省略单位时按毫秒解释。

use std::collections::BTreeMap;

use colored::*;
use serde::Serialize;

/// 断言失败时的进程退出码，与「探测失败」(1) 区分，便于流水线分流。
pub const ASSERTION_EXIT_CODE: i32 = 3;

/// 比较运算符
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Contains,
}

impl Op {
    fn as_str(self) -> &'static str {
        match self {
            Op::Eq => "=",
            Op::Ne => "!=",
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Gt => ">",
            Op::Ge => ">=",
            Op::Contains => "*=",
        }
    }

    fn is_numeric_only(self) -> bool {
        matches!(self, Op::Lt | Op::Le | Op::Gt | Op::Ge)
    }
}

/// 断言右值
#[derive(Debug, Clone, PartialEq)]
enum Expected {
    Num(f64),
    Text(String),
}

/// 一条解析后的断言
#[derive(Debug, Clone)]
pub struct Assertion {
    raw: String,
    key: String,
    op: Op,
    expected: Expected,
}

/// 被断言的指标值
#[derive(Debug, Clone, PartialEq)]
pub enum MetricValue {
    Num(f64),
    Text(String),
    /// 该命令支持此指标，但本次运行取不到值（例如请求失败时没有状态码）。
    /// 与「指标名写错」区分开，错误信息才有指导意义。
    Unavailable(String),
}

impl MetricValue {
    fn render(&self) -> Option<String> {
        match self {
            MetricValue::Num(value) => Some(format_num(*value)),
            MetricValue::Text(value) => Some(value.clone()),
            MetricValue::Unavailable(_) => None,
        }
    }
}

/// 命令暴露给断言引擎的指标集合
#[derive(Debug, Default)]
pub struct Metrics(BTreeMap<String, MetricValue>);

impl Metrics {
    pub fn new() -> Self {
        Metrics(BTreeMap::new())
    }

    /// 写入数值指标（时延统一使用毫秒，比例统一使用百分数）
    pub fn num(&mut self, key: &str, value: f64) -> &mut Self {
        self.0.insert(key.to_string(), MetricValue::Num(value));
        self
    }

    /// 写入文本指标
    pub fn text(&mut self, key: &str, value: impl Into<String>) -> &mut Self {
        self.0
            .insert(key.to_string(), MetricValue::Text(value.into()));
        self
    }

    /// 声明一个本次取不到值的指标，并说明原因
    pub fn unavailable(&mut self, key: &str, reason: impl Into<String>) -> &mut Self {
        self.0
            .insert(key.to_string(), MetricValue::Unavailable(reason.into()));
        self
    }

    fn get(&self, key: &str) -> Option<&MetricValue> {
        self.0.get(key)
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(String::as_str).collect()
    }
}

/// 单条断言的判定结果
#[derive(Debug, Serialize)]
pub struct AssertionOutcome {
    /// 用户原始表达式
    pub expression: String,
    pub passed: bool,
    /// 实际取到的指标值；指标不存在时为 `None`
    pub actual: Option<String>,
    /// 失败原因；通过时为 `None`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 全部断言的汇总
#[derive(Debug, Serialize)]
pub struct AssertionReport {
    pub passed: bool,
    pub total: usize,
    pub failed: usize,
    pub results: Vec<AssertionOutcome>,
}

/// 解析一批表达式；任一条语法错误即整体失败（属于 CLI 用法错误）。
pub fn parse_all(expressions: &[String]) -> Result<Vec<Assertion>, String> {
    expressions.iter().map(|raw| parse(raw)).collect()
}

fn parse(raw: &str) -> Result<Assertion, String> {
    let trimmed = raw.trim();
    let (key, op, value) = split_expression(trimmed)
        .ok_or_else(|| format!("invalid assertion '{raw}', expected <metric><op><value>"))?;

    if key.is_empty() {
        return Err(format!("invalid assertion '{raw}', metric name is empty"));
    }
    if value.is_empty() {
        return Err(format!("invalid assertion '{raw}', value is empty"));
    }

    let key = canonical_key(key);
    let expected = parse_value(value);

    if op.is_numeric_only() && matches!(expected, Expected::Text(_)) {
        return Err(format!(
            "invalid assertion '{raw}', operator {} requires a numeric value",
            op.as_str()
        ));
    }
    if op == Op::Contains && matches!(expected, Expected::Num(_)) {
        // 数字子串匹配是合法需求（如 body*=200），按文本处理。
        return Ok(Assertion {
            raw: trimmed.to_string(),
            key,
            op,
            expected: Expected::Text(value.to_string()),
        });
    }

    Ok(Assertion {
        raw: trimmed.to_string(),
        key,
        op,
        expected,
    })
}

fn split_expression(input: &str) -> Option<(&str, Op, &str)> {
    // 双字符运算符必须先匹配，否则 `>=` 会被截成 `>`。
    const TWO_CHAR: [(&str, Op); 5] = [
        (">=", Op::Ge),
        ("<=", Op::Le),
        ("!=", Op::Ne),
        ("*=", Op::Contains),
        ("==", Op::Eq),
    ];
    for (token, op) in TWO_CHAR {
        if let Some(idx) = input.find(token) {
            let (key, rest) = input.split_at(idx);
            return Some((key.trim(), op, rest[token.len()..].trim()));
        }
    }
    for (token, op) in [(">", Op::Gt), ("<", Op::Lt), ("=", Op::Eq)] {
        if let Some(idx) = input.find(token) {
            let (key, rest) = input.split_at(idx);
            return Some((key.trim(), op, rest[token.len()..].trim()));
        }
    }
    None
}

/// 指标别名归一化，让用户可以写 `latency` 而不必写 `latency_ms`。
fn canonical_key(key: &str) -> String {
    let key = key.trim().to_ascii_lowercase().replace('-', "_");
    match key.as_str() {
        "latency" | "time" | "rtt" | "duration" | "total" => "latency_ms".to_string(),
        "avg" | "average" => "avg_ms".to_string(),
        "min" => "min_ms".to_string(),
        "max" => "max_ms".to_string(),
        "p50" | "median" => "p50_ms".to_string(),
        "p95" => "p95_ms".to_string(),
        "p99" => "p99_ms".to_string(),
        "success" | "success_ratio" => "success_rate".to_string(),
        "code" => "status".to_string(),
        _ => key,
    }
}

fn parse_value(value: &str) -> Expected {
    let lower = value.trim();
    for suffix in ["ms", "%"] {
        if let Some(number) = lower.strip_suffix(suffix) {
            if let Ok(parsed) = number.trim().parse::<f64>() {
                return Expected::Num(parsed);
            }
        }
    }
    // `s` 必须在 `ms` 之后判断，否则 "500ms" 会被当成 "500m" 秒。
    if let Some(number) = lower.strip_suffix('s') {
        if let Ok(parsed) = number.trim().parse::<f64>() {
            return Expected::Num(parsed * 1000.0);
        }
    }
    // 无单位的时延值按毫秒解释，其余数值按原样比较。
    if let Ok(parsed) = lower.parse::<f64>() {
        return Expected::Num(parsed);
    }
    Expected::Text(lower.to_string())
}

/// 逐条判定断言。
pub fn evaluate(assertions: &[Assertion], metrics: &Metrics) -> AssertionReport {
    let results: Vec<AssertionOutcome> = assertions
        .iter()
        .map(|assertion| evaluate_one(assertion, metrics))
        .collect();
    let failed = results.iter().filter(|result| !result.passed).count();
    AssertionReport {
        passed: failed == 0,
        total: results.len(),
        failed,
        results,
    }
}

fn evaluate_one(assertion: &Assertion, metrics: &Metrics) -> AssertionOutcome {
    let Some(actual) = metrics.get(&assertion.key) else {
        return AssertionOutcome {
            expression: assertion.raw.clone(),
            passed: false,
            actual: None,
            error: Some(format!(
                "unknown metric '{}' for this command; available: {}",
                assertion.key,
                metrics.keys().join_display()
            )),
        };
    };

    let rendered = actual.render();
    match (actual, &assertion.expected) {
        (MetricValue::Unavailable(reason), _) => AssertionOutcome {
            expression: assertion.raw.clone(),
            passed: false,
            actual: None,
            error: Some(format!(
                "metric '{}' is unavailable in this run: {reason}",
                assertion.key
            )),
        },
        (MetricValue::Num(left), Expected::Num(right)) => AssertionOutcome {
            expression: assertion.raw.clone(),
            passed: compare_num(*left, assertion.op, *right),
            actual: rendered,
            error: None,
        },
        (MetricValue::Text(left), Expected::Text(right)) => {
            let passed = match assertion.op {
                Op::Eq => left == right,
                Op::Ne => left != right,
                Op::Contains => left.contains(right.as_str()),
                _ => false,
            };
            AssertionOutcome {
                expression: assertion.raw.clone(),
                passed,
                actual: rendered,
                error: None,
            }
        }
        (MetricValue::Text(left), Expected::Num(right)) => {
            // 文本指标遇到数字右值时尝试数值化，失败则退回字符串比较。
            match left.parse::<f64>() {
                Ok(parsed) => AssertionOutcome {
                    expression: assertion.raw.clone(),
                    passed: compare_num(parsed, assertion.op, *right),
                    actual: rendered,
                    error: None,
                },
                Err(_) => AssertionOutcome {
                    expression: assertion.raw.clone(),
                    passed: false,
                    actual: rendered,
                    error: Some(format!(
                        "metric '{}' is not numeric, cannot compare with {}",
                        assertion.key,
                        format_num(*right)
                    )),
                },
            }
        }
        (MetricValue::Num(_), Expected::Text(right)) => AssertionOutcome {
            expression: assertion.raw.clone(),
            passed: false,
            actual: rendered,
            error: Some(format!(
                "metric '{}' is numeric, cannot compare with text '{}'",
                assertion.key, right
            )),
        },
    }
}

fn compare_num(left: f64, op: Op, right: f64) -> bool {
    match op {
        // 浮点相等使用容差，避免 99.99999999 != 100 之类的意外失败。
        Op::Eq => (left - right).abs() < f64::EPSILON * right.abs().max(1.0) * 4.0,
        Op::Ne => !compare_num(left, Op::Eq, right),
        Op::Lt => left < right,
        Op::Le => left <= right,
        Op::Gt => left > right,
        Op::Ge => left >= right,
        Op::Contains => false,
    }
}

fn format_num(value: f64) -> String {
    if (value.fract()).abs() < 1e-9 {
        format!("{}", value as i64)
    } else {
        format!("{value:.2}")
    }
}

trait JoinDisplay {
    fn join_display(&self) -> String;
}

impl JoinDisplay for Vec<&str> {
    fn join_display(&self) -> String {
        if self.is_empty() {
            "<none>".to_string()
        } else {
            self.join(", ")
        }
    }
}

/// 表格模式下渲染断言结果。
pub fn print_report(report: &AssertionReport) {
    println!();
    println!("{}", "Assertions".bold());
    for result in &report.results {
        let mark = if result.passed {
            "✅".to_string()
        } else {
            "❌".to_string()
        };
        let actual = result.actual.as_deref().unwrap_or("--");
        println!("  {} {} (actual: {})", mark, result.expression, actual);
        if let Some(error) = &result.error {
            println!("     {}", error.red());
        }
    }
    let summary = format!("  {}/{} passed", report.total - report.failed, report.total);
    if report.passed {
        println!("{}", summary.green());
    } else {
        println!("{}", summary.red());
    }
}

/// 判定后统一处理退出码：失败时置 [`ASSERTION_EXIT_CODE`]。
pub fn mark_exit_code(report: &AssertionReport) {
    if !report.passed {
        crate::output::mark_exit_code(ASSERTION_EXIT_CODE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(expr: &str) -> Assertion {
        parse(expr).expect("expression should parse")
    }

    fn metrics() -> Metrics {
        let mut m = Metrics::new();
        m.num("status", 200.0)
            .num("latency_ms", 350.0)
            .num("success_rate", 100.0)
            .num("p95_ms", 480.0)
            .text("final_url", "https://example.com/health")
            .text("body", "{\"status\":\"healthy\"}")
            .text("tls_version", "TLSv1.3");
        m
    }

    #[test]
    fn parses_operators_longest_first() {
        assert_eq!(one("status>=200").op, Op::Ge);
        assert_eq!(one("status<=200").op, Op::Le);
        assert_eq!(one("status!=200").op, Op::Ne);
        assert_eq!(one("status==200").op, Op::Eq);
        assert_eq!(one("status=200").op, Op::Eq);
        assert_eq!(one("status>200").op, Op::Gt);
        assert_eq!(one("status<200").op, Op::Lt);
        assert_eq!(one("body*=ok").op, Op::Contains);
    }

    #[test]
    fn normalizes_metric_aliases() {
        assert_eq!(one("latency<1s").key, "latency_ms");
        assert_eq!(one("p95<1s").key, "p95_ms");
        assert_eq!(one("Success-Rate>=99%").key, "success_rate");
        assert_eq!(one("code=200").key, "status");
    }

    #[test]
    fn parses_units() {
        assert_eq!(one("latency<500ms").expected, Expected::Num(500.0));
        assert_eq!(one("latency<0.5s").expected, Expected::Num(500.0));
        assert_eq!(one("latency<500").expected, Expected::Num(500.0));
        assert_eq!(one("success_rate>=99%").expected, Expected::Num(99.0));
    }

    #[test]
    fn rejects_malformed_expressions() {
        assert!(parse("status").is_err());
        assert!(parse("=200").is_err());
        assert!(parse("status=").is_err());
    }

    #[test]
    fn rejects_ordering_on_text_value() {
        let err = parse("tls_version<TLSv1.3").unwrap_err();
        assert!(err.contains("requires a numeric value"), "{err}");
    }

    #[test]
    fn numeric_assertions_pass_and_fail() {
        let m = metrics();
        assert!(evaluate(&[one("status=200")], &m).passed);
        assert!(evaluate(&[one("status!=500")], &m).passed);
        assert!(evaluate(&[one("latency<500ms")], &m).passed);
        assert!(evaluate(&[one("latency<0.3s")], &m).failed == 1);
        assert!(evaluate(&[one("success_rate>=99%")], &m).passed);
    }

    #[test]
    fn text_assertions_support_contains() {
        let m = metrics();
        assert!(evaluate(&[one("body*=healthy")], &m).passed);
        assert!(evaluate(&[one("final_url*=https://")], &m).passed);
        assert!(evaluate(&[one("tls_version=TLSv1.3")], &m).passed);
        assert_eq!(evaluate(&[one("body*=unhealthy")], &m).failed, 1);
    }

    #[test]
    fn unavailable_metric_is_distinguished_from_unknown_metric() {
        let mut m = Metrics::new();
        m.unavailable("status", "no HTTP response was received");

        let report = evaluate(&[one("status=200")], &m);

        assert!(!report.passed);
        assert_eq!(report.results[0].actual, None);
        let error = report.results[0].error.as_ref().unwrap();
        assert!(error.contains("is unavailable in this run"), "{error}");
        assert!(error.contains("no HTTP response"), "{error}");
        assert!(!error.contains("unknown metric"), "{error}");
    }

    #[test]
    fn unavailable_metric_fails_every_operator() {
        let mut m = Metrics::new();
        m.unavailable("latency_ms", "probe never completed");

        for expression in ["latency<1ms", "latency>1ms", "latency=1ms", "latency!=1ms"] {
            let report = evaluate(&[one(expression)], &m);

            assert!(!report.passed, "{expression} should not pass");
        }
    }

    #[test]
    fn unknown_metric_fails_with_available_list() {
        let m = metrics();
        let report = evaluate(&[one("nope=1")], &m);

        assert!(!report.passed);
        let error = report.results[0].error.as_ref().unwrap();
        assert!(error.contains("unknown metric 'nope'"), "{error}");
        assert!(error.contains("status"), "{error}");
    }

    #[test]
    fn numeric_metric_against_text_value_fails_clearly() {
        let m = metrics();
        let report = evaluate(&[one("status=abc")], &m);

        assert!(!report.passed);
        assert!(report.results[0]
            .error
            .as_ref()
            .unwrap()
            .contains("numeric"));
    }

    #[test]
    fn report_counts_multiple_assertions() {
        let m = metrics();
        let report = evaluate(&[one("status=200"), one("latency<1ms")], &m);

        assert_eq!(report.total, 2);
        assert_eq!(report.failed, 1);
        assert!(!report.passed);
    }

    #[test]
    fn parse_all_propagates_first_error() {
        let result = parse_all(&["status=200".to_string(), "broken".to_string()]);

        assert!(result.is_err());
    }

    #[test]
    fn missing_metric_renders_double_dash_in_table() {
        let report = evaluate(&[one("nope=1")], &Metrics::new());

        assert_eq!(report.results[0].actual, None);
    }
}
