use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::herdr::Herdr;

/// Tudo que a feature criou, para a conclusão e a Limpeza apagarem exatamente isso e nada mais.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Inventory {
    pub id: String,
    pub repo: PathBuf,
    pub branch: String,
    pub base: String,
    pub worktrees: Vec<Worktree>,
    pub port: u16,
    pub compose_project: String,
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    pub created_at: u64,
    /// MR empilhado: a feature mãe e o ponto dela de onde esta saiu
    #[serde(default)]
    pub parent: Option<Parent>,
    #[serde(default)]
    pub jira: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Parent {
    pub id: String,
    pub branch: String,
    /// a ponta da mãe na criação ou no último rebase: o `--onto` corta daqui
    pub sha: String,
    /// a base da mãe (`main`): para onde a filha vai quando a mãe mergeia
    pub base: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Worktree {
    pub repo: PathBuf,
    pub path: PathBuf,
    pub branch: String,
    /// a branch foi criada pela feature (a conclusão pode apagá-la se estiver mergeada)
    pub created_branch: bool,
}

pub struct Request {
    pub repo: PathBuf,
    pub branch: String,
    pub base: String,
    pub prompt: String,
    pub copy_env: bool,
    pub setup: bool,
    pub claude: bool,
    pub model: Option<&'static str>,
    pub group: Option<String>,
    /// "Outros repos": mesma branch numa worktree de cada
    pub others: Vec<PathBuf>,
    /// "Em cima de": a mãe; `base` já vem com a branch dela
    pub parent: Option<Inventory>,
    pub jira: Option<String>,
}

pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        // sem acento, como o `normalize('NFD')` do protótipo
        let c = match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' | 'Á' | 'À' | 'Â' | 'Ã' => 'a',
            'é' | 'ê' | 'è' | 'É' | 'Ê' => 'e',
            'í' | 'ì' | 'Í' => 'i',
            'ó' | 'ô' | 'õ' | 'ò' | 'Ó' | 'Ô' | 'Õ' => 'o',
            'ú' | 'ü' | 'ù' | 'Ú' => 'u',
            'ç' | 'Ç' => 'c',
            c => c,
        };
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// `~/repos/bots-api` + `feat/x` → `~/repos/bots-api.wt/feat-x`, como no protótipo.
pub fn worktree_path(repo: &Path, branch: &str) -> PathBuf {
    let name = repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    repo.with_file_name(format!("{name}.wt")).join(slug(branch))
}

pub fn compose_project(repo: &Path, branch: &str) -> String {
    let name = repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("{}-{}", slug(&name), slug(branch))
}

fn data_dir() -> PathBuf {
    std::env::var_os("MAESTRO_FEATURES").map_or_else(
        || {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                .join(".local/share/maestro/features")
        },
        PathBuf::from,
    )
}

pub fn inventories() -> Vec<Inventory> {
    let Ok(dir) = std::fs::read_dir(data_dir()) else {
        return Vec::new();
    };
    dir.flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            let text = std::fs::read_to_string(e.path()).ok()?;
            serde_json::from_str(&text)
                .inspect_err(|err| eprintln!("maestro: inventário inválido {:?}: {err}", e.path()))
                .ok()
        })
        .collect()
}

/// Temporário + `rename`: um crash no meio não deixa JSON truncado (e a feature órfã).
pub fn save(inv: &Inventory) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir())?;
    let dst = data_dir().join(format!("{}.json", inv.id));
    let tmp = dst.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(inv)?)?;
    std::fs::rename(tmp, dst)
}

/// Vincula (ou desvincula, `None`) o card do Jira à feature.
pub fn set_jira(id: &str, key: Option<String>) -> std::io::Result<()> {
    let mut inv = inventories()
        .into_iter()
        .find(|i| i.id == id)
        .ok_or_else(|| std::io::Error::other("feature sem inventário"))?;
    inv.jira = key;
    save(&inv)
}

/// O bloco que a aba Card lê para propor as sub-tasks (o padrão da skill `/subtasks`).
pub fn subtasks_format(key: &str) -> String {
    format!(
        "um bloco ```subtasks com um array JSON          [{{\"type\":\"Desenvolvimento|Subtarefa|Qualidade e Testes\",\"title\":\"[sistema] o que fazer\",\"sp\":2}}]          com as sub-tasks de {key} no padrão do time: uma por sistema tocado, títulos em minúsculas com o prefixo [sistema],          sempre uma última Qualidade e Testes com o título e2e, SP em Fibonacci (1, 2, 3 ou 5). Não crie nada no Jira."
    )
}

/// A primeira porta acima de 3000 que nenhuma feature usa (3000 fica com a main).
pub fn next_port(used: &[u16]) -> u16 {
    (3001..).find(|p| !used.contains(p)).unwrap_or(3001)
}

/// O setup do repo: `setup = "…"` no `.maestro.toml`, senão pelo lockfile.
pub fn setup_command(repo: &Path) -> Option<String> {
    if let Ok(text) = std::fs::read_to_string(repo.join(".maestro.toml")) {
        let line = text.lines().find(|l| l.trim_start().starts_with("setup"))?;
        let v = line.split_once('=')?.1.trim().trim_matches('"');
        return (!v.is_empty()).then(|| v.to_string());
    }
    let has = |f: &str| repo.join(f).exists();
    Some(
        if has("pnpm-lock.yaml") {
            "pnpm install --frozen-lockfile"
        } else if has("yarn.lock") {
            "yarn install --frozen-lockfile"
        } else if has("package-lock.json") {
            "npm ci"
        } else {
            return None;
        }
        .into(),
    )
}

/// Registry no Artifact Registry: o token do `~/.npmrc` vence e o install toma 403.
/// O token vai para o `~/.npmrc`, nunca para o `.npmrc` versionado da worktree (iria parar no MR).
pub fn registry_auth(repo: &Path) -> Option<&'static str> {
    std::fs::read_to_string(repo.join(".npmrc"))
        .ok()
        .filter(|t| t.contains("pkg.dev"))
        .map(|_| "npx -y google-artifactregistry-auth --repo-config .npmrc --credential-config ~/.npmrc")
}

/// Pastas (com `/` no fim) e depois arquivos do repo, para o `@` do prompt.
pub fn paths(repo: &Path) -> Vec<String> {
    let files = git(
        repo,
        &["ls-files", "--cached", "--others", "--exclude-standard"],
    )
    .unwrap_or_default();
    let mut dirs = std::collections::BTreeSet::new();
    for f in files.lines() {
        let mut d = f;
        while let Some((parent, _)) = d.rsplit_once('/') {
            if !dirs.insert(format!("{parent}/")) {
                break;
            }
            d = parent;
        }
    }
    let mut files: Vec<String> = files
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();
    files.sort();
    dirs.into_iter().chain(files).collect()
}

/// Os de `paths` que contêm `q` (sem caixa), no máximo `n`.
pub fn matching<'a>(paths: &'a [String], q: &str, n: usize) -> Vec<&'a String> {
    let q = q.to_lowercase();
    paths
        .iter()
        .filter(|p| p.to_lowercase().contains(&q))
        .take(n)
        .collect()
}

/// Os repos git em `root` (menos `except`), como `root/nome/`.
pub fn repos(root: &Path, except: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join(".git").exists() && p != except)
        .map(|p| format!("{}/", p.to_string_lossy()))
        .collect();
    out.sort();
    out
}

