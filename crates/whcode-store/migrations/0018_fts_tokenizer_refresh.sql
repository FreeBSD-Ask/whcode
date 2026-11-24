-- schema v18：强制 FTS 索引以 opensearch_standard tokenizer 重建（ADR-099 修订）。
--
-- 背景：默认 unicode tokenizer 按空白切分，中文无空格长句整体成一个 token，
-- 导致“防火墙”等中文子串 0 命中（T422 实测更正）。修订后建索引显式指定
-- tokenizer='opensearch_standard'，中英文子串均可命中。
--
-- 旧库若已有 fts 索引与时效元数据，ensure_fts_index 会据 meta 判定“索引新鲜”而
-- 跳过重建，继续使用旧 tokenizer。本迁移删除 meta 行，使其判定为陈旧并重建。
-- 不删 fts 索引本身——create_fts_index(..., overwrite=1) 会覆盖；不动表结构。
DELETE FROM fts_index_meta WHERE id = 1;
