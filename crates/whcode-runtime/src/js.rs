//! # `js` 运行时（T110、D-08：受限 ECMAScript 引擎）
//!
//! 原 `providers.rs` 的 JsProvider 原文迁移；T110 起按 feature `runtime-js` 双路径实现：
//!
//! - **默认构建**（无 feature）：诚实不可用——`is_available() == false`，`execute` 返回
//!   `runtime_unavailable` 结构化错误（FR-704/NFR-102：默认构建不引入引擎依赖、零开销）；
//! - **feature `runtime-js`**：内置 Boa 受限 ECMAScript 引擎（ADR-029），进程内求值，
//!   `script.input` 以**冻结全局字符串** `input` 注入，结果值 ToString 后进
//!   [`crate::worker::WorkerResult`]，解释器限制有界（NFR-215）。
//!
//! 能力边界（D-08、§4.7.1、Check 109/153）：**Boa ≠ 完整 Node.js**。`require`、
//! `process`、`fs`、`net`、`http`、`child_process` 调用一律返回含“能力不足”的结构化
//! 失败（退出码 1 + stderr 消息），不崩溃、不静默返回 undefined；声明保持
//! Restricted + 三项 unsupported 不变。
//!
//! 与 ADR-008 的和解：引擎在主进程内求值，但 Boa 无宿主函数、无 IO、无网络——脚本
//! 只能触达本模块显式注册的全局名（内置对象 + 冻结 `input` + 拒绝面），沙箱能力与
//! 受控子进程等价；该决策与复审条件由 ADR-029 记录。

use whcode_domain::WhError;

use crate::capability::{Capability, RuntimeDeclaration};
use crate::checkpoint::Checkpoint;
#[cfg(feature = "runtime-js")]
use crate::provider::ensure_available;
use crate::provider::{RuntimeProvider, Script};
#[cfg(feature = "runtime-js")]
use crate::worker::{MAX_OUTPUT_BYTES, WorkerResult};
#[cfg(feature = "runtime-js")]
use boa_engine::Source;

/// 声明默认值（D-08：Restricted + 三项不支持；双路径共用，不得宣称完整 Node.js）。
fn default_declaration() -> RuntimeDeclaration {
    RuntimeDeclaration {
        name: "js".to_string(),
        capability: Capability::Restricted,
        unsupported: vec![
            "Node.js 内置模块".to_string(),
            "文件系统".to_string(),
            "网络".to_string(),
        ],
    }
}

/// `js` 运行时（T110、D-08：受限 ECMAScript 引擎）。
#[derive(Debug, Clone)]
pub struct JsProvider {
    declaration: RuntimeDeclaration,
}

impl Default for JsProvider {
    fn default() -> Self {
        Self {
            declaration: default_declaration(),
        }
    }
}

#[cfg(not(feature = "runtime-js"))]
impl RuntimeProvider for JsProvider {
    fn name(&self) -> &str {
        "js"
    }
    fn declaration(&self) -> &RuntimeDeclaration {
        &self.declaration
    }
    fn is_available(&self) -> bool {
        // D-08：未内置引擎（Boa 等属后续接入）——诚实不可用，不伪装。
        false
    }
    fn env_fingerprint(&self) -> String {
        "js/unavailable".to_string()
    }
    fn execute(&self, _script: &Script) -> Result<Checkpoint, WhError> {
        Err(crate::errors::runtime_unavailable(
            "js",
            crate::errors::UnavailableReason::Missing,
            "受限 ECMAScript 引擎未内置（D-08：不宣称完整 Node.js）",
        ))
    }
    fn terminate(&mut self) -> Result<(), WhError> {
        Ok(())
    }
}

