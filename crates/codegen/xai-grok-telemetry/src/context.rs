//! Git context collection for telemetry events.

pub struct GitContext {
    pub is_git_repo: bool,
}

pub fn collect_git_context(cwd: &str) -> GitContext {
    // [LOCAL-DEV] git2 (vendored libgit2) removed for build speed: a `.git`
    // presence walk is equivalent for the is-git-repo event attribute.
    use std::path::Path;
    let mut dir = Some(Path::new(cwd));
    while let Some(d) = dir {
        if d.join(".git").exists() {
            return GitContext { is_git_repo: true };
        }
        dir = d.parent();
    }
    GitContext { is_git_repo: false }
}
