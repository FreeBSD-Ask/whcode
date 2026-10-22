# tests/unit

跨模块单元测试（tasks.md T008）。

约定：crate 内部单元测试就近放在 `crates/<crate>/src/**`（`#[cfg(test)] mod tests`）；
本目录用于**跨模块但不跨进程**的单元测试，或需要多个 crate 内部类型协作的场景。

当前状态：目录已建立，随 M1 起按需落地。
