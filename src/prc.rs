//! PRCs: o board de chamados do Jira e a base de receitas no vault (`prcs/`).
//! O board e a empresa ficam na config local, fora do repo.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::jira::{Board, Column, PrcCard};

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Config {
    pub board: u64,
    #[serde(default = "default_repos")]
    pub repos: String,
    /// o tipo de vínculo PRC↔ACR no Jira (ex.: `Problem/Incident`)
    #[serde(default)]
    pub acr_link: Option<String>,
}

fn default_repos() -> String {
    "~/repos".into()
}

pub const EXAMPLE: &str = "{\n  \"board\": 123,\n  \"repos\": \"~/repos\"\n}";

pub fn config_path() -> PathBuf {
    std::env::var_os("MAESTRO_PRC").map_or_else(
        || {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                .join(".config/maestro/prc.json")
        },
        PathBuf::from,
    )
}

/// `Ok(None)`: sem config (a tela mostra o exemplo); `Err`: config inválida.
pub fn load() -> Result<Option<Config>, String> {
    match std::fs::read_to_string(config_path()) {
        Ok(s) => serde_json::from_str(&s)
            .map(Some)
            .map_err(|e| format!("{}: {e}", config_path().display())),
        Err(_) => Ok(None),
    }
}

/// Os cards de cada coluna, na ordem do board. Card com status fora das colunas não aparece.
pub fn columns(b: &Board) -> Vec<(&Column, Vec<&PrcCard>)> {
    b.cols
        .iter()
        .map(|c| {
            let xs = if c.done {
                Vec::new()
            } else {
                b.cards
                    .iter()
                    .filter(|x| c.statuses.contains(&x.status))
                    .collect()
            };
            (c, xs)
        })
        .collect()
}

/// O status de destino ao soltar um card na coluna: o primeiro que ela mapeia.
pub fn target(c: &Column) -> Option<&str> {
    c.statuses.first().map(String::as_str)
}

/// As receitas de `prcs/` que batem com as labels do card.
pub fn recipes(labels: &[String], vault: &Path) -> Vec<String> {
    labels
        .iter()
        .filter(|l| vault.join("prcs").join(format!("{l}.md")).is_file())
        .cloned()
        .collect()
}

/// As notas de `prcs/` (sem `_acr.md`), para a IA procurar quando nenhuma label bate.
pub fn notes(vault: &Path) -> Vec<String> {
    let mut xs: Vec<String> = std::fs::read_dir(vault.join("prcs"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            let stem = n.strip_suffix(".md")?;
            (!stem.starts_with('_')).then(|| stem.to_string())
        })
        .collect();
    xs.sort();
    xs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jira::parse_board;

    #[test]
    fn colunas_e_destino() {
        let b = parse_board(include_str!("fixtures/prc-board.jsonl"), None).unwrap();
        let cols: Vec<_> = columns(&b)
            .iter()
            .map(|(c, xs)| {
                (
                    c.name.clone(),
                    xs.iter().map(|x| x.key.clone()).collect::<Vec<_>>(),
                )
            })
            .collect();
        assert_eq!(
            cols,
            [
                ("triagem".to_string(), vec!["PRC-1".to_string()]),
                ("em andamento".into(), vec![]),
                ("com dev".into(), vec!["PRC-2".into()]),
                ("resolvido".into(), vec![]),
            ]
        );
        assert_eq!(target(&b.cols[2]), Some("Aguardando Dev"));
        assert_eq!(
            serde_json::from_str::<Config>("{\"board\":7}").unwrap(),
            Config {
                board: 7,
                repos: "~/repos".into(),
                acr_link: None
            }
        );
    }

    #[test]
    fn receitas_do_vault() {
        let dir = std::env::temp_dir().join(format!("maestro-prc-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("prcs")).unwrap();
        for n in ["falha_envio_email", "alteracao_email", "_acr"] {
            std::fs::write(dir.join("prcs").join(format!("{n}.md")), "# x").unwrap();
        }
        let labels = ["escalado".to_string(), "falha_envio_email".to_string()];
        assert_eq!(recipes(&labels, &dir), ["falha_envio_email"]);
        assert_eq!(notes(&dir), ["alteracao_email", "falha_envio_email"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
