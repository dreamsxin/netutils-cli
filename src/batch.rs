//! 批量目标的并发编排。
//!
//! 只负责「按并发度跑完、结果按输入顺序回填、可被 Ctrl-C 停下」这三件事。
//! 每个目标完成时的实时输出留给调用方的闭包——那属于呈现，不属于编排。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

/// 一批目标的执行结果。
pub struct BatchRun<T> {
    /// 与输入顺序一致。被中断时未派发的目标不出现在这里。
    pub results: Vec<T>,
    /// 是否被 Ctrl-C 打断
    pub interrupted: bool,
}

/// 装上 Ctrl-C 监听，返回一个只会被置位的停止标志。
///
/// 刻意只置标志、不取消在飞任务：用 `select!` 包住整批会在中断时 drop 未完成的
/// future，把它们已经采到的数据一起丢掉，而中断时最需要的恰恰是「已经发生了
/// 什么」。已派发的目标跑完，未派发的不再开始。
fn install_stop_flag() -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            flag.store(true, Ordering::Relaxed);
        }
    });
    stop
}

/// 按 `parallel` 并发跑完 `targets`，结果按输入顺序回填。
///
/// 回填而非按完成顺序收集，是因为汇总输出必须与清单对齐；调用方若要实时进度，
/// 应在 `run_one` 内部自己打印。
pub async fn run_targets<T, F, Fut>(
    targets: Vec<String>,
    parallel: usize,
    run_one: F,
) -> BatchRun<T>
where
    F: Fn(usize, String) -> Fut,
    Fut: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let parallel = parallel.max(1);
    let stop = install_stop_flag();
    let semaphore = Arc::new(Semaphore::new(parallel));
    let total = targets.len();

    let mut slots: Vec<Option<T>> = Vec::with_capacity(total);
    slots.resize_with(total, || None);

    let mut set: JoinSet<(usize, T)> = JoinSet::new();
    for (index, target) in targets.into_iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        // 先拿许可再派发：既限制在飞数量，也顺带提供背压，清单很长时不会
        // 一次性把所有任务铺开。
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("semaphore closed");
        let task = run_one(index, target);
        set.spawn(async move {
            let _permit = permit;
            (index, task.await)
        });
    }

    while let Some(joined) = set.join_next().await {
        if let Ok((index, value)) = joined {
            slots[index] = Some(value);
        }
    }

    BatchRun {
        results: slots.into_iter().flatten().collect(),
        interrupted: stop.load(Ordering::Relaxed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    #[tokio::test]
    async fn restores_input_order_regardless_of_completion_order() {
        // 让后面的目标先完成：若按完成顺序收集，结果会是倒序
        let targets = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let run = run_targets(targets, 3, |index, target| async move {
            let delay = 30u64.saturating_sub(index as u64 * 10);
            tokio::time::sleep(Duration::from_millis(delay)).await;
            target
        })
        .await;

        assert_eq!(run.results, vec!["a", "b", "c"]);
        assert!(!run.interrupted);
    }

    #[tokio::test]
    async fn never_exceeds_the_requested_parallelism() {
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let targets: Vec<String> = (0..12).map(|i| i.to_string()).collect();
        let run = run_targets(targets, 3, |_, target| {
            let in_flight = in_flight.clone();
            let peak = peak.clone();
            async move {
                let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(5)).await;
                in_flight.fetch_sub(1, Ordering::SeqCst);
                target
            }
        })
        .await;

        assert_eq!(run.results.len(), 12);
        assert!(
            peak.load(Ordering::SeqCst) <= 3,
            "peak in-flight was {}",
            peak.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn treats_zero_parallelism_as_serial() {
        let targets = vec!["only".to_string()];
        let run = run_targets(targets, 0, |_, target| async move { target }).await;
        assert_eq!(run.results, vec!["only"]);
    }
}