/// Um `@` com `/` ou `~`: o que há na pasta até a última `/` e começa com o resto (pastas primeiro, sem ocultos).
pub fn disk_matching(q: &str, n: usize) -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let full = match q.strip_prefix('~') {
        Some("") => format!("{home}/"),
        Some(r) => format!("{home}{r}"),
        None => q.to_string(),
    };
    let Some((dir, rest)) = full.rsplit_once('/') else {
        return Vec::new();
    };
    let rest = rest.to_lowercase();
    let mut out: Vec<String> = std::fs::read_dir(if dir.is_empty() { "/" } else { dir })
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let shown = rest.starts_with('.') || !name.starts_with('.');
            (shown && name.to_lowercase().starts_with(&rest)).then(|| {
                let slash = if e.path().is_dir() { "/" } else { "" };
                format!("{dir}/{name}{slash}")
            })
        })
        .collect();
    out.sort_by_key(|p| (!p.ends_with('/'), p.clone()));
    out.truncate(n);
    out
}

/// Os `.env*` da raiz que o git ignora (os versionados já vêm com a worktree).
pub fn env_files(repo: &Path) -> Vec<String> {
    git(
        repo,
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "--",
            ".env*",
        ],
    )
    .unwrap_or_default()
    .lines()
    .filter(|l| !l.contains('/') && !l.is_empty())
    .map(String::from)
    .collect()
}

pub fn branches(repo: &Path) -> Vec<String> {
    git(
        repo,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )
    .unwrap_or_default()
    .lines()
    .map(String::from)
    .collect()
}

/// Bases do seletor "A partir de": as do protótipo que existem no repo.
pub fn bases(repo: &Path) -> Vec<String> {
    let all = branches(repo);
    let mut b: Vec<String> = ["main", "master", "develop"]
        .into_iter()
        .filter(|x| all.iter().any(|y| y == x))
        .map(String::from)
        .collect();
    if b.is_empty() {
        b.extend(all.into_iter().take(1));
    }
    b
}

/// `'…'` para o shell: o prompt vai como um argumento só, com aspas dentro.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    // leitura de fundo não pega o index.lock: um rebase ou commit no terminal ao mesmo tempo não quebra
    let out = Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn git_err(repo: &Path, args: &[&str]) -> Result<(), String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// O comando que roda no pane da feature: setup e depois o `claude` com o prompt.
/// `context`: o arquivo com a nota do repo no vault, que vai no `--append-system-prompt`.
/// `dirs`: as worktrees dos outros repos, que o Claude passa a enxergar.
pub fn start_command(
    req: &Request,
    setup: Option<&str>,
    context: Option<&Path>,
    dirs: &[PathBuf],
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(s) = setup.filter(|_| req.setup) {
        parts.push(s.to_string());
    }
    if req.claude {
        let mut c = String::from("claude --permission-mode plan");
        if let Some(m) = req.model {
            c.push_str(&format!(" --model {m}"));
        }
        for d in dirs {
            c.push_str(&format!(" --add-dir {}", sh_quote(&d.to_string_lossy())));
        }
        if let Some(f) = context {
            c.push_str(&format!(
                " --append-system-prompt \"$(cat {})\"",
                sh_quote(&f.to_string_lossy())
            ));
        }
        if !req.prompt.trim().is_empty() {
            let prompt = match &req.jira {
                Some(k) => format!(
                    "{} No fim do plano, inclua {}",
                    req.prompt.trim(),
                    subtasks_format(k)
                ),
                None => req.prompt.trim().to_string(),
            };
            c.push(' ');
            c.push_str(&sh_quote(&prompt));
        }
        parts.push(c);
    }
    // install que falha não segura o Claude: o erro fica na tela, acima dele
    parts.join("; ")
}

