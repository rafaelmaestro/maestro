use std::sync::Arc;

use gpui_kit::{App, Div, EmptyView, Hsla, Stateful, div, prelude::*, px, rgb, rgba};

use crate::{palette::Palette, terminal::hsla};

/// Tokens do `:root` do protótipo aprovado. O que existe na paleta do Ghostty vem dela,
/// para o chrome seguir o tema do terminal; o resto são os tons intermediários do protótipo.
#[derive(Clone)]
pub struct Theme {
    pub win: Hsla,
    pub panel: Hsla,
    pub raise: Hsla,
    pub line: Hsla,
    pub fg: Hsla,
    pub muted: Hsla,
    pub comment: Hsla,
    pub accent: Hsla,
    pub accent_soft: Hsla,
    pub ok: Hsla,
    pub cyan: Hsla,
    pub orange: Hsla,
    pub sans: &'static str,
    pub mono: String,
}

impl Theme {
    pub fn new(p: &Palette, mono: String) -> Self {
        let accent = hsla(p.ansi[4]);
        Self {
            win: hsla(p.background),
            panel: hsla(p.ansi[0]),
            raise: rgb(0x313342).into(),
            line: rgb(0x3b3d50).into(),
            fg: hsla(p.foreground),
            muted: rgb(0x9a9dbb).into(),
            comment: hsla(p.ansi[8]),
            accent,
            accent_soft: Hsla { a: 0.16, ..accent },
            ok: hsla(p.ansi[2]),
            cyan: hsla(p.ansi[6]),
            orange: rgb(0xffb86c).into(),
            sans: "IBM Plex Sans",
            mono,
        }
    }
}

impl Theme {
    /// O `.ed` do protótipo no editor do gpui-component, que pinta pelo tema global dele.
    pub fn apply_to_editor(&self, cx: &mut App) {
        // carrega o tema escuro inteiro: a busca do editor (Ctrl+F) e os botões dela vinham claros
        gpui_kit::component::Theme::change(gpui_kit::component::ThemeMode::Dark, None, cx);
        let th = gpui_kit::component::Theme::global_mut(cx);
        th.foreground = self.fg;
        th.caret = self.fg;
        th.muted_foreground = self.comment;
        th.selection = rgba(0x44475ae6).into();
        // completar, hover e code actions do LSP: o `.cmpl`/`.hov` do protótipo
        th.popover = self.raise;
        th.tokens.popover = self.raise.into();
        th.popover_foreground = self.fg;
        th.accent = self.accent_soft;
        th.accent_foreground = self.fg;
        th.tokens.accent = self.accent_soft.into();
        th.border = self.line;
        let mut hl = (*th.highlight_theme).clone();
        hl.style.editor_background = Some(self.win);
        hl.style.editor_foreground = Some(self.fg);
        hl.style.editor_gutter_background = Some(self.win);
        hl.style.editor_line_number = Some(self.comment);
        hl.style.editor_active_line_number = Some(self.comment);
        hl.style.editor_active_line = None;
        th.highlight_theme = Arc::new(hl);
    }
}

/// A faixa de 4 px entre colunas que se arrasta para mudar a largura.
pub fn resize_handle<T: Clone + 'static>(id: &'static str, drag: T, t: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(4.))
        .h_full()
        .flex_none()
        .cursor_col_resize()
        .hover(|d| d.bg(t.accent_soft))
        .on_drag(drag, |_, _, _, cx| cx.new(|_| EmptyView))
}
