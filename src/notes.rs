//! Notas livres: cada uma é um `.md` em `<vault>/notas/`, formatável pelo Claude.

use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

use serde_json::Value;

pub const DIR: &str = "notas";

/// A primeira linha com texto, sem os `#` de título.
pub fn title(text: &str) -> String {
    text.lines()
        .map(|l| l.trim_start_matches('#').trim())
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

/// O título só vira nome de arquivo depois do Enter: antes disso o `# r` de quem ainda digita viraria `r.md`.
pub fn ready(text: &str) -> bool {
    !title(text).is_empty() && text.trim_start().contains('\n')
}

/// `notas/<slug>.md`, com `-2`, `-3`… se já existir.
pub fn free_key(root: &Path, text: &str) -> String {
    let base = match crate::feature::slug(&title(text)) {
        s if s.is_empty() => format!("nota-{}", crate::vault::today()),
        s => s,
    };
    (1..)
        .map(|i| match i {
            1 => format!("{DIR}/{base}.md"),
            i => format!("{DIR}/{base}-{i}.md"),
        })
        .find(|k| !root.join(k).exists())
        .unwrap_or_default()
}

/// O Claude mais barato e sem ferramentas arruma o Markdown sem inventar conteúdo: (texto, tokens).
pub fn format(text: &str) -> Result<(String, u64), String> {
    let prompt = format!(
        "Formate a anotação abaixo em Markdown limpo para o Obsidian.\n\
         - Mantenha o idioma, o sentido e TODA a informação; não invente nem acrescente nada.\n\
         - Primeira linha: `# título` curto.\n\
         - Procedimentos viram passos numerados; comandos, caminhos e nomes de arquivo vão em `código` ou em bloco ```sh.\n\
         - Avisos importantes em **negrito**.\n\
         - Corrija ortografia e abreviações (tbm, q, qdo).\n\
         Responda só com o Markdown, sem comentários e sem cercar tudo num bloco de código.\n\n{text}"
    );
    let mut child = Command::new("claude")
        .args([
            "-p",
            "--model",
            "haiku",
            "--tools",
            "",
            "--output-format",
            "json",
            "--max-budget-usd",
            "0.05",
            "--no-session-persistence",
            "--strict-mcp-config",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("claude: {e}"))?;
    if let Some(mut s) = child.stdin.take() {
        let _ = s.write_all(prompt.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let u = &v["usage"];
    let tokens = [
        "input_tokens",
        "output_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
    ]
    .iter()
    .filter_map(|k| u[k].as_u64())
    .sum();
    let md = v["result"]
        .as_str()
        .map(unfence)
        .filter(|s| !s.trim().is_empty())
        .ok_or("o Claude não devolveu o texto")?;
    Ok((md, tokens))
}

/// Às vezes o modelo cerca tudo num ```markdown mesmo pedindo que não.
fn unfence(s: &str) -> String {
    let t = s.trim();
    t.strip_prefix("```markdown")
        .or_else(|| t.strip_prefix("```md"))
        .and_then(|r| r.strip_suffix("```"))
        .map_or(t, str::trim)
        .to_string()
        + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titulo_e_nome() {
        assert_eq!(title("\n# Subir túnel de stg\nx"), "Subir túnel de stg");
        assert_eq!(title("sem cerquilha\nx"), "sem cerquilha");
        assert_eq!(title("#  \n"), "");
        assert!(!ready("# rotac"));
        assert!(ready("# rotacionar token\n"));
        let root = std::env::temp_dir().join(format!("maestro-notes-{}", std::process::id()));
        std::fs::create_dir_all(root.join(DIR)).unwrap();
        assert_eq!(free_key(&root, "# Subir Túnel!\n"), "notas/subir-tunel.md");
        std::fs::write(root.join("notas/subir-tunel.md"), "").unwrap();
        assert_eq!(
            free_key(&root, "# Subir Túnel!\n"),
            "notas/subir-tunel-2.md"
        );
        assert!(free_key(&root, "#\n\n").starts_with("notas/nota-"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tira_a_cerca() {
        assert_eq!(unfence("```markdown\n# a\n```"), "# a\n");
        assert_eq!(unfence("# a\n```sh\nls\n```"), "# a\n```sh\nls\n```\n");
    }
}
