# 规格说明（SDD 夹具——含违规产物）

## Requirements

- **REQ-001**: 会话可创建
- **REQ-002**: 覆盖不足的需求
- **REQ-003**: 遗漏的需求

## Tasks

- [ ] T-001: 孤儿任务，无任何引用
- [ ] T-002: 无效引用任务 (refs: REQ-404) (depends: T-003)
- [ ] T-003: 循环依赖任务 (refs: REQ-001) (depends: T-002) (acceptance: 断言)

## Notes

- **REQ-009**: 未识别段落的条目（解析器不得静默丢弃）
