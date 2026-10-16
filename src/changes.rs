use std::{
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

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

/// A aba "Arquivos": o que o git conhece na worktree, sem os ignorados.
pub fn files(root: &Path) -> Vec<String> {
    git(
        root,
        &["ls-files", "--cached", "--others", "--exclude-standard"],
    )
    .map(|o| o.lines().map(String::from).collect::<Vec<_>>())
    .map(|mut v| {
        // pasta vazia não entra no ls-files; `--directory` lista como `pasta/` e a árvore mostra
        let dirs = git(
            root,
            &["ls-files", "--others", "--exclude-standard", "--directory"],
        )
        .unwrap_or_default();
        v.extend(
            dirs.lines()
                .filter(|d| d.ends_with('/'))
                .filter(|d| fs::read_dir(root.join(d)).is_ok_and(|mut r| r.next().is_none()))
                .map(String::from),
        );
        v
    })
    .unwrap_or_default()
}

/// Caminho digitado na árvore: relativo, sem `..` nem `.`, para não sair da worktree.
fn inside(rel: &str) -> Result<&str, String> {
    let rel = rel.trim().trim_end_matches('/');
    let ok = !rel.is_empty()
        && Path::new(rel)
            .components()
            .all(|c| matches!(c, Component::Normal(_)));
    ok.then_some(rel)
        .ok_or_else(|| format!("Nome inválido: {rel:?} (relativo à worktree, sem ..)"))
}

fn free(root: &Path, rel: &str) -> Result<(String, PathBuf), String> {
    let rel = inside(rel)?;
    let abs = root.join(rel);
    if abs.symlink_metadata().is_ok() {
        return Err(format!("{rel} já existe"));
    }
    if let Some(p) = abs.parent() {
        fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    Ok((rel.into(), abs))
}

/// Novo arquivo ou pasta na worktree; `a/b.ts` cria `a/`. Devolve o caminho limpo.
pub fn create(root: &Path, rel: &str, dir: bool) -> Result<String, String> {
    let (rel, abs) = free(root, rel)?;
    if dir {
        fs::create_dir(&abs)
    } else {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&abs)
            .map(drop)
    }
    .map_err(|e| e.to_string())?;
    Ok(rel)
}

pub fn rename(root: &Path, from: &str, to: &str) -> Result<String, String> {
    let from_rel = inside(from)?;
    if inside(to)?.starts_with(&format!("{from_rel}/")) {
        return Err(format!("{from_rel} não cabe dentro dela mesma"));
    }
    let from = root.join(from_rel);
    let (rel, abs) = free(root, to)?;
    fs::rename(from, abs).map_err(|e| e.to_string())?;
    Ok(rel)
}

/// `app.ts` já existe em `dir`: `app cópia.ts`, depois `app cópia 2.ts`.
fn free_name(dir: &str, name: &str, taken: impl Fn(&str) -> bool) -> String {
    let (base, ext) = match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    };
    let join = |n: String| {
        if dir.is_empty() {
            n
        } else {
            format!("{dir}/{n}")
        }
    };
    let mut path = join(name.into());
    let mut k = 1;
    while taken(&path) {
        path = join(if k == 1 {
            format!("{base} cópia{ext}")
        } else {
            format!("{base} cópia {k}{ext}")
        });
        k += 1;
    }
    path
}

/// Copia o arquivo ou a pasta para dentro de `dir` ("" é a raiz), sem sobrescrever nem seguir links.
pub fn copy(root: &Path, from: &str, dir: &str) -> Result<String, String> {
    let from = inside(from)?;
    let dir = if dir.trim().is_empty() {
        ""
    } else {
        inside(dir)?
    };
    if dir == from || dir.starts_with(&format!("{from}/")) {
        return Err(format!("{from} não cabe dentro dela mesma"));
    }
    let src = root.join(from);
    src.symlink_metadata()
        .map_err(|_| format!("{from} não existe mais"))?;
    let name = from.rsplit('/').next().unwrap_or(from);
    let to = free_name(dir, name, |p| root.join(p).symlink_metadata().is_ok());
    fn walk(a: &Path, b: &Path) -> std::io::Result<()> {
        let t = a.symlink_metadata()?.file_type();
        if t.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(a)?, b)
        } else if t.is_dir() {
            fs::create_dir(b)?;
            for e in fs::read_dir(a)? {
                let e = e?;
                walk(&e.path(), &b.join(e.file_name()))?;
            }
            Ok(())
        } else {
            fs::copy(a, b).map(drop)
        }
    }
    let (rel, abs) = free(root, &to)?;
    walk(&src, &abs).map_err(|e| e.to_string())?;
    Ok(rel)
}

pub fn remove(root: &Path, rel: &str) -> Result<(), String> {
    let abs = root.join(inside(rel)?);
    if abs.is_dir() {
        fs::remove_dir_all(abs)
    } else {
        fs::remove_file(abs)
    }
    .map_err(|e| e.to_string())
}

#[derive(Default)]
struct Dir {
    dirs: std::collections::BTreeMap<String, Dir>,
    files: Vec<String>,
}

