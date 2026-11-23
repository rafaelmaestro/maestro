use std::{fs, path::Path, process::Command};

/// Um arquivo em "Mudanças": tudo que a branch mudou desde a base, commitado ou não.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub path: String,
    pub added: u32,
    pub removed: u32,
    pub new: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Changes {
    pub base: String,
    pub files: Vec<Change>,
    pub uncommitted: usize,
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    // leitura de fundo não pega o index.lock: um rebase ou commit no terminal ao mesmo tempo não quebra
    let out = Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn changes(root: &Path) -> Option<Changes> {
    let base = crate::diff::base(root)?;
    let fork = git(root, &["merge-base", "HEAD", &base])?;
    let mut files = parse_numstat(&git(root, &["diff", "--numstat", fork.trim()])?);
    let created = git(
        root,
        &["diff", "--name-only", "--diff-filter=A", fork.trim()],
    )?;
    for f in &mut files {
        f.new = created.lines().any(|l| l == f.path);
    }
    let lines = |p: &str| fs::read_to_string(root.join(p)).map_or(0, |t| t.lines().count() as u32);
    for path in git(
        root,
        &[
            "ls-files",
            "--others",
            "--exclude-standard",
            "--directory",
            "--no-empty-directory",
        ],
    )?
    .lines()
    {
        let added = if path.ends_with('/') {
            git(
                root,
                &["ls-files", "--others", "--exclude-standard", "--", path],
            )
            .unwrap_or_default()
            .lines()
            .map(lines)
            .sum()
        } else {
            lines(path)
        };
        files.push(Change {
            path: path.into(),
            added,
            removed: 0,
            new: true,
        });
    }
    let uncommitted = git(root, &["status", "--porcelain"])?.lines().count();
    Some(Changes {
        base,
        files,
        uncommitted,
    })
}

/// `12\t3\tsrc/a.rs`; binário vem como `-\t-`.
fn parse_numstat(text: &str) -> Vec<Change> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.splitn(3, '\t');
            let added = it.next()?.parse().unwrap_or(0);
            let removed = it.next()?.parse().unwrap_or(0);
            Some(Change {
                path: it.next()?.into(),
                added,
                removed,
                new: false,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_o_numstat() {
        let c = parse_numstat("8\t0\tsrc/health.ts\n2\t1\tsrc/app.ts\n-\t-\timg.png\n");
        assert_eq!(c.len(), 3);
        assert_eq!(
            (c[0].added, c[0].removed, c[0].path.as_str()),
            (8, 0, "src/health.ts")
        );
        assert_eq!((c[2].added, c[2].removed), (0, 0));
    }

    #[test]
    fn pasta_nova_vira_uma_linha() {
        let root = std::env::temp_dir().join(format!("maestro-ch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        let g = |a: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&root)
                    .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                    .args(a)
                    .output()
                    .unwrap()
                    .status
                    .success()
            )
        };
        g(&["init", "-q", "-b", "main"]);
        fs::write(root.join("src/a.ts"), "a\n").unwrap();
        g(&["add", "."]);
        g(&["commit", "-qm", "x"]);
        fs::write(root.join("src/a.ts"), "a\nb\n").unwrap();
        fs::create_dir_all(root.join("docs/adr/sub")).unwrap();
        fs::write(root.join("docs/adr/1.md"), "1\n2\n").unwrap();
        fs::write(root.join("docs/adr/2.md"), "1\n").unwrap();
        fs::write(root.join("docs/adr/sub/3.md"), "1\n").unwrap();
        fs::write(root.join("solto.md"), "1\n").unwrap();
        let c = changes(&root).unwrap();
        let got: Vec<_> = c
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.added, f.new))
            .collect();
        assert_eq!(
            got,
            [
                ("src/a.ts", 1, false),
                ("docs/", 4, true),
                ("solto.md", 1, true)
            ]
        );
        fs::remove_dir_all(&root).unwrap();
    }
}