/// Cria a worktree, copia os `.env`, grava o inventário e abre o workspace no herdr com o setup e o Claude.
/// `Ok(Some(aviso))`: a feature existe, mas sem os hooks de checkpoint e do gate do `/mr`.
pub fn create(herdr: &Herdr, req: &Request, port: u16) -> Result<Option<String>, String> {
    if branches(&req.repo).contains(&req.branch) {
        return Err(format!(
            "Já existe uma feature em {}. Escolha outro nome ou abra a existente.",
            req.branch
        ));
    }
    let path = worktree_path(&req.repo, &req.branch);
    if path.exists() {
        return Err(format!("{} já existe.", path.display()));
    }
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    git_err(
        &req.repo,
        &[
            "worktree",
            "add",
            "-b",
            &req.branch,
            &path.to_string_lossy(),
            &req.base,
        ],
    )?;
    if req.copy_env {
        for f in env_files(&req.repo) {
            let _ = std::fs::copy(req.repo.join(&f), path.join(&f));
        }
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut inv = Inventory {
        id: format!("{}-{now}", slug(&req.branch)),
        repo: req.repo.clone(),
        branch: req.branch.clone(),
        base: req.base.clone(),
        worktrees: vec![Worktree {
            repo: req.repo.clone(),
            path: path.clone(),
            branch: req.branch.clone(),
            created_branch: true,
        }],
        port,
        compose_project: compose_project(&req.repo, &req.branch),
        workspace_id: None,
        group: req.group.clone(),
        model: req.model.map(String::from),
        created_at: now,
        parent: req.parent.as_ref().map(|m| Parent {
            id: m.id.clone(),
            branch: m.branch.clone(),
            sha: git(&req.repo, &["rev-parse", &m.branch])
                .unwrap_or_default()
                .trim()
                .to_string(),
            base: m.parent.as_ref().map_or(m.base.clone(), |p| p.base.clone()),
        }),
        jira: req.jira.clone(),
    };
    // grava antes do herdr: se algo falhar daqui em diante, a Limpeza ainda acha a worktree
    save(&inv).map_err(|e| e.to_string())?;
    for repo in &req.others {
        add_worktree(&mut inv, repo)?;
    }
    let others: Vec<PathBuf> = inv.worktrees[1..].iter().map(|w| w.path.clone()).collect();

    let ws = herdr
        .call(
            "workspace.create",
            json!({
                "cwd": path,
                "label": req.branch,
                "focus": true,
                "env": { "PORT": port.to_string(), "COMPOSE_PROJECT_NAME": inv.compose_project },
            }),
        )
        .map_err(|e| format!("herdr: {e}"))?;
    let ws_id = ws["workspace"]["workspace_id"].as_str().map(String::from);
    let pane = ws["root_pane"]["pane_id"].as_str().map(String::from);
    inv.workspace_id = ws_id.clone();
    save(&inv).map_err(|e| e.to_string())?;

    let warning = install_hook(&req.repo, &path, &inv.id)
        .err()
        .map(|e| format!("Feature criada sem os hooks (turnos e gate do /mr): {e}"));
    // a nota do repo no vault e o que ela linka entram no contexto do Claude da feature
    let repo_name = req
        .repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let vault = crate::vault::context(&crate::vault::load(&crate::vault::dir()), &repo_name);
    let context = match (vault, feature_context(&inv)) {
        (None, None) => None,
        (v, e) => Some(
            [v, e]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("\n\n"),
        ),
    }
    .and_then(|c| {
        let f = data_dir().join(format!("{}.context.md", inv.id));
        std::fs::write(&f, c).ok().map(|_| f)
    });
    let setup = setup_command(&req.repo).map(|s| match registry_auth(&req.repo) {
        Some(a) => format!("{a} && {s}"),
        None => s,
    });
    let cmd = start_command(req, setup.as_deref(), context.as_deref(), &others);
    if let (Some(pane), false) = (&pane, cmd.is_empty()) {
        let _ = herdr.call(
            "pane.send_text",
            json!({ "pane_id": pane, "text": format!("{cmd}\r") }),
        );
    }
    // aba 2 é zsh, como no protótipo; cada outro repo ganha a sua aba
    if let Some(ws) = &ws_id {
        for cwd in std::iter::once(&path).chain(&others) {
            let _ = herdr.call(
                "tab.create",
                json!({ "workspace_id": ws, "cwd": cwd, "focus": false }),
            );
        }
    }
    Ok(warning)
}

fn repo_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// O que o Claude da feature precisa saber além do vault: os outros repos e a mãe da pilha.
fn feature_context(inv: &Inventory) -> Option<String> {
    let mut out = Vec::new();
    if inv.worktrees.len() > 1 {
        let list: Vec<String> = inv
            .worktrees
            .iter()
            .map(|w| format!("- {}: {}", repo_name(&w.repo), w.path.display()))
            .collect();
        out.push(format!(
            "Esta feature é multi-repo: a branch {} existe numa worktree de cada repo abaixo.\n{}\nAo abrir os MRs, cite os outros no corpo de cada um como \"Relacionados: <repo>!<iid>\".",
            inv.branch,
            list.join("\n")
        ));
    }
    out.push(match &inv.parent {
        Some(p) => format!(
            "Esta feature está empilhada em {0}: o MR dela deve apontar (target branch) para {0}, não para {1}.",
            p.branch, p.base
        ),
        None => format!("O MR desta feature aponta (target branch) para {}.", inv.base),
    });
    (!out.is_empty()).then(|| out.join("\n\n"))
}

/// O que a confirmação precisa mostrar antes de concluir.
pub struct Pending {
    pub uncommitted: usize,
    pub merged: bool,
    /// (repo, não commitados) das worktrees com mudança, quando a feature é multi-repo
    pub dirty: Vec<(String, usize)>,
}

/// Conta o não commitado de todas as worktrees: as extras limpas saem sem perguntar.
pub fn pending(inv: &Inventory) -> Pending {
    let dirty: Vec<(String, usize)> = inv
        .worktrees
        .iter()
        .map(|w| {
            (
                repo_name(&w.repo),
                git(&w.path, &["status", "--porcelain"]).map_or(0, |s| s.lines().count()),
            )
        })
        .filter(|(_, n)| *n > 0)
        .collect();
    Pending {
        uncommitted: dirty.iter().map(|d| d.1).sum(),
        merged: is_merged(&inv.repo, &inv.branch, &inv.base),
        dirty: if inv.worktrees.len() > 1 {
            dirty
        } else {
            Vec::new()
        },
    }
}

fn is_merged(repo: &Path, branch: &str, base: &str) -> bool {
    git(
        repo,
        &["branch", "--merged", base, "--format=%(refname:short)"],
    )
    .is_some_and(|s| s.lines().any(|l| l == branch))
}

pub fn find_by_path(path: &Path) -> Option<Inventory> {
    inventories()
        .into_iter()
        .find(|i| i.worktrees.iter().any(|w| path.starts_with(&w.path)))
}

/// Conclui a feature apagando só o que o inventário diz que ela criou. Volume do compose fica.
pub fn conclude(herdr: &Herdr, inv: &Inventory) -> Result<Vec<String>, String> {
    let mut done = Vec::new();
    if let Some(ws) = &inv.workspace_id {
        // fecha antes da worktree: o claude e o shell saem da pasta
        if herdr
            .call("workspace.close", json!({ "workspace_id": ws }))
            .is_ok()
        {
            done.push(format!("workspace {ws} fechado"));
        }
    }
    let compose = Command::new("docker")
        .args(["compose", "-p", &inv.compose_project, "down"])
        .output();
    if compose.is_ok_and(|o| o.status.success()) {
        done.push(format!("compose {} parado", inv.compose_project));
    }
    for wt in &inv.worktrees {
        crate::lsp::stop(&wt.path);
        if wt.path.exists() {
            git_err(
                &wt.repo,
                &["worktree", "remove", "--force", &wt.path.to_string_lossy()],
            )?;
            done.push(format!("worktree {} removida", wt.path.display()));
        }
        let _ = git(&wt.repo, &["worktree", "prune"]);
        if wt.created_branch && is_merged(&wt.repo, &wt.branch, &inv.base) {
            git_err(&wt.repo, &["branch", "-d", &wt.branch])?;
            done.push(format!("branch {} apagada (já mergeada)", wt.branch));
        }
        let prefix = format!("refs/maestro/{}/", inv.id);
        for r in git(&wt.repo, &["for-each-ref", "--format=%(refname)", &prefix])
            .unwrap_or_default()
            .lines()
        {
            let _ = git(&wt.repo, &["update-ref", "-d", r]);
        }
        if let Some(parent) = wt.path.parent() {
            // `<repo>.wt` sai junto quando fica vazio
            let _ = std::fs::remove_dir(parent);
        }
    }
    let _ = std::fs::remove_file(data_dir().join(format!("{}.json", inv.id)));
    let _ = std::fs::remove_file(turns_file(&inv.id));
    let _ = std::fs::remove_file(preflight_file(&inv.id));
    for ext in ["merged", "sent", "hist", "context.md"] {
        let _ = std::fs::remove_file(data_dir().join(format!("{}.{ext}", inv.id)));
    }
    Ok(done)
}

/// Foto de todos os arquivos da worktree (inclusive os novos) num commit solto, sem tocar no índice nem na branch.
pub fn snapshot(root: &Path) -> Result<String, String> {
    let index = std::env::temp_dir().join(format!("maestro-idx-{}", std::process::id()));
    let _ = std::fs::remove_file(&index);
    let run = |args: &[&str]| -> Result<String, String> {
        let o = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .env("GIT_INDEX_FILE", &index)
            // a foto não precisa (nem deve) levar a identidade de quem usa; sem isto falha em máquina sem user.name
            .envs([
                ("GIT_AUTHOR_NAME", "Maestro"),
                ("GIT_AUTHOR_EMAIL", "maestro@localhost"),
                ("GIT_COMMITTER_NAME", "Maestro"),
                ("GIT_COMMITTER_EMAIL", "maestro@localhost"),
            ])
            .output()
            .map_err(|e| e.to_string())?;
        if o.status.success() {
            Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
        } else {
            Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
        }
    };
    run(&["read-tree", "HEAD"])?;
    run(&["add", "-A"])?;
    let tree = run(&["write-tree"])?;
    let sha = run(&[
        "commit-tree",
        &tree,
        "-p",
        "HEAD",
        "-m",
        "maestro checkpoint",
    ])?;
    let _ = std::fs::remove_file(&index);
    Ok(sha)
}

/// Volta a worktree para uma foto, byte a byte: apaga o que surgiu depois e restaura o resto. O índice fica.
pub fn restore(root: &Path, sha: &str) -> Result<(), String> {
    let now = snapshot(root)?;
    for f in git(root, &["diff", "--name-only", "--diff-filter=A", sha, &now])
        .unwrap_or_default()
        .lines()
    {
        let _ = std::fs::remove_file(root.join(f));
    }
    git_err(
        root,
        &[
            "restore",
            &format!("--source={sha}"),
            "--worktree",
            "--",
            ".",
        ],
    )
}

/// Um turno do Claude: a foto de antes do prompt N.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub n: usize,
    pub sha: String,
    pub prompt: String,
}

