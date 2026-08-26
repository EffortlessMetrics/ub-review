from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label} count={count}")
    return text.replace(old, new, 1)


claim_graph = Path("src/claim_graph.rs")
text = claim_graph.read_text()
old_literal = '''            Some(&crate::RevisionRef {
                digest: "c".repeat(64),
                semantics: "merge_result".to_owned(),
                reviewed_commit: "d".repeat(64),
            }),
'''
new_literal = '''            Some(&crate::RevisionRef {
                digest: "c".repeat(64),
                semantics: "merge_result".to_owned(),
                base_commit: "a".repeat(64),
                head_commit: "b".repeat(64),
                reviewed_commit: "d".repeat(64),
            }),
'''
claim_graph.write_text(
    replace_once(text, old_literal, new_literal, "row-stamp revision literal")
)

admission = Path("src/revision_admission.rs")
text = admission.read_text()
old_validation = '''        let stored_objects = [
            self.base_commit_oid.as_str(),
            self.head_commit_oid.as_str(),
            self.reviewed_commit_oid.as_str(),
        ];
        if stored_objects.iter().all(|value| value.is_empty()) {
            return Ok(());
        }
        if self.base_commit_oid != parsed.base_commit_oid()
            || self.head_commit_oid != parsed.head_commit_oid()
            || self.reviewed_commit_oid != parsed.reviewed_commit_oid()
            || self.semantics != parsed.semantics_key()
        {
            bail!("revision admission object fields do not match its canonical identity");
        }
        if self
            .pr_head_commit
            .as_deref()
            .is_some_and(|commit| commit != parsed.head_commit_oid())
        {
            bail!("revision admission pull-request head does not match its canonical identity");
        }
        Ok(())
'''
new_validation = '''        if self.semantics != parsed.semantics_key() {
            bail!("revision admission semantics do not match its canonical identity");
        }
        if self
            .pr_head_commit
            .as_deref()
            .is_some_and(|commit| commit != parsed.head_commit_oid())
        {
            bail!("revision admission pull-request head does not match its canonical identity");
        }
        let stored_objects = [
            self.base_commit_oid.as_str(),
            self.head_commit_oid.as_str(),
            self.reviewed_commit_oid.as_str(),
        ];
        if stored_objects.iter().all(|value| value.is_empty()) {
            return Ok(());
        }
        if self.base_commit_oid != parsed.base_commit_oid()
            || self.head_commit_oid != parsed.head_commit_oid()
            || self.reviewed_commit_oid != parsed.reviewed_commit_oid()
        {
            bail!("revision admission object fields do not match its canonical identity");
        }
        Ok(())
'''
text = replace_once(
    text,
    old_validation,
    new_validation,
    "admission validation target",
)

marker = '''    #[test]
    fn revision_ref_joins_admission_and_validates_shape() -> Result<()> {
'''
regression = '''    #[test]
    fn legacy_admission_fields_cannot_override_canonical_semantics_or_pr_head() -> Result<()> {
        let repo = init_repo()?;
        let (base_tip, pr_head) = divergent_commits(&repo)?;
        let merge = synthetic_merge(&repo, &base_tip, &pr_head)?;
        let admission = admit_revision(
            repo.root(),
            "main",
            &merge,
            Some(&pr_head),
            &files_vec(),
            &sample_patch(),
        )?;

        let mut legacy = admission.clone();
        legacy.base_commit_oid.clear();
        legacy.head_commit_oid.clear();
        legacy.reviewed_commit_oid.clear();
        legacy.validate()?;

        let mut wrong_semantics = legacy.clone();
        wrong_semantics.semantics = "candidate_head".to_owned();
        let Err(semantics_error) = wrong_semantics.validate() else {
            bail!("legacy compatibility cannot override canonical semantics");
        };
        assert!(
            semantics_error.to_string().contains("semantics do not match"),
            "{semantics_error}"
        );

        let mut wrong_head = legacy;
        wrong_head.pr_head_commit = Some("f".repeat(40));
        let Err(head_error) = wrong_head.validate() else {
            bail!("legacy compatibility cannot override canonical PR head");
        };
        assert!(
            head_error
                .to_string()
                .contains("pull-request head does not match"),
            "{head_error}"
        );
        Ok(())
    }

    #[test]
    fn revision_ref_joins_admission_and_validates_shape() -> Result<()> {
'''
admission.write_text(
    replace_once(text, marker, regression, "regression insertion target")
)
