//! # 测试临时目录（仅测试构建）
//!
//! 目录名 = 进程 ID + 纳秒时间戳，**每个测试进程只创建一次**：既保证同一进程内同一
//! `tag` 始终映射到同一路径（不少用例会多次取同一库），又避免“上轮崩溃残留 + PID 复用”
//! 读到上一轮的数据库——那会让幂等键已存在、断言出现 `Deduplicated` 之类的假失败。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static ROOT: OnceLock<PathBuf> = OnceLock::new();

/// 本次测试进程的临时根目录（首次调用时创建）。
pub(crate) fn root() -> &'static Path {
    ROOT.get_or_init(|| {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let dir = std::env::temp_dir().join(format!("whcode-test-{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("创建测试临时目录");
        dir
    })
    .as_path()
}

/// 本次测试进程内按 `tag` 创建的独占子目录（同 tag 重复调用返回同一路径）。
pub(crate) fn dir(tag: &str) -> PathBuf {
    let dir = root().join(tag);
    std::fs::create_dir_all(&dir).expect("创建测试临时子目录");
    dir
}