fn turns_file(id: &str) -> PathBuf {
    data_dir().join(format!("{id}.turns"))
}

/// Uma lista que sobrevive ao reinício (`<id>.merged`: MRs já oferecidos; `<id>.sent`: comentários já enviados).
pub fn state_lines(id: &str, ext: &str) -> Vec<String> {
    std::fs::read_to_string(data_dir().join(format!("{id}.{ext}")))
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

pub fn add_state_line(id: &str, ext: &str, line: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(data_dir().join(format!("{id}.{ext}")))
    {
        let _ = writeln!(f, "{line}");
    }
}

/// A árvore dos arquivos que passaram no pré-voo (ou tiveram override).
pub fn preflight_file(id: &str) -> PathBuf {
    data_dir().join(format!("{id}.preflight"))
}

pub fn turns(id: &str) -> Vec<Turn> {
    std::fs::read_to_string(turns_file(id))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut it = l.splitn(3, '\t');
            Some(Turn {
                n: it.next()?.parse().ok()?,
                sha: it.next()?.into(),
                prompt: it.next().unwrap_or("").into(),
            })
        })
        .collect()
}

/// O hook `UserPromptSubmit` da worktree chama isto (`maestro checkpoint <id>`) com o JSON do Claude no stdin.
/// O hook UserPromptSubmit: grava o turno e, num `/mr` sem pré-voo, devolve o motivo do bloqueio.
pub fn checkpoint(id: &str, stdin: &str) -> Result<Option<String>, String> {
    let inv = inventories()
        .into_iter()
        .find(|i| i.id == id)
        .ok_or("feature desconhecida")?;
    let root = &inv.worktrees[0].path;
    let sha = snapshot(root)?;
    let prompt = serde_json::from_str::<serde_json::Value>(stdin)
        .ok()
        .and_then(|v| v["prompt"].as_str().map(|p| p.replace(['\n', '\t'], " ")))
        .unwrap_or_default();
    // bloqueado não vira turno: o Claude nem recebe o prompt
    if let Some(reason) = crate::preflight::gate(id, root, &sha, &prompt) {
        return Ok(Some(reason));
    }
    let n = turns(id).len() + 1;
    git_err(
        root,
        &["update-ref", &format!("refs/maestro/{id}/t{n}"), &sha],
    )?;
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(turns_file(id))
        .map_err(|e| e.to_string())?;
    writeln!(f, "{n}\t{sha}\t{prompt}").map_err(|e| e.to_string())?;
    Ok(None)
}