#[cfg(feature = "runtime-js")]
impl RuntimeProvider for JsProvider {
    fn name(&self) -> &str {
        "js"
    }
    fn declaration(&self) -> &RuntimeDeclaration {
        &self.declaration
    }
    fn is_available(&self) -> bool {
        // 引擎经 feature 静态编入（无运行时探测必要）：feature 在即可用。
        true
    }
    fn env_fingerprint(&self) -> String {
        // Boa 0.21.1 未导出公开版本常量（已核实 0.21.1 源码无此 API），此处声明与
        // Cargo.toml 的 boa_engine = 0.21.1 兼容线一致的主次版本；精确补丁版本由
        // Cargo.lock 锁定。复审触发：升级 boa_engine 时必须同步此字符串（ADR-029）。
        "boa/0.21/js/restricted".to_string()
    }
    fn execute(&self, script: &Script) -> Result<Checkpoint, WhError> {
        ensure_available(self)?;
        let started = std::time::Instant::now();
        let mut context = engine::new_context(&script.input)
            .map_err(|error| engine::setup_error("初始化", error))?;
        let evaluated = context.eval(Source::from_bytes(script.script.as_str()));
        let elapsed = started.elapsed();
        // 墙钟检查（事后判定）：boa 0.21 无解释器中断钩子，无法在同步求值中途按墙钟
        // 打断；死循环的最坏不可中断窗口由 engine::LOOP_ITERATION_LIMIT（固定操作数）
        // 封顶，超限即结构化失败。求值完成后已超时限的按超时报告（与 worker.rs 子进程
        // 超时同语义：exit_code None + timed_out）。
        if elapsed >= script.timeout {
            return Ok(Checkpoint {
                script_hash: crate::payload::fnv64(script.script.as_bytes()),
                input_hash: crate::payload::fnv64(script.input.as_bytes()),
                runtime_fingerprint: self.env_fingerprint(),
                workdir_policy: "in_process".to_string(),
                result: Some(WorkerResult {
                    exit_code: None,
                    stdout_tail: String::new(),
                    stderr_tail: String::new(),
                    timed_out: true,
                    cancelled: false,
                }),
            });
        }
        // 脚本级失败（语法错误、运行时异常、Node API 拒绝面、解释器上限）与子进程协议
        // 同语义：退出码 1 + stderr 尾部——不崩溃、不静默（Check 109/153）。
        let result = match evaluated {
            Ok(value) => match value.to_string(&mut context) {
                Ok(text) => WorkerResult {
                    exit_code: Some(0),
                    stdout_tail: engine::bounded_tail(
                        &text.to_std_string_escaped(),
                        MAX_OUTPUT_BYTES,
                    ),
                    stderr_tail: String::new(),
                    timed_out: false,
                    cancelled: false,
                },
                Err(error) => engine::script_failure(&error),
            },
            Err(error) => engine::script_failure(&error),
        };
        Ok(Checkpoint {
            script_hash: crate::payload::fnv64(script.script.as_bytes()),
            input_hash: crate::payload::fnv64(script.input.as_bytes()),
            runtime_fingerprint: self.env_fingerprint(),
            workdir_policy: "in_process".to_string(),
            result: Some(result),
        })
    }
    fn terminate(&mut self) -> Result<(), WhError> {
        // 每次执行独立装配上下文（无跨执行常驻状态；资源纪律：空闲态零占用）。
        // 同步求值无法中途打断，取消语义为“不发起下一次执行”——该限制显式声明而非
        // 隐含默认；复审触发：boa 引入中断钩子后改为可打断（ADR-029）。
        Ok(())
    }
}

#[cfg(feature = "runtime-js")]
mod engine {
    //! Boa 受限引擎装配（仅 feature `runtime-js` 编入；FR-704/NFR-102：默认构建零引擎依赖）。
    //!
    //! 有界性（NFR-215）：解释器限制经 `Context::runtime_limits_mut` 设置——boa 0.21 的
    //! 实际 API 为 `boa_engine::vm::RuntimeLimits` 的 `set_loop_iteration_limit` 与
    //! `set_recursion_limit`（无 `RuntimeLimit::LoopOperations` 之名，以实际为准）；
    //! boa 0.21 无墙钟中断钩子，墙钟只在求值前后判定（见父模块 execute 注释）。

    use boa_engine::property::Attribute;
    use boa_engine::{
        Context, JsError, JsNativeError, JsResult, JsString, JsValue, NativeFunction, js_string,
    };
    use whcode_domain::{ErrorCode, WhError};