/// As linhas visíveis da árvore do repo: (profundidade, nome, caminho, é pasta).
/// Pastas antes dos arquivos; só desce nas pastas de `open`.
pub fn tree(
    files: &[String],
    open: &std::collections::HashSet<String>,
) -> Vec<(usize, String, String, bool)> {
    let mut root = Dir::default();
    for f in files {
        let mut d = &mut root;
        let mut parts = f.split('/').peekable();
        while let Some(p) = parts.next() {
            if parts.peek().is_none() {
                if !p.is_empty() {
                    d.files.push(p.to_string());
                }
            } else {
                d = d.dirs.entry(p.to_string()).or_default();
            }
        }
    }
    fn walk(
        d: &Dir,
        prefix: &str,
        depth: usize,
        open: &std::collections::HashSet<String>,
        out: &mut Vec<(usize, String, String, bool)>,
    ) {
        for (name, sub) in &d.dirs {
            let path = format!("{prefix}{name}");
            out.push((depth, name.clone(), path.clone(), true));
            if open.contains(&path) {
                walk(sub, &format!("{path}/"), depth + 1, open, out);
            }
        }
        let mut files = d.files.clone();
        files.sort();
        for f in files {
            out.push((depth, f.clone(), format!("{prefix}{f}"), false));
        }
    }
    let mut out = Vec::new();
    walk(&root, "", 0, open, &mut out);
    out
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
    for path in git(root, &["ls-files", "--others", "--exclude-standard"])?.lines() {
        let lines = std::fs::read_to_string(root.join(path)).map_or(0, |t| t.lines().count());
        files.push(Change {
            path: path.into(),
            added: lines as u32,
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
    fn mexe_so_dentro_da_worktree() {
        let root = std::env::temp_dir().join(format!("maestro-fs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        for bad in ["", "../fora.ts", "/etc/x", "a/../../b", "./a"] {
            assert!(create(&root, bad, false).is_err(), "{bad}");
        }
        assert_eq!(create(&root, " a/b.ts ", false).unwrap(), "a/b.ts");
        assert!(root.join("a/b.ts").is_file());
        assert!(
            create(&root, "a/b.ts", false)
                .unwrap_err()
                .contains("já existe")
        );
        assert_eq!(create(&root, "vazia/", true).unwrap(), "vazia");
        assert_eq!(rename(&root, "a/b.ts", "c/d.ts").unwrap(), "c/d.ts");
        assert!(root.join("c/d.ts").is_file() && !root.join("a/b.ts").exists());
        assert!(rename(&root, "c/d.ts", "vazia").is_err());
        assert!(remove(&root, "../x").is_err());
        remove(&root, "c").unwrap();
        assert!(!root.join("c").exists());
        assert_eq!(
            super::tree(&["vazia/".into()], &Default::default()).len(),
            1
        );
        create(&root, "p/q/a.ts", false).unwrap();
        assert!(rename(&root, "p", "p/q/p").unwrap_err().contains("dentro"));
        assert!(copy(&root, "p", "p/q").unwrap_err().contains("dentro"));
        assert_eq!(copy(&root, "p/q/a.ts", "p/q").unwrap(), "p/q/a cópia.ts");
        assert_eq!(copy(&root, "p/q/a.ts", "p/q").unwrap(), "p/q/a cópia 2.ts");
        std::os::unix::fs::symlink("/etc", root.join("p/link")).unwrap();
        assert_eq!(copy(&root, "p", "").unwrap(), "p cópia");
        assert!(root.join("p cópia/q/a cópia 2.ts").is_file());
        let link = root.join("p cópia/link").symlink_metadata().unwrap();
        assert!(link.file_type().is_symlink());
        assert!(copy(&root, "sumiu.ts", "vazia").is_err());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn nome_livre_para_colar() {
        let taken = ["src/app.ts", "src/app cópia.ts", "Makefile", ".env"];
        let t = |p: &str| taken.contains(&p);
        assert_eq!(free_name("src", "app.ts", t), "src/app cópia 2.ts");
        assert_eq!(free_name("", "Makefile", t), "Makefile cópia");
        assert_eq!(free_name("", ".env", t), ".env cópia");
        assert_eq!(free_name("lib", "app.ts", t), "lib/app.ts");
    }

    #[test]
    fn tree_opens_only_expanded_dirs() {
        let files: Vec<String> = [
            "src/b.rs",
            "README.md",
            "src/a/x.rs",
            "Cargo.toml",
            "src/main.rs",
        ]
        .map(String::from)
        .to_vec();
        let rows = |open: &[&str]| {
            super::tree(&files, &open.iter().map(|s| s.to_string()).collect())
                .into_iter()
                .map(|(d, n, _, dir)| format!("{d}{}{n}", if dir { "/" } else { " " }))
                .collect::<Vec<_>>()
        };
        assert_eq!(rows(&[]), ["0/src", "0 Cargo.toml", "0 README.md"]);
        assert_eq!(
            rows(&["src", "src/a"]),
            [
                "0/src",
                "1/a",
                "2 x.rs",
                "1 b.rs",
                "1 main.rs",
                "0 Cargo.toml",
                "0 README.md"
            ]
        );
    }
}
