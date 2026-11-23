use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Style {
    /// `cmd <wt> --goto <arquivo>:<l>:<c>`
    Goto,
    /// `cmd <wt> <arquivo>:<l>:<c>`
    Direct,
    /// `cmd --line <l> --column <c> <arquivo>`
    Line,
}

#[derive(Debug, PartialEq)]
pub struct Editor {
    pub id: &'static str,
    pub label: &'static str,
    cmds: &'static [&'static str],
    style: Style,
}

const fn ed(
    id: &'static str,
    label: &'static str,
    cmds: &'static [&'static str],
    style: Style,
) -> Editor {
    Editor {
        id,
        label,
        cmds,
        style,
    }
}

pub const EDITORS: &[Editor] = &[
    ed("vscode", "VS Code", &["code"], Style::Goto),
    ed("cursor", "Cursor", &["cursor"], Style::Goto),
    ed("zed", "Zed", &["zed", "zeditor"], Style::Direct),
    ed(
        "vscode-insiders",
        "VS Code Insiders",
        &["code-insiders"],
        Style::Goto,
    ),
    ed("vscodium", "VSCodium", &["codium"], Style::Goto),
    ed("idea", "IntelliJ IDEA", &["idea"], Style::Line),
    ed("webstorm", "WebStorm", &["webstorm"], Style::Line),
    ed("goland", "GoLand", &["goland"], Style::Line),
    ed("pycharm", "PyCharm", &["pycharm"], Style::Line),
    ed("rustrover", "RustRover", &["rustrover"], Style::Line),
];

/// Um editor instalado e o comando achado no `PATH`.
pub struct Found {
    pub editor: &'static Editor,
    bin: PathBuf,
}

fn which(cmd: &str, path: &std::ffi::OsStr) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path).map(|d| d.join(cmd)).find(|p| {
        p.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

pub fn detect_in(path: &std::ffi::OsStr) -> Vec<Found> {
    EDITORS
        .iter()
        .filter_map(|e| {
            let bin = e.cmds.iter().find_map(|c| which(c, path))?;
            Some(Found { editor: e, bin })
        })
        .collect()
}

pub fn detect() -> Vec<Found> {
    detect_in(&std::env::var_os("PATH").unwrap_or_default())
}

fn args(style: Style, wt: &Path, at: Option<(&Path, u32)>) -> Vec<OsString> {
    let Some((file, line)) = at else {
        return vec![wt.into()];
    };
    let pos = |f: &Path| {
        let mut s = f.as_os_str().to_owned();
        s.push(format!(":{line}:1"));
        s
    };
    match style {
        Style::Goto => vec![wt.into(), "--goto".into(), pos(file)],
        Style::Direct => vec![wt.into(), pos(file)],
        Style::Line => vec![
            "--line".into(),
            line.to_string().into(),
            "--column".into(),
            "1".into(),
            file.into(),
        ],
    }
}

fn config_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".config/maestro/editor.json")
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Pref {
    editor: String,
}

fn saved() -> Option<String> {
    let t = std::fs::read_to_string(config_path()).ok()?;
    serde_json::from_str::<Pref>(&t).ok().map(|p| p.editor)
}

pub fn save(id: &str) {
    let p = config_path();
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    let _ = std::fs::write(
        p,
        serde_json::to_string(&Pref { editor: id.into() }).unwrap(),
    );
}

/// O salvo, se ainda instalado; senão o primeiro instalado na ordem de `EDITORS`.
fn pick<'a>(found: &'a [Found], saved: Option<&str>) -> Option<&'a Found> {
    found
        .iter()
        .find(|f| Some(f.editor.id) == saved)
        .or(found.first())
}

pub fn preferred(found: &[Found]) -> Option<&Found> {
    pick(found, saved().as_deref())
}

fn launch(mut c: Command) -> Result<(), String> {
    let mut child = c.spawn().map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Abre `wt` (ou `wt/rel` na linha) no editor `id`, ou no preferido. Devolve o toast.
pub fn open(wt: &Path, at: Option<(&str, u32)>, id: Option<&str>) -> String {
    let found = detect();
    let target = at.map(|(rel, line)| (wt.join(rel), line));
    let pick = match id {
        Some(id) => found.iter().find(|f| f.editor.id == id),
        None => preferred(&found),
    };
    let Some(f) = pick else {
        let what = target.as_ref().map_or(wt, |(p, _)| p.as_path());
        return match launch({
            let mut c = Command::new("xdg-open");
            c.arg(what);
            c
        }) {
            Ok(()) => "Nenhum editor encontrado no PATH: abri com o padrão do sistema".into(),
            Err(e) => format!("Nenhum editor encontrado no PATH e o xdg-open falhou: {e}"),
        };
    };
    let mut c = Command::new(&f.bin);
    c.args(args(
        f.editor.style,
        wt,
        target.as_ref().map(|(p, l)| (p.as_path(), *l)),
    ));
    match launch(c) {
        Ok(()) => match at {
            Some((rel, line)) => format!("{rel}:{line} no {}", f.editor.label),
            None => format!("Abrindo no {}", f.editor.label),
        },
        Err(e) => format!("Não deu para abrir no {}: {e}", f.editor.label),
    }
}

pub fn file_manager(dir: &Path) -> String {
    let mut c = Command::new("xdg-open");
    c.arg(dir);
    match launch(c) {
        Ok(()) => "Abrindo no gerenciador de arquivos".into(),
        Err(e) => format!("Não deu para abrir o gerenciador de arquivos: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bin(dir: &Path, name: &str) {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn argumentos_por_estilo() {
        let wt = Path::new("/w/x");
        let f = Path::new("/w/x/docs/minha nota.md");
        let s = |v: Vec<OsString>| {
            v.into_iter()
                .map(|a| a.into_string().unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(s(args(Style::Goto, wt, None)), ["/w/x"]);
        assert_eq!(
            s(args(Style::Goto, wt, Some((f, 42)))),
            ["/w/x", "--goto", "/w/x/docs/minha nota.md:42:1"]
        );
        assert_eq!(
            s(args(Style::Direct, wt, Some((f, 42)))),
            ["/w/x", "/w/x/docs/minha nota.md:42:1"]
        );
        assert_eq!(
            s(args(Style::Line, wt, Some((f, 42)))),
            ["--line", "42", "--column", "1", "/w/x/docs/minha nota.md"]
        );
    }

    #[test]
    fn detecta_so_o_que_esta_no_path_e_executavel() {
        let d = std::env::temp_dir().join(format!("maestro-open-in-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        bin(&d, "zeditor");
        bin(&d, "code");
        std::fs::write(d.join("cursor"), "").unwrap();
        let ids: Vec<_> = detect_in(d.as_os_str())
            .iter()
            .map(|f| f.editor.id)
            .collect();
        assert_eq!(ids, ["vscode", "zed"]);
        let found = detect_in(d.as_os_str());
        assert_eq!(pick(&found, Some("zed")).unwrap().editor.id, "zed");
        assert_eq!(pick(&found, Some("cursor")).unwrap().editor.id, "vscode");
        assert_eq!(pick(&found, None).unwrap().editor.id, "vscode");
        assert!(pick(&[], Some("zed")).is_none());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
