mod ask;
mod audit;
mod audit_view;
mod binds;
mod blame;
mod braille;
mod card_tab;
mod changes;
mod cleanup;
mod compare;
mod confirm;
mod diff;
mod editor;
mod feature;
mod form;
mod ghostty;
mod graph;
mod herdr;
mod jira;
mod keys;
mod lsp;
mod lsp_editor;
mod mascot;
mod mouse;
mod mr;
mod mr_notify;
mod mr_tab;
mod mr_watch;
mod mrs;
mod mrs_view;
mod new_feature;
mod news;
mod news_view;
mod notes;
mod palette;
mod palette_cmd;
mod perms;
mod preflight;
mod review;
mod session;
mod shell;
mod sidebar;
mod tasks;
mod terminal;
mod theme;
mod tour;
mod tunnels;
mod tunnels_view;
mod vault;
mod vault_view;

use std::sync::Arc;

use ghostty::GhosttyConfig;
use gpui_kit::*;
use herdr::Herdr;
use shell::Shell;
use sidebar::Sidebar;
use terminal::TerminalView;
use theme::Theme;

/// `spawn` sem `wait` deixa zumbi até o app fechar: a espera fica numa thread.
pub fn spawn_reaped(c: &mut std::process::Command) {
    if let Ok(mut child) = c.spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

fn main() {
    // `maestro checkpoint <id>`: o hook UserPromptSubmit da worktree, sem abrir janela
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("checkpoint") {
        let mut stdin = String::new();
        let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut stdin);
        match feature::checkpoint(args.get(2).map_or("", String::as_str), &stdin) {
            Ok(Some(reason)) => println!(
                "{}",
                serde_json::json!({ "decision": "block", "reason": reason })
            ),
            Ok(None) => {}
            Err(e) => eprintln!("maestro checkpoint: {e}"),
        }
        return;
    }
    // `maestro perm <id>`: o hook PermissionRequest só anota o pedido (as permissões que aprendem)
    if args.get(1).map(String::as_str) == Some("perm") {
        let mut stdin = String::new();
        let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut stdin);
        if let Err(e) = perms::record(args.get(2).map_or("", String::as_str), &stdin) {
            eprintln!("maestro perm: {e}");
        }
        return;
    }
    // aberto de dentro de um pane do herdr, o herdr do Maestro se recusaria a rodar ("nested herdr")
    for (key, _) in std::env::vars().filter(|(k, _)| k.starts_with("HERDR_")) {
        // SAFETY: ainda não há outras threads
        unsafe { std::env::remove_var(key) };
    }
    let herdr = Herdr::from_env().map(Arc::new);
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        cx.bind_keys([
            KeyBinding::new("tab", terminal::SendTab, Some("Terminal")),
            KeyBinding::new("shift-tab", terminal::SendBackTab, Some("Terminal")),
            KeyBinding::new("tab", ask::NextOption, Some("Kcmd")),
            // Ctrl+Shift+setas trocam o foco como no Ghostty; seleção por palavra fica no Shift+Alt
            KeyBinding::new(
                "ctrl-shift-left",
                editor::LeaveEditor,
                Some("MaestroEditor > Input"),
            ),
            // ponytail: o Ctrl+clique do gpui-base não navegou no teste; F12 chama o mesmo GoToDefinition
            KeyBinding::new(
                "f12",
                gpui_kit::component::input::GoToDefinition,
                Some("MaestroEditor > Input"),
            ),
        ]);
        // a sans do chrome do protótipo vai embutida: não depende de a máquina ter a fonte
        let _ = cx.text_system().add_fonts(vec![
            include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf").into(),
            include_bytes!("../assets/fonts/IBMPlexSans-Medium.ttf").into(),
            include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf").into(),
            include_bytes!("../assets/fonts/IBMPlexSans-Bold.ttf").into(),
        ]);
        let config = GhosttyConfig::load();
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("Maestro".into()),
                appears_transparent: true,
                ..Default::default()
            }),
            app_id: Some("maestro".into()),
            icon: image::load_from_memory_with_format(
                include_bytes!("../assets/icons/maestro-512.png"),
                image::ImageFormat::Png,
            )
            .ok()
            .map(|i| {
                std::sync::Arc::new(
                    i.resize(128, 128, image::imageops::FilterType::Lanczos3)
                        .to_rgba8(),
                )
            }),
            window_decorations: Some(WindowDecorations::Client),
            window_background: if config.background_opacity < 1. {
                WindowBackgroundAppearance::Transparent
            } else {
                WindowBackgroundAppearance::Opaque
            },
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            let theme = Theme::new(&config.palette, config.font_family.clone());
            theme.apply_to_editor(cx);
            let theme_name = if config.palette == palette::Palette::default() {
                "Dracula · "
            } else {
                ""
            };
            let font = format!("{theme_name}{} {}", config.font_family, config.font_size_pt);
            let launch = herdr.as_ref().map(|h| h.launch());
            let binds = if herdr.is_some() {
                binds::Binds::load()
            } else {
                binds::Binds::default()
            };
            let terminal = cx.new(|cx| TerminalView::new(config, launch, binds, cx));
            let anchors = tour::Anchors::default();
            let sidebar =
                herdr.map(|h| cx.new(|cx| Sidebar::new(h, theme.clone(), anchors.clone(), cx)));
            window.focus(&terminal.read(cx).focus_handle(), cx);
            cx.new(|cx| Shell::new(sidebar, terminal, theme, font, anchors, cx))
        })
        .expect("abrir a janela");
    });
}
