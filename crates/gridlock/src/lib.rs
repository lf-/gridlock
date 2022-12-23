static REPO_QUERY: &'static str = r#"
query ($name: String = "", $owner: String = "", $explicitBranch: Boolean = false, $branch: String = "") {
  repository(name: $name, owner: $owner) {
    defaultBranchRef {
      name
      target {
        oid
      }
    }
    ref(qualifiedName: $branch) @include(if: $explicitBranch) {
      name
      target {
        oid
      }
    }
  }
}
"#;
