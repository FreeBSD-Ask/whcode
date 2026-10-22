//! # Constitution 创建/导入、版本记录与变更审计（T070、FR-301）
//!
//! 初始化工作区时创建或导入 Constitution，记录版本与变更审计（FR-301）。
//! 变更审计经 domain [`whcode_domain::AuditSink`]（`AuditAction::ConstitutionChange`，
//! §10.3 强制清单）——**审计失败时变更不得视为成功**（AuditSink 契约）。
//!
//! 对"应该可用"类不可验证条款，登记为 `mandatory`（强制约束，V-201 门禁依据）；
//! 门禁校验属 [`crate::analyze`] 与应用服务（T074）。

use whcode_domain::{AuditAction, AuditRecord, AuditSink, WhError};

/// Constitution 版本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Constitution {
    /// 版本号（每次变更递增）。
    pub version: u32,
    /// 条款（稳定 ID + 文本 + 是否强制）。
    pub articles: Vec<ConstitutionArticle>,
}

/// Constitution 条款。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstitutionArticle {
    /// 稳定 ID（`ART-001`）。
    pub id: String,
    /// 条款文本。
    pub text: String,
    /// 是否强制约束（V-201 门禁校验的对象）。
    pub mandatory: bool,
}

impl Constitution {
    /// 创建 Constitution（版本 1）。
    ///
    /// # Errors
    ///
    /// 条款 ID 重复或文本为空时返回 [`WhError`]。
    pub fn create(articles: Vec<ConstitutionArticle>) -> Result<Self, WhError> {
        validate_articles(&articles)?;
        Ok(Self {
            version: 1,
            articles,
        })
    }

    /// 导入 Constitution（指定版本；导入也产生审计记录，FR-301）。
    ///
    /// # Errors
    ///
    /// 条款非法时返回 [`WhError`]。
    pub fn import(version: u32, articles: Vec<ConstitutionArticle>) -> Result<Self, WhError> {
        validate_articles(&articles)?;
        if version == 0 {
            return Err(WhError::invalid_state("Constitution 版本必须 ≥ 1"));
        }
        Ok(Self { version, articles })
    }

    /// 修改变更（条款替换），版本递增并写 `constitution_change` 审计（FR-301）。
    ///
    /// **审计失败时变更不得视为成功**（`AuditSink` 契约）：审计错误原样传播。
    ///
    /// # Errors
    ///
    /// 条款非法、审计写入失败时返回 [`WhError`]；此时 `self` 不被修改。
    pub fn change(
        &mut self,
        articles: Vec<ConstitutionArticle>,
        sink: &dyn AuditSink,
        actor: &str,
        reason: &str,
        now_ms: u64,
    ) -> Result<(), WhError> {
        validate_articles(&articles)?;
        let record = AuditRecord {
            id: format!("A-constitution-{}-{now_ms}", self.version + 1),
            actor: actor.to_string(),
            action: AuditAction::ConstitutionChange,
            target: format!("constitution v{} → v{}", self.version, self.version + 1),
            detail: serde_json::json!({
                "reason": reason,
                "articles": articles.len(),
            }),
            ts_ms: now_ms,
        };
        sink.record(&record)?;
        self.version += 1;
        self.articles = articles;
        Ok(())
    }

    /// 强制条款 ID（V-201 门禁校验依据）。
    #[must_use]
    pub fn mandatory_article_ids(&self) -> Vec<String> {
        self.articles
            .iter()
            .filter(|article| article.mandatory)
            .map(|article| article.id.clone())
            .collect()
    }
}

fn validate_articles(articles: &[ConstitutionArticle]) -> Result<(), WhError> {
    if articles.is_empty() {
        return Err(WhError::invalid_state("Constitution 至少一条条款"));
    }
    let mut seen = std::collections::BTreeSet::new();
    for article in articles {
        if article.id.trim().is_empty() || article.text.trim().is_empty() {
            return Err(WhError::invalid_state("条款 ID 与文本不得为空"));
        }
        if !seen.insert(article.id.clone()) {
            return Err(WhError::invalid_state(format!(
                "条款 ID 重复：{}（FR-309 同源语义：不静默接受重复）",
                article.id
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_domain::{AuditAction, CollectingAuditSink};

    fn article(id: &str, text: &str, mandatory: bool) -> ConstitutionArticle {
        ConstitutionArticle {
            id: id.to_string(),
            text: text.to_string(),
            mandatory,
        }
    }

    #[test]
    fn create_import_and_version_records() {
        let constitution = Constitution::create(vec![
            article("ART-001", "不伪造证据", true),
            article("ART-002", "默认回环监听", false),
        ])
        .expect("创建");
        assert_eq!(constitution.version, 1);
        assert_eq!(constitution.mandatory_article_ids(), vec!["ART-001"]);

        let imported =
            Constitution::import(3, vec![article("ART-001", "导入", true)]).expect("导入");
        assert_eq!(imported.version, 3);
        assert!(Constitution::import(0, vec![article("A", "x", true)]).is_err());
        assert!(Constitution::create(vec![]).is_err());
    }

    struct FailingSink;
    impl AuditSink for FailingSink {
        fn record(&self, _record: &AuditRecord) -> Result<(), WhError> {
            Err(WhError::internal("审计写入失败"))
        }
    }

    #[test]
    fn change_audits_and_failing_audit_prevents_change() {
        let mut constitution =
            Constitution::create(vec![article("ART-001", "v1", true)]).expect("创建");
        let sink = CollectingAuditSink::new();

        constitution
            .change(
                vec![
                    article("ART-001", "v2", true),
                    article("ART-002", "新增", false),
                ],
                &sink,
                "tester",
                "加入新约束",
                1_000,
            )
            .expect("变更");
        assert_eq!(constitution.version, 2);
        assert_eq!(sink.len(), 1);
        let record = &sink.records()[0];
        assert_eq!(record.action, AuditAction::ConstitutionChange);
        assert_eq!(record.detail["reason"], "加入新约束");

        // 审计失败 → 变更不生效（AuditSink 契约：审计失败不得视为成功）。
        let version_before = constitution.version;
        let err = constitution
            .change(
                vec![article("ART-001", "v3", true)],
                &FailingSink,
                "tester",
                "x",
                2_000,
            )
            .expect_err("审计失败必须传播");
        assert!(err.to_string().contains("审计"));
        assert_eq!(constitution.version, version_before, "变更未生效");
    }

    #[test]
    fn duplicate_article_ids_are_rejected() {
        let err = Constitution::create(vec![
            article("ART-001", "一", true),
            article("ART-001", "重复", true),
        ])
        .expect_err("重复 ID 必须拒绝");
        assert!(err.to_string().contains("重复"));
    }
}