    use crate::worker::MAX_OUTPUT_BYTES;

    /// 单脚本循环迭代上限：超出即结构化失败（死循环窗口被固定操作数封顶，NFR-215）。
    pub(crate) const LOOP_ITERATION_LIMIT: u64 = 1_000_000;
    /// 函数递归上限。
    pub(crate) const RECURSION_LIMIT: usize = 256;

    /// 拒绝面单条目（全局名 + 形参数量 + 拒绝函数指针）。
    type Rejection = (&'static str, usize, NativeFunction);

    /// Node API 拒绝面（Check 109/153）：调用即抛“运行时 js 能力不足”的伪装全局名。
    /// 单一事实来源：引擎装配注册与本文件测试共用，不得静默返回 undefined。
    pub(crate) fn rejection_surface() -> [Rejection; 6] {
        [
            ("require", 1, NativeFunction::from_fn_ptr(reject_require)),
            ("process", 0, NativeFunction::from_fn_ptr(reject_process)),
            ("fs", 0, NativeFunction::from_fn_ptr(reject_fs)),
            ("net", 0, NativeFunction::from_fn_ptr(reject_net)),
            ("http", 0, NativeFunction::from_fn_ptr(reject_http)),
            (
                "child_process",
                0,
                NativeFunction::from_fn_ptr(reject_child_process),
            ),
        ]
    }

    /// 构造“运行时 js 能力不足”错误（消息含运行时名 js、API 名与边界说明；D-08/FR-708）。
    fn insufficient(api: &str) -> JsError {
        JsNativeError::error()
            .with_message(format!(
                "运行时 js 能力不足：Node.js API {api} 不受支持（受限 ECMAScript 引擎；无 Node.js 内置模块、文件系统与网络）"
            ))
            .into()
    }

    // 拒绝面函数体。boa 0.21 的安全原生函数只有 fn 指针与 Copy 闭包两条路；
    // fn 指针零捕获、无需 unsafe 闭包 API，逐名展开是最稳写法。
    fn reject_require(
        _this: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<JsValue> {
        Err(insufficient("require"))
    }

    fn reject_process(
        _this: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<JsValue> {
        Err(insufficient("process"))
    }

    fn reject_fs(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
        Err(insufficient("fs"))
    }

    fn reject_net(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
        Err(insufficient("net"))
    }

    fn reject_http(
        _this: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<JsValue> {
        Err(insufficient("http"))
    }

    fn reject_child_process(
        _this: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<JsValue> {
        Err(insufficient("child_process"))
    }

    /// 装配期错误 → 结构化 WhError（仅初始化/装配失败走此路径；脚本级失败走退出码 1）。
    pub(crate) fn setup_error(stage: &str, error: JsError) -> WhError {
        WhError::new(ErrorCode::Internal, format!("js 引擎{stage}失败：{error}"))
    }

    /// 脚本级失败 → 退出码 1 + stderr 尾部（与子进程协议同语义；Check 109/153）。
    pub(crate) fn script_failure(error: &JsError) -> crate::worker::WorkerResult {
        crate::worker::WorkerResult {
            exit_code: Some(1),
            stdout_tail: String::new(),
            stderr_tail: bounded_tail(&error.to_string(), MAX_OUTPUT_BYTES),
            timed_out: false,
            cancelled: false,
        }
    }

    /// 有界输出尾部：语义对齐 `worker::tail`（只保留尾部、不超过 max 字节）；输入是
    /// Rust 字符串，先按 UTF-8 字符边界回退再转换，避免把多字节字符截成替换符。
    pub(crate) fn bounded_tail(text: &str, max: usize) -> String {
        let bytes = text.as_bytes();
        if bytes.len() <= max {
            return text.to_string();
        }
        let mut start = bytes.len() - max;
        while start < bytes.len() && (bytes[start] & 0b1100_0000) == 0b1000_0000 {
            start += 1;
        }
        String::from_utf8_lossy(&bytes[start..]).to_string()
    }

    /// 装配受限上下文：解释器限制（有界）+ 冻结全局 input + Node API 拒绝面。
    ///
    /// # Errors
    ///
    /// 全局注册失败（重复定义等装配期错误）时返回 [`JsError`]。
    pub(crate) fn new_context(input: &str) -> Result<Context, JsError> {
        let mut context = Context::default();
        let limits = context.runtime_limits_mut();
        limits.set_loop_iteration_limit(LOOP_ITERATION_LIMIT);
        limits.set_recursion_limit(RECURSION_LIMIT);
        // 冻结全局字符串 input：Attribute 的 READONLY/PERMANENT 均为 0 位，
        // 只置 ENUMERABLE 即 writable=false + configurable=false（属性冻结，可读不可写）。
        context.register_global_property(
            js_string!("input"),
            JsString::from(input),
            Attribute::ENUMERABLE,
        )?;
        for (name, length, function) in rejection_surface() {
            context.register_global_callable(JsString::from(name), length, function)?;
        }
        Ok(context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 原 providers.rs 的 js 分支断言（原文保留；仅默认构建运行——
    /// feature `runtime-js` 开启后引擎路径可用，断言见下方 engine_path 子模块）。
    #[cfg(not(feature = "runtime-js"))]
    #[test]
    fn js_without_engine_is_honestly_unavailable() {
        let mut js = JsProvider::default();
        assert!(!js.is_available(), "js 无引擎诚实不可用（D-08）");
        let err = js
            .execute(&Script {
                script: "1".to_string(),
                input: String::new(),
                timeout: std::time::Duration::from_secs(1),
            })
            .expect_err("不可用必须结构化错误");
        assert_eq!(err.code, whcode_domain::ErrorCode::RuntimeUnavailable);
        js.terminate().expect("终止");
    }

    /// 声明不变（D-08/§4.7.1）：双路径下都必须保持 Restricted + 三项不支持，
    /// 指纹不得宣称 Node.js。
    #[test]
    fn declaration_stays_restricted_with_unsupported_list() {
        let js = JsProvider::default();
        let declaration = js.declaration();
        assert_eq!(declaration.name, "js");
        assert_eq!(declaration.capability, Capability::Restricted);
        assert_eq!(
            declaration.unsupported,
            vec![
                "Node.js 内置模块".to_string(),
                "文件系统".to_string(),
                "网络".to_string(),
            ]
        );
        assert!(
            !js.env_fingerprint().contains("node"),
            "指纹不得宣称 Node.js（D-08）：{}",
            js.env_fingerprint()
        );
    }

    /// 引擎路径测试（feature `runtime-js`）：固定测试集、Node API 拒绝面、冻结输入、
    /// 有界解释器、墙钟超时与基准（Check 109/153、V-302）。
    #[cfg(feature = "runtime-js")]
    mod engine_path {
        use crate::js::{JsProvider, engine};
        use crate::provider::{RuntimeProvider, Script};
        use crate::worker::WorkerResult;
        use boa_engine::Source;
        use std::time::{Duration, Instant};

        /// 经完整 provider 路径执行：校验检查点四要素与引擎指纹（信号链完备）。
        fn run_with_timeout(script: &str, input: &str, timeout: Duration) -> WorkerResult {
            let checkpoint = JsProvider::default()
                .execute(&Script {
                    script: script.to_string(),
                    input: input.to_string(),
                    timeout,
                })
                .expect("引擎路径必须返回检查点");
            crate::checkpoint::validate(&checkpoint).expect("检查点四要素齐备");
            assert!(
                checkpoint.runtime_fingerprint.contains("boa/"),
                "指纹必须记录引擎：{}",
                checkpoint.runtime_fingerprint
            );
            checkpoint.result.expect("引擎路径必须登记结果")
        }

        /// 默认时限执行。
        fn run(script: &str, input: &str) -> WorkerResult {
            run_with_timeout(script, input, Duration::from_secs(30))
        }

        /// 引擎可用性与指纹：feature 开启即可用，指纹含 boa/ 与版本号。
        #[test]
        fn engine_reports_available_with_boa_fingerprint() {
            let js = JsProvider::default();
            assert!(js.is_available(), "feature runtime-js 开启时引擎必须可用");
            let fingerprint = js.env_fingerprint();
            assert!(
                fingerprint.contains("boa/"),
                "指纹须含引擎族：{fingerprint}"
            );
            assert!(
                fingerprint.contains("0.21"),
                "指纹须含版本号：{fingerprint}"
            );
        }

        /// 固定 ECMAScript 测试集（Check 153/V-302）：全部断言求值结果；含中文内容。
        #[test]
        fn fixed_ecmascript_suite_passes() {
            const SUITE: &[(&str, &str, &str)] = &[
                // （用例名，脚本，期望 stdout）
                ("算术", "2 + 3 * 4", "14"),
                ("字符串拼接", "'受限' + '引擎'", "受限引擎"),
                (
                    "数组与对象字面量",
                    "const point = { x: 10, y: [1, 2] }; point.y[1] + point.x",
                    "12",
                ),
                ("模板串", "`和为 ${6 * 7}`", "和为 42"),
                ("箭头函数", "const double = (n) => n * 2; double(21)", "42"),
                ("块作用域", "let outer = 1; { let outer = 2; } outer", "1"),
                (
                    "JSON 往返",
                    r#"JSON.stringify(JSON.parse('{"名": "值"}'))"#,
                    r#"{"名":"值"}"#,
                ),
                (
                    "中文内容与标识符",
                    "const 问候 = '你好'; 问候 + '，世界'",
                    "你好，世界",
                ),
                (
                    "异常捕获",
                    "try { throw new Error('内部'); } catch (error) { '捕获 ' + error.message }",
                    "捕获 内部",
                ),
            ];
            for (name, script, expected) in SUITE {
                let result = run(script, "");
                assert_eq!(
                    result.exit_code,
                    Some(0),
                    "{name} 必须成功：{:?}",
                    result.stderr_tail
                );
                assert_eq!(result.stdout_tail, *expected, "{name} 求值结果必须一致");
                assert!(result.stderr_tail.is_empty(), "{name} 成功不得有 stderr");
            }
        }

        /// Check 109：Node API 拒绝面——调用即脚本级失败，错误消息含“能力不足”与
        /// 运行时名 js；不崩溃、不静默产出结果。
        #[test]
        fn node_api_rejections_fail_with_capability_error() {
            for (name, _, _) in engine::rejection_surface() {
                let script = format!("{name}();");
                let result = run(&script, "");
                assert_eq!(
                    result.exit_code,
                    Some(1),
                    "{name} 调用必须脚本级失败：{result:?}"
                );
                assert!(result.stdout_tail.is_empty(), "{name} 不得静默产出 stdout");
                assert!(
                    result.stderr_tail.contains("能力不足"),
                    "{name} 错误消息必须含“能力不足”：{}",
                    result.stderr_tail
                );
                assert!(
                    result.stderr_tail.contains("js"),
                    "{name} 错误消息必须含运行时名 js：{}",
                    result.stderr_tail
                );
                assert!(
                    result.stderr_tail.contains(name),
                    "{name} 错误消息必须含 API 名：{}",
                    result.stderr_tail
                );
            }
        }

        /// `script.input` 以冻结全局字符串 `input` 注入：可读、严格模式赋值失败、
        /// 非严格模式赋值不改值。
        #[test]
        fn input_is_injected_as_frozen_global_string() {
            let read = run("input", "你好，受限输入");
            assert_eq!(read.exit_code, Some(0), "读取 input 必须成功");
            assert_eq!(read.stdout_tail, "你好，受限输入");
            let strict = run("'use strict'; input = 1;", "");
            assert_eq!(strict.exit_code, Some(1), "严格模式冻结赋值必须失败");
            assert!(!strict.stderr_tail.is_empty(), "失败必须有 stderr 消息");
            let sloppy = run("input = 42; input", "原值");
            assert_eq!(sloppy.exit_code, Some(0));
            assert_eq!(sloppy.stdout_tail, "原值", "非严格赋值不得改写冻结值");
        }

        /// 有界解释器（NFR-215）：死循环在固定操作数内结构化失败，不挂起、不崩溃。
        #[test]
        fn runaway_loop_is_bounded_and_fails_structured() {
            let started = Instant::now();
            let result = run("let sum = 0; while (true) { sum += 1; }", "");
            let elapsed = started.elapsed();
            assert_eq!(result.exit_code, Some(1), "死循环必须结构化失败而非挂起");
            assert!(
                result.stderr_tail.contains("RuntimeLimit"),
                "超出循环上限须报 RuntimeLimit：{}",
                result.stderr_tail
            );
            assert!(
                elapsed < Duration::from_secs(20),
                "有界解释器必须及时返回（实际 {elapsed:?}）"
            );
        }

        /// 墙钟超限按超时报告（与 worker.rs 子进程超时同语义：exit_code None + timed_out，
        /// 且与取消可区分）。
        #[test]
        fn wall_clock_overrun_reports_timed_out() {
            let result = run_with_timeout("1 + 1", "", Duration::ZERO);
            assert!(result.timed_out, "墙钟超限必须报告超时");
            assert_eq!(result.exit_code, None);
            assert!(!result.cancelled, "超时不得混同为取消");
        }

        /// 未捕获异常按退出码 1 传播到 stderr 尾部（不崩溃、不静默）。
        #[test]
        fn uncaught_exception_propagates_as_failure() {
            let result = run("throw new Error('boom-异常');", "");
            assert_eq!(result.exit_code, Some(1));
            assert!(result.stdout_tail.is_empty());
            assert!(
                result.stderr_tail.contains("boom-异常"),
                "异常消息须进 stderr 尾部：{}",
                result.stderr_tail
            );
        }

        /// ADR-029 基准（实验性，默认忽略）：同一脚本连续 1000 次求值的吞吐与进程 RSS。
        /// 运行方式：
        /// `cargo test -p whcode-runtime --lib --features runtime-js -- --ignored boa_benchmark_throughput_and_rss --nocapture`
        /// 说明：同一 Context 重复求值（脚本顶层无词法声明，可重复执行）；RSS 经
        /// `whcode_platform::resources::current_rss_bytes()` 读取（Windows tasklist）。
        #[test]
        #[ignore = "基准按需运行：cargo test -p whcode-runtime --lib --features runtime-js -- --ignored boa_benchmark_throughput_and_rss --nocapture"]
        fn boa_benchmark_throughput_and_rss() {
            const RUNS: usize = 1000;
            let script = "(function () { const values = [1, 2, 3, 4, 5]; let sum = 0; for (const value of values) { sum += value * 2; } return `合计 ${sum} 次`; })()";
            let mut context = engine::new_context("基准输入").expect("上下文装配");
            for _ in 0..10 {
                context.eval(Source::from_bytes(script)).expect("预热求值");
            }
            let started = Instant::now();
            let mut last = String::new();
            for _ in 0..RUNS {
                let value = context.eval(Source::from_bytes(script)).expect("求值");
                last = value
                    .to_string(&mut context)
                    .expect("结果转换")
                    .to_std_string_escaped();
            }
            let elapsed = started.elapsed();
            let seconds = elapsed.as_secs_f64();
            let per_eval_ms = seconds * 1000.0 / RUNS as f64;
            let throughput = RUNS as f64 / seconds;
            println!(
                "boa 基准：{RUNS} 次求值 | 总耗时 {elapsed:?} | 单次 {per_eval_ms:.3} ms | 吞吐 {throughput:.1} eval/s"
            );
            println!("结果快照：{last}");
            match whcode_platform::resources::current_rss_bytes() {
                Ok(bytes) => println!(
                    "进程 RSS：{bytes} 字节（{:.1} MiB）",
                    bytes as f64 / (1024.0 * 1024.0)
                ),
                Err(error) => println!("RSS 读取失败（不作为基准通过条件）：{error}"),
            }
            assert_eq!(
                last, "合计 30 次",
                "固定脚本结果必须稳定（1+…+5=15，×2=30）"
            );
        }
    }
}