/// Hook só nesta worktree (`.claude/settings.local.json`, fora do git pelo `info/exclude`): o settings global fica intacto.
pub fn install_hook(repo: &Path, worktree: &Path, id: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = worktree.join(".claude");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let exe = sh_quote(&exe.to_string_lossy());
    let hook = |cmd: String| json!([ { "hooks": [ { "type": "command", "command": cmd } ] } ]);
    let settings = json!({
        "hooks": {
            "UserPromptSubmit": hook(format!("{exe} checkpoint {id}")),
            "PermissionRequest": hook(format!("{exe} perm {id}")),
        },
        "permissions": { "allow": crate::perms::personal(repo) },
    });
    std::fs::write(
        dir.join("settings.local.json"),
        serde_json::to_string_pretty(&settings).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    let exclude = repo.join(".git/info/exclude");
    let text = std::fs::read_to_string(&exclude).unwrap_or_default();
    if !text.lines().any(|l| l == ".claude/settings.local.json") {
        let _ = std::fs::create_dir_all(exclude.parent().unwrap());
        std::fs::write(
            &exclude,
            format!(
                "{text}{}.claude/settings.local.json\n",
                if text.ends_with('\n') || text.is_empty() {
                    ""
                } else {
                    "\n"
                }
            ),
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Tentativa descartada no melhor de N: conclui e apaga a branch mesmo sem merge (foi você que escolheu).
pub fn discard(herdr: &Herdr, inv: &Inventory) -> Result<(), String> {
    conclude(herdr, inv)?;
    for w in inv.worktrees.iter().filter(|w| w.created_branch) {
        if branches(&w.repo).contains(&w.branch) {
            git_err(&w.repo, &["branch", "-D", &w.branch])?;
        }
    }
    Ok(())
}

/// Ficou com esta: o grupo some do inventário e ela vira uma feature comum.
pub fn keep(inv: &Inventory) -> Result<(), String> {
    let mut i = inv.clone();
    i.group = None;
    save(&i).map_err(|e| e.to_string())
}

/// Repositórios irmãos do da feature (`~/repos-ctbz/*` com `.git`), para o "+ repo".
pub fn sibling_repos(repo: &Path) -> Vec<PathBuf> {
    let Some(parent) = repo.parent() else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = std::fs::read_dir(parent)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p != repo && p.join(".git").is_dir())
        .collect();
    out.sort();
    out
}

/// "+ repo": worktree na mesma branch em outro repositório, só quando pedido, registrada no inventário.
pub fn add_repo(herdr: &Herdr, inv: &mut Inventory, repo: &Path) -> Result<PathBuf, String> {
    let path = add_worktree(inv, repo)?;
    // o Claude da feature passa a enxergar a pasta nova
    let agents = herdr.agents().unwrap_or_default();
    let ws = inv.workspace_id.as_deref();
    if let Some(a) = agents.iter().find(|a| Some(a.workspace_id.as_str()) == ws) {
        let _ = herdr.call(
            "pane.send_text",
            json!({ "pane_id": a.pane_id, "text": format!("/add-dir {}\r", path.display()) }),
        );
    }
    Ok(path)
}

/// A worktree da feature em mais um repositório, gravada no inventário (a conclusão apaga junto).
pub fn add_worktree(inv: &mut Inventory, repo: &Path) -> Result<PathBuf, String> {
    if inv.worktrees.iter().any(|w| w.repo == repo) {
        return Err(format!("{} já faz parte da feature", repo.display()));
    }
    let path = worktree_path(repo, &inv.branch);
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let exists = branches(repo).contains(&inv.branch);
    let base = bases(repo)
        .into_iter()
        .next()
        .ok_or("o repositório não tem branch base")?;
    let p = path.to_string_lossy().into_owned();
    if exists {
        git_err(repo, &["worktree", "add", &p, &inv.branch])?;
    } else {
        git_err(repo, &["worktree", "add", "-b", &inv.branch, &p, &base])?;
    }
    inv.worktrees.push(Worktree {
        repo: repo.to_path_buf(),
        path: path.clone(),
        branch: inv.branch.clone(),
        created_branch: !exists,
    });
    save(inv).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Numa filha empilhada, "Mudanças" e "Revisar" comparam com a mãe: os commits dela não são desta feature.
pub fn stack_base(root: &Path) -> Option<String> {
    let p = find_by_path(root)?.parent?;
    git(root, &["rev-parse", "--verify", "--quiet", &p.branch]).map(|_| p.branch)
}

/// O que a pilha pede para a filha (a pílula da lateral).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stack {
    /// "base mudou · Rebase"
    Rebase,
    /// "mergeada · Mover p/ main"
    MoveToBase,
}

/// Mãe mergeada vence: a filha vai para a base dela. Senão, a mãe andou e a filha não tem a ponta nova.
pub fn stack(mother_merged: bool, mother_tip_in_child: bool) -> Option<Stack> {
    if mother_merged {
        Some(Stack::MoveToBase)
    } else if !mother_tip_in_child {
        Some(Stack::Rebase)
    } else {
        None
    }
}

/// Lê do git o estado da pilha desta filha. Branch da mãe sumida conta como mergeada (a conclusão apaga a mergeada).
pub fn stack_state(inv: &Inventory, mother_mr_merged: bool) -> Option<Stack> {
    let p = inv.parent.as_ref()?;
    let wt = &inv.worktrees.first()?.path;
    let tip = git(wt, &["rev-parse", "--verify", "-q", &p.branch]).map(|s| s.trim().to_string());
    let in_child = tip
        .as_deref()
        .is_some_and(|t| git(wt, &["merge-base", "--is-ancestor", t, "HEAD"]).is_some());
    stack(mother_mr_merged || tip.is_none(), in_child)
}

/// Os arquivos em conflito de um rebase parado, para o toast e o terminal da filha.
/// Falha sem conflito não deixa a worktree no meio de um rebase: aborta e volta ao que era.
fn conflict(wt: &Path, e: String) -> String {
    let files: Vec<String> = git(wt, &["diff", "--name-only", "--diff-filter=U"])
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect();
    if files.is_empty() {
        let _ = git(wt, &["rebase", "--abort"]);
        e
    } else {
        format!("conflito em {}", files.join(", "))
    }
}

/// "base mudou · Rebase": `git rebase <mãe>` na worktree da filha. Conflito para no meio, para resolver no terminal dela.
pub fn rebase_on_parent(inv: &mut Inventory) -> Result<(), String> {
    let p = inv
        .parent
        .clone()
        .ok_or("esta feature não está empilhada")?;
    let wt = inv.worktrees[0].path.clone();
    git_err(&wt, &["rebase", "--autostash", &p.branch]).map_err(|e| conflict(&wt, e))?;
    if let Some(sha) = git(&wt, &["rev-parse", &p.branch]) {
        inv.parent.as_mut().unwrap().sha = sha.trim().to_string();
    }
    save(inv).map_err(|e| e.to_string())
}

/// "Mover p/ main": a mãe mergeou, então só os commits da filha vão para a base (`rebase --onto <base> <ponta antiga da mãe>`).
pub fn move_to_base(inv: &mut Inventory) -> Result<String, String> {
    let p = inv
        .parent
        .clone()
        .ok_or("esta feature não está empilhada")?;
    let wt = inv.worktrees[0].path.clone();
    // o merge da mãe está no remoto; a base local pode estar atrás
    let remote = format!("origin/{}", p.base);
    let onto = if git_err(&wt, &["fetch", "-q", "origin", &p.base]).is_ok() {
        remote.as_str()
    } else {
        p.base.as_str()
    };
    git_err(&wt, &["rebase", "--autostash", "--onto", onto, &p.sha])
        .map_err(|e| conflict(&wt, e))?;
    inv.base = p.base.clone();
    inv.parent = None;
    save(inv).map_err(|e| e.to_string())?;
    Ok(p.base)
}

/// Envs lidas nas linhas adicionadas do diff: (nome, `arquivo:linha`).
// ponytail: busca textual pelos jeitos comuns de ler env em TS/Go/Rust/Python; parser de verdade se der falso positivo
pub fn new_envs(files: &[crate::diff::FileDiff]) -> Vec<(String, String)> {
    const READS: [&str; 9] = [
        "process.env.",
        "process.env[",
        "configService.get(",
        "configService.get<",
        "configService.getOrThrow(",
        "os.Getenv(",
        "env::var(",
        "os.environ.get(",
        "os.getenv(",
    ];
    let mut out: Vec<(String, String)> = Vec::new();
    for f in files {
        for l in f.hunks.iter().flat_map(|h| &h.lines) {
            if l.kind != crate::diff::Kind::Add {
                continue;
            }
            for r in READS {
                for (i, _) in l.text.match_indices(r) {
                    let rest = &l.text[i + r.len()..];
                    // `get<number>('X')`: pula o tipo
                    let rest = rest
                        .split_once('(')
                        .filter(|_| r.ends_with('<'))
                        .map_or(rest, |x| x.1);
                    let rest = rest.trim_start_matches(['\'', '"', '`']);
                    let name: String = rest
                        .chars()
                        .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
                        .collect();
                    let real = name.len() > 1 && name.chars().any(|c| c.is_ascii_uppercase());
                    if real && !out.iter().any(|(n, _)| *n == name) {
                        out.push((name, format!("{}:{}", f.path, l.new.unwrap_or_default())));
                    }
                }
            }
        }
    }
    out
}

/// O configmap do serviço: os `<serviço>.yaml` em qualquer `<cluster>/<namespace>/`.
fn configmap_files(dir: &Path, service: &str, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() && e.file_name() != ".git" {
            configmap_files(&p, service, out);
        } else if p.file_stem().is_some_and(|s| s == service)
            && p.extension().is_some_and(|x| x == "yaml" || x == "yml")
        {
            out.push(p);
        }
    }
}

/// Das envs novas, as que nenhum configmap do serviço declara. Sem arquivo do serviço não dá para dizer: nada.
pub fn missing_in_configmap(
    envs: &[(String, String)],
    configmap: &Path,
    service: &str,
) -> Vec<(String, String)> {
    let mut files = Vec::new();
    configmap_files(configmap, service, &mut files);
    let text: String = files
        .iter()
        .filter_map(|f| std::fs::read_to_string(f).ok())
        .collect();
    if files.is_empty() {
        return Vec::new();
    }
    envs.iter()
        .filter(|(n, _)| !text.contains(&format!("{n}:")))
        .cloned()
        .collect()
}

/// O aviso do Revisar: envs novas da feature que faltam no k8s-configmap dela (só com a worktree do configmap na feature).
pub fn env_warning(inv: &Inventory, files: &[crate::diff::FileDiff]) -> Vec<(String, String)> {
    let Some(cm) = inv
        .worktrees
        .iter()
        .find(|w| w.repo.file_name().is_some_and(|n| n == "k8s-configmap"))
    else {
        return Vec::new();
    };
    missing_in_configmap(&new_envs(files), &cm.path, &repo_name(&inv.repo))
}

/// Algo no disco que nenhuma feature reconhece.
#[derive(Debug, Clone, PartialEq)]
pub enum Orphan {
    /// worktree em `<repo>.wt/` (onde o Maestro cria as dele) sem inventário
    Worktree {
        repo: PathBuf,
        path: PathBuf,
        dirty: bool,
    },
    /// inventário cuja worktree já não existe
    Inventory { id: String, branch: String },
    /// `refs/maestro/<id>/…` de uma feature que não existe mais
    Ref { repo: PathBuf, name: String },
    /// projeto do compose `<repo>-…` parado e sem feature
    Compose { name: String },
    /// branch local já mergeada na base, sem worktree; pode ser sua, por isso vem desmarcada
    Branch { repo: PathBuf, name: String },
}

impl Orphan {
    pub fn label(&self) -> (String, String) {
        match self {
            Orphan::Worktree { path, dirty, .. } => (
                path.display().to_string(),
                if *dirty {
                    "worktree sem feature, com mudanças não commitadas".into()
                } else {
                    "worktree sem feature".into()
                },
            ),
            Orphan::Inventory { branch, .. } => (
                branch.clone(),
                "inventário de feature cuja worktree sumiu".into(),
            ),
            Orphan::Ref { name, .. } => (name.clone(), "ref de feature removida".into()),
            Orphan::Compose { name } => (name.clone(), "compose parado".into()),
            Orphan::Branch { name, .. } => (name.clone(), "branch local mergeada".into()),
        }
    }

    /// Marcado por padrão: só o que é seguro apagar sem olhar.
    pub fn safe(&self) -> bool {
        !matches!(
            self,
            Orphan::Branch { .. } | Orphan::Worktree { dirty: true, .. }
        )
    }

    pub fn remove(&self) -> Result<(), String> {
        match self {
            Orphan::Worktree { repo, path, .. } => {
                git_err(
                    repo,
                    &["worktree", "remove", "--force", &path.to_string_lossy()],
                )?;
                if let Some(parent) = path.parent() {
                    let _ = std::fs::remove_dir(parent);
                }
                Ok(())
            }
            Orphan::Inventory { id, .. } => {
                std::fs::remove_file(data_dir().join(format!("{id}.json")))
                    .map_err(|e| e.to_string())
            }
            Orphan::Ref { repo, name } => git_err(repo, &["update-ref", "-d", name]),
            Orphan::Compose { name } => Command::new("docker")
                .args(["compose", "-p", name, "down"])
                .output()
                .map_err(|e| e.to_string())
                .map(drop),
            Orphan::Branch { repo, name } => git_err(repo, &["branch", "-d", name]),
        }
    }
}

/// Compara o disco com os inventários nos repositórios dados (os das features e os abertos no herdr).
pub fn orphans(repos: &[PathBuf]) -> Vec<Orphan> {
    let invs = inventories();
    let known_paths: Vec<&Path> = invs
        .iter()
        .flat_map(|i| i.worktrees.iter().map(|w| w.path.as_path()))
        .collect();
    let mut out = Vec::new();
    for inv in &invs {
        if inv.worktrees.iter().all(|w| !w.path.exists()) {
            out.push(Orphan::Inventory {
                id: inv.id.clone(),
                branch: inv.branch.clone(),
            });
        }
    }
    let mut repos: Vec<PathBuf> = repos
        .iter()
        .cloned()
        .chain(invs.iter().map(|i| i.repo.clone()))
        .collect();
    repos.sort();
    repos.dedup();
    for repo in &repos {
        let wt_dir = worktree_path(repo, "x")
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let list = git(repo, &["worktree", "list", "--porcelain"]).unwrap_or_default();
        let mut wt_branches = Vec::new();
        for block in list.split("\n\n") {
            let path = block
                .lines()
                .find_map(|l| l.strip_prefix("worktree "))
                .map(PathBuf::from);
            if let Some(b) = block
                .lines()
                .find_map(|l| l.strip_prefix("branch refs/heads/"))
            {
                wt_branches.push(b.to_string());
            }
            if let Some(path) =
                path.filter(|p| p.starts_with(&wt_dir) && !known_paths.contains(&p.as_path()))
            {
                let dirty =
                    git(&path, &["status", "--porcelain"]).is_some_and(|s| !s.trim().is_empty());
                out.push(Orphan::Worktree {
                    repo: repo.clone(),
                    path,
                    dirty,
                });
            }
        }
        for r in git(
            repo,
            &["for-each-ref", "--format=%(refname)", "refs/maestro/"],
        )
        .unwrap_or_default()
        .lines()
        {
            let id = r
                .trim_start_matches("refs/maestro/")
                .split('/')
                .next()
                .unwrap_or_default();
            if !invs.iter().any(|i| i.id == id) {
                out.push(Orphan::Ref {
                    repo: repo.clone(),
                    name: r.into(),
                });
            }
        }
        let base = bases(repo).into_iter().next().unwrap_or_default();
        let current = git(repo, &["branch", "--show-current"]).unwrap_or_default();
        for b in git(
            repo,
            &["branch", "--merged", &base, "--format=%(refname:short)"],
        )
        .unwrap_or_default()
        .lines()
        {
            let in_use = b == base || b == current.trim() || wt_branches.iter().any(|w| w == b);
            if !in_use
                && !invs
                    .iter()
                    .any(|i| i.worktrees.iter().any(|w| w.branch == b))
            {
                out.push(Orphan::Branch {
                    repo: repo.clone(),
                    name: b.into(),
                });
            }
        }
        // só compose cujo arquivo está numa worktree do Maestro; prefixo de nome pegaria projeto de outro repo
        for (name, file) in stopped_compose_projects() {
            if file.starts_with(&wt_dir) && !invs.iter().any(|i| i.compose_project == name) {
                out.push(Orphan::Compose { name });
            }
        }
    }
    out
}

/// (nome, primeiro arquivo do compose) dos projetos parados.
fn stopped_compose_projects() -> Vec<(String, PathBuf)> {
    let Ok(o) = Command::new("docker")
        .args(["compose", "ls", "-a", "--format", "json"])
        .output()
    else {
        return Vec::new();
    };
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_default();
    v.as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["Status"].as_str().is_some_and(|s| !s.contains("running")))
        .filter_map(|p| {
            let file = p["ConfigFiles"].as_str()?.split(',').next()?;
            Some((p["Name"].as_str()?.to_string(), PathBuf::from(file)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(prompt: &str) -> Request {
        Request {
            repo: "/r/bots-api".into(),
            branch: "feat/x".into(),
            base: "main".into(),
            prompt: prompt.into(),
            copy_env: true,
            setup: true,
            claude: true,
            model: None,
            group: None,
            others: Vec::new(),
            parent: None,
            jira: None,
        }
    }

    #[test]
    fn nomes_portas_e_comando() {
        assert_eq!(slug("feat/PRD-1 Paginação!"), "feat-prd-1-paginacao");
        assert_eq!(
            worktree_path(Path::new("/r/bots-api"), "feat/health"),
            PathBuf::from("/r/bots-api.wt/feat-health")
        );
        assert_eq!(
            compose_project(Path::new("/r/bots-api"), "feat/health"),
            "bots-api-feat-health"
        );
        assert_eq!(next_port(&[3001, 3003]), 3002);
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(
            start_command(&req("faz X"), Some("npm ci"), None, &[]),
            "npm ci; claude --permission-mode plan 'faz X'"
        );
        let mut r = req("");
        r.setup = false;
        r.model = Some("sonnet");
        assert_eq!(
            start_command(&r, Some("npm ci"), None, &[]),
            "claude --permission-mode plan --model sonnet"
        );
        assert_eq!(
            start_command(&r, None, Some(Path::new("/d/x.context.md")), &[]),
            "claude --permission-mode plan --model sonnet --append-system-prompt \"$(cat '/d/x.context.md')\""
        );
        let mut j = req("pagina");
        j.setup = false;
        j.jira = Some("PRD-1".into());
        let cmd = start_command(&j, None, None, &[]);
        assert!(cmd.starts_with("claude --permission-mode plan 'pagina No fim do plano"));
        assert!(
            cmd.contains("```subtasks")
                && cmd.contains("de PRD-1 no padrão")
                && !cmd.contains('\n')
        );
    }

    #[test]
    fn inventario_antigo_sem_jira() {
        let old = r#"{"id":"x","repo":"/r","branch":"b","base":"main","worktrees":[],"port":3001,
            "compose_project":"r-b","workspace_id":null,"created_at":1}"#;
        let inv: Inventory = serde_json::from_str(old).unwrap();
        assert_eq!(inv.jira, None);
        let mut v = serde_json::to_value(&inv).unwrap();
        v["jira"] = "PRD-1".into();
        let inv: Inventory = serde_json::from_value(v).unwrap();
        assert_eq!(inv.jira.as_deref(), Some("PRD-1"));
    }

    #[test]
    fn setup_e_env_de_um_repo() {
        let dir = std::env::temp_dir().join(format!("maestro-feat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .output()
                .unwrap()
        };
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.join(".gitignore"), ".env\n.env.local\n").unwrap();
        std::fs::write(dir.join(".env"), "A=1").unwrap();
        std::fs::write(dir.join(".env.local"), "B=2").unwrap();
        std::fs::write(dir.join(".env.example"), "A=").unwrap();
        std::fs::write(dir.join("package-lock.json"), "{}").unwrap();
        let mut env = env_files(&dir);
        env.sort();
        assert_eq!(env, vec![".env", ".env.local"]);
        assert_eq!(setup_command(&dir).as_deref(), Some("npm ci"));
        assert_eq!(registry_auth(&dir), None);
        std::fs::write(
            dir.join(".npmrc"),
            "@x:registry=https://us-central1-npm.pkg.dev/p/npm/\n",
        )
        .unwrap();
        assert!(registry_auth(&dir).is_some_and(|a| a.contains("--credential-config ~/.npmrc")));
        std::fs::write(dir.join(".maestro.toml"), "setup = \"make deps\"\n").unwrap();
        assert_eq!(setup_command(&dir).as_deref(), Some("make deps"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn mais_um_repo_e_conclusao_voltam_ao_estado_de_antes() {
        let root = std::env::temp_dir().join(format!("maestro-multi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (app, k8s) = (root.join("app"), root.join("k8s-configmap"));
        for r in [&app, &k8s] {
            std::fs::create_dir_all(r).unwrap();
            for args in [
                &["init", "-q", "-b", "main"][..],
                &[
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "commit",
                    "-q",
                    "--allow-empty",
                    "-m",
                    "i",
                ],
            ] {
                Command::new("git")
                    .arg("-C")
                    .arg(r)
                    .args(args)
                    .output()
                    .unwrap();
            }
        }
        let state = |r: &Path| {
            (
                git(r, &["worktree", "list", "--porcelain"])
                    .unwrap()
                    .lines()
                    .filter(|l| l.starts_with("worktree"))
                    .count(),
                git(r, &["branch", "--format=%(refname:short)"]).unwrap(),
            )
        };
        let before = (state(&app), state(&k8s));
        let h = Herdr::offline();
        let wt = worktree_path(&app, "feat/multi");
        git_err(
            &app,
            &[
                "worktree",
                "add",
                "-b",
                "feat/multi",
                &wt.to_string_lossy(),
                "main",
            ],
        )
        .unwrap();
        let mut inv = Inventory {
            id: format!("teste-multi-{}", std::process::id()),
            repo: app.clone(),
            branch: "feat/multi".into(),
            base: "main".into(),
            worktrees: vec![Worktree {
                repo: app.clone(),
                path: wt,
                branch: "feat/multi".into(),
                created_branch: true,
            }],
            port: 3999,
            compose_project: "app-feat-multi-teste".into(),
            workspace_id: None,
            group: None,
            model: None,
            created_at: 0,
            parent: None,
            jira: None,
        };
        assert_eq!(sibling_repos(&app), vec![k8s.clone()]);
        let extra = add_repo(&h, &mut inv, &k8s).unwrap();
        assert!(extra.ends_with("k8s-configmap.wt/feat-multi") && extra.exists());
        assert!(add_repo(&h, &mut inv, &k8s).is_err());
        assert_eq!(pending(&inv).uncommitted, 0);
        conclude(&h, &inv).unwrap();
        assert_eq!((state(&app), state(&k8s)), before);
        assert!(!root.join("app.wt").exists() && !root.join("k8s-configmap.wt").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn outros_repos_entram_no_claude() {
        let r = req("");
        assert_eq!(
            start_command(
                &r,
                None,
                None,
                &[PathBuf::from("/r/k8s-configmap.wt/feat-x")]
            ),
            "claude --permission-mode plan --add-dir '/r/k8s-configmap.wt/feat-x'"
        );
    }

    /// Mãe ganha commit → Rebase; mãe mergeada e apagada → Mover para a base, só com os commits da filha.
    #[test]
    fn pilha_de_ponta_a_ponta() {
        let root = std::env::temp_dir().join(format!("maestro-pilha-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("app");
        std::fs::create_dir_all(&repo).unwrap();
        let run = |dir: &Path, args: &[&str]| {
            let o = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .output()
                .unwrap();
            assert!(
                o.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
        };
        let commit = |dir: &Path, file: &str| {
            std::fs::write(dir.join(file), file).unwrap();
            run(dir, &["add", "."]);
            run(dir, &["commit", "-qm", file]);
        };
        run(&repo, &["init", "-q", "-b", "main"]);
        // o rebase do app usa o git sem `-c`: no CI não há identidade global
        run(&repo, &["config", "user.name", "t"]);
        run(&repo, &["config", "user.email", "t@t"]);
        commit(&repo, "base.txt");
        let (mwt, cwt) = (root.join("mae"), root.join("filha"));
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feat/mae",
                &mwt.to_string_lossy(),
                "main",
            ],
        );
        commit(&mwt, "mae1.txt");
        let mother = Inventory {
            id: format!("teste-mae-{}", std::process::id()),
            repo: repo.clone(),
            branch: "feat/mae".into(),
            base: "main".into(),
            worktrees: vec![Worktree {
                repo: repo.clone(),
                path: mwt.clone(),
                branch: "feat/mae".into(),
                created_branch: true,
            }],
            port: 3998,
            compose_project: "x".into(),
            workspace_id: None,
            group: None,
            model: None,
            created_at: 0,
            parent: None,
            jira: None,
        };
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feat/filha",
                &cwt.to_string_lossy(),
                "feat/mae",
            ],
        );
        commit(&cwt, "filha.txt");
        let mut child = Inventory {
            id: format!("teste-filha-{}", std::process::id()),
            branch: "feat/filha".into(),
            base: "feat/mae".into(),
            worktrees: vec![Worktree {
                repo: repo.clone(),
                path: cwt.clone(),
                branch: "feat/filha".into(),
                created_branch: true,
            }],
            parent: Some(Parent {
                id: mother.id.clone(),
                branch: "feat/mae".into(),
                sha: git(&repo, &["rev-parse", "feat/mae"])
                    .unwrap()
                    .trim()
                    .into(),
                base: "main".into(),
            }),
            ..mother.clone()
        };
        assert_eq!(stack_state(&child, false), None);
        commit(&mwt, "mae2.txt");
        assert_eq!(stack_state(&child, false), Some(Stack::Rebase));
        rebase_on_parent(&mut child).unwrap();
        assert!(cwt.join("mae2.txt").exists());
        assert_eq!(stack_state(&child, false), None);
        // a mãe mergeia (squash, como no GitLab) e a branch dela some
        run(&repo, &["merge", "-q", "--squash", "feat/mae"]);
        run(&repo, &["commit", "-qm", "squash da mae"]);
        run(
            &repo,
            &["worktree", "remove", "--force", &mwt.to_string_lossy()],
        );
        run(&repo, &["branch", "-D", "feat/mae"]);
        assert_eq!(stack_state(&child, false), Some(Stack::MoveToBase));
        assert_eq!(move_to_base(&mut child).unwrap(), "main");
        assert_eq!(
            (child.base.as_str(), child.parent.is_none()),
            ("main", true)
        );
        let log = git(&cwt, &["log", "--format=%s", "main..HEAD"]).unwrap();
        assert_eq!(
            log.trim(),
            "filha.txt",
            "só o commit da filha fica acima da main"
        );
        for id in [&mother.id, &child.id] {
            let _ = std::fs::remove_file(data_dir().join(format!("{id}.json")));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn caminhos_para_o_arroba() {
        let ps = paths(Path::new("."));
        let first_file = ps.iter().position(|p| !p.ends_with('/')).unwrap();
        assert!(ps[..first_file].contains(&"src/fixtures/tasks/".to_string()));
        assert!(ps[..first_file].contains(&"src/".to_string()));
        assert_eq!(matching(&ps, "FORM.R", 8), [&"src/form.rs".to_string()]);
        let here = std::fs::canonicalize(".").unwrap();
        let src = format!("{}/src", here.to_string_lossy());
        let hits = disk_matching(&format!("{src}/F"), 8);
        assert_eq!(hits[0], format!("{src}/fixtures/"));
        assert!(hits.contains(&format!("{src}/form.rs")));
        assert!(disk_matching("~", 50).iter().all(|p| !p.contains("/.")));
        let parent = here.parent().unwrap();
        assert!(!repos(parent, &here).contains(&format!("{}/", here.to_string_lossy())));
    }

    #[test]
    fn decisao_da_pilha() {
        assert_eq!(stack(true, true), Some(Stack::MoveToBase));
        assert_eq!(stack(true, false), Some(Stack::MoveToBase));
        assert_eq!(stack(false, false), Some(Stack::Rebase));
        assert_eq!(stack(false, true), None);
    }

    #[test]
    fn envs_novas_e_o_configmap() {
        let files = crate::diff::parse(
            "diff --git a/src/slack.ts b/src/slack.ts\n--- a/src/slack.ts\n+++ b/src/slack.ts\n@@ -1,2 +1,4 @@\n const a = 1\n-const t = process.env.OLD_TIMEOUT\n+const t = Number(process.env.SLACK_TIMEOUT_MS ?? 10000)\n+const k = this.configService.get<number>('RETRY_MAX') + process.env['API_KEY']\n+const x = process.env.node_env\n",
        );
        let envs = new_envs(&files);
        let names: Vec<&str> = envs.iter().map(|e| e.0.as_str()).collect();
        assert_eq!(names, ["SLACK_TIMEOUT_MS", "API_KEY", "RETRY_MAX"]);
        assert_eq!(envs[0].1, "src/slack.ts:2");
        let dir = std::env::temp_dir().join(format!("maestro-cm-{}", std::process::id()));
        let ns = dir.join("cluster/ns-prd");
        std::fs::create_dir_all(&ns).unwrap();
        std::fs::write(
            ns.join("bots-api.yaml"),
            "data:\n  API_KEY: x\n  RETRY_MAX: \"3\"\n",
        )
        .unwrap();
        let missing = missing_in_configmap(&envs, &dir, "bots-api");
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].0, "SLACK_TIMEOUT_MS");
        assert!(missing_in_configmap(&envs, &dir, "outro-servico").is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn foto_e_volta_byte_a_byte() {
        let dir = std::env::temp_dir().join(format!("maestro-snap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .output()
                .unwrap()
        };
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "1\n").unwrap();
        std::fs::write(dir.join("b.txt"), "b\n").unwrap();
        run(&["add", "."]);
        run(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "i",
        ]);
        std::fs::write(dir.join("a.txt"), "2 sem newline").unwrap();
        std::fs::write(dir.join("novo.txt"), "n\n").unwrap();
        let status =
            || String::from_utf8_lossy(&run(&["status", "--porcelain"]).stdout).into_owned();
        let before = status();
        let sha = snapshot(&dir).unwrap();
        assert_eq!(status(), before, "a foto não mexe no índice");
        std::fs::write(dir.join("a.txt"), "3\n").unwrap();
        std::fs::remove_file(dir.join("b.txt")).unwrap();
        std::fs::write(dir.join("depois.txt"), "d\n").unwrap();
        std::fs::remove_file(dir.join("novo.txt")).unwrap();
        restore(&dir, &sha).unwrap();
        assert_eq!(std::fs::read(dir.join("a.txt")).unwrap(), b"2 sem newline");
        assert_eq!(std::fs::read(dir.join("b.txt")).unwrap(), b"b\n");
        assert_eq!(std::fs::read(dir.join("novo.txt")).unwrap(), b"n\n");
        assert!(!dir.join("depois.txt").exists());
        assert_eq!(status(), before);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn acha_orfaos_sem_tocar_no_resto() {
        let root = std::env::temp_dir().join(format!("maestro-orf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("app");
        std::fs::create_dir_all(&repo).unwrap();
        let run = |dir: &Path, args: &[&str]| {
            let o = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .unwrap();
            assert!(
                o.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
        };
        run(&repo, &["init", "-q", "-b", "main"]);
        run(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "i",
            ],
        );
        run(&repo, &["branch", "velha"]);
        let wt = worktree_path(&repo, "feat/sobra");
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feat/sobra",
                &wt.to_string_lossy(),
                "main",
            ],
        );
        run(&repo, &["update-ref", "refs/maestro/sumiu-1/t1", "HEAD"]);
        let fora = root.join("minha-worktree");
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "minha",
                &fora.to_string_lossy(),
                "main",
            ],
        );
        let o = orphans(std::slice::from_ref(&repo));
        assert!(o.contains(&Orphan::Worktree {
            repo: repo.clone(),
            path: wt.clone(),
            dirty: false
        }));
        assert!(o.contains(&Orphan::Ref {
            repo: repo.clone(),
            name: "refs/maestro/sumiu-1/t1".into()
        }));
        assert!(o.contains(&Orphan::Branch {
            repo: repo.clone(),
            name: "velha".into()
        }));
        // worktree fora de `<repo>.wt/` não é do Maestro; branch em uso por worktree também não sai
        assert!(
            !o.iter()
                .any(|x| matches!(x, Orphan::Worktree { path, .. } if *path == fora))
        );
        assert!(!o.iter().any(
            |x| matches!(x, Orphan::Branch { name, .. } if name == "minha" || name == "feat/sobra")
        ));
        assert!(
            !Orphan::Branch {
                repo: repo.clone(),
                name: "velha".into()
            }
            .safe()
        );
        // só o que é deste repo de teste: inventários e composes reais da máquina não entram
        let mine = |x: &Orphan| matches!(x, Orphan::Worktree { repo: r, .. } | Orphan::Ref { repo: r, .. } if *r == repo);
        for x in o.iter().filter(|x| x.safe() && mine(x)) {
            x.remove().unwrap();
        }
        assert!(
            orphans(std::slice::from_ref(&repo))
                .iter()
                .filter(|x| mine(x))
                .all(|x| !x.safe())
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn contexto_sempre_diz_o_alvo_do_mr() {
        let inv = Inventory {
            id: "x".into(),
            repo: "/r/app".into(),
            branch: "feat/x".into(),
            base: "develop".into(),
            worktrees: vec![],
            port: 3001,
            compose_project: "app-feat-x".into(),
            workspace_id: None,
            group: None,
            model: None,
            created_at: 0,
            parent: None,
            jira: None,
        };
        assert!(feature_context(&inv).unwrap().contains("para develop."));
    }
}
