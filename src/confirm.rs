use gpui_kit::{prelude::FluentBuilder, *};

use crate::{
    form::{TextField, button, checkbox},
    mrs::MergeOpts,
    theme::Theme,
};

pub enum Answer {
    Ok,
    Cancel,
}

/// O `confirmDlg` do protótipo: título, texto, Cancelar e a ação (vermelha quando é perigosa).
pub struct Confirm {
    title: String,
    text: String,
    ok: String,
    danger: bool,
    /// campo opcional entre o texto e os botões (o motivo do won't do)
    pub field: Option<Entity<TextField>>,
    focus: FocusHandle,
    theme: Theme,
}

impl EventEmitter<Answer> for Confirm {}

impl Confirm {
    pub fn new(
        theme: Theme,
        title: String,
        text: String,
        ok: &str,
        danger: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            title,
            text,
            ok: ok.into(),
            danger,
            field: None,
            focus,
            theme,
        }
    }
}

impl Render for Confirm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let danger = self.danger;
        let red: Hsla = rgb(0xff5555).into();
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(move |_, e: &KeyDownEvent, _, cx| {
                match e.keystroke.key.as_str() {
                    "escape" => cx.emit(Answer::Cancel),
                    // Enter confirma só quando não é perigoso (o foco do protótipo fica no Cancelar)
                    "enter" if !danger => cx.emit(Answer::Ok),
                    _ => {}
                }
            }))
            .w(px(480.))
            .max_w_full()
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .bg(t.raise)
            .border_1()
            .border_color(t.line)
            .rounded(px(8.))
            .shadow_lg()
            .text_color(t.fg)
            .font_family(t.sans)
            .text_size(px(14.))
            .child(
                div()
                    .text_size(px(16.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.title.clone()),
            )
            .child(div().text_color(t.muted).child(self.text.clone()))
            .children(
                self.field
                    .clone()
                    .map(|f| div().min_h(px(64.)).text_size(px(13.5)).child(f)),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(6.))
                    .child(
                        button(t, "c-no", "Cancelar", false)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Answer::Cancel))),
                    )
                    .child(
                        button(t, "c-ok", &self.ok, !danger)
                            .when(danger, |d| d.text_color(red))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Answer::Ok))),
                    ),
            )
    }
}

/// O `md()` do protótipo para uma linha: `código` em mono.
fn inline(t: &Theme, s: &str) -> Div {
    let mut d = div().flex().flex_wrap().gap_x(px(4.));
    for (i, part) in s.split('`').enumerate() {
        if i % 2 == 1 {
            d = d.child(
                div()
                    .font_family(t.mono.clone())
                    .text_size(px(13.))
                    .px(px(3.))
                    .rounded(px(3.))
                    .bg(t.panel)
                    .child(part.to_string()),
            );
        } else {
            for w in part.split_whitespace() {
                d = d.child(w.to_string());
            }
        }
    }
    d
}

/// O `choiceDlg` do protótipo: título, texto e as opções empilhadas; Enter escolhe a principal.
pub struct Choice {
    title: String,
    text: String,
    options: Vec<(String, bool)>,
    focus: FocusHandle,
    theme: Theme,
}

/// Índice da opção, ou `None` no Esc.
pub struct Chose(pub Option<usize>);

impl EventEmitter<Chose> for Choice {}

impl Choice {
    pub fn new(
        theme: Theme,
        title: String,
        text: String,
        options: Vec<(String, bool)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            title,
            text,
            options,
            focus,
            theme,
        }
    }
}

impl Render for Choice {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let pri = self.options.iter().position(|o| o.1);
        let mut acts = div().flex().flex_col().gap(px(6.));
        for (i, (label, p)) in self.options.iter().enumerate() {
            acts = acts.child(
                button(t, ("ch", i), label, *p)
                    .justify_center()
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(Chose(Some(i))))),
            );
        }
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(move |_, e: &KeyDownEvent, _, cx| {
                match e.keystroke.key.as_str() {
                    "escape" => cx.emit(Chose(None)),
                    "enter" if pri.is_some() => cx.emit(Chose(pri)),
                    _ => {}
                }
            }))
            .w(px(480.))
            .max_w_full()
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .bg(t.raise)
            .border_1()
            .border_color(t.line)
            .rounded(px(8.))
            .shadow_lg()
            .text_color(t.fg)
            .font_family(t.sans)
            .text_size(px(14.))
            .child(
                inline(t, &self.title)
                    .text_size(px(16.))
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(inline(t, &self.text).text_color(t.muted))
            .child(acts)
    }
}

/// `.toast` do protótipo: some sozinho.
pub fn toast(t: &Theme, msg: &str) -> Div {
    div()
        .bg(t.fg)
        .text_color(t.win)
        .px(px(12.))
        .py(px(8.))
        .rounded(px(6.))
        .shadow_lg()
        .text_size(px(13.5))
        .max_w(px(720.))
        .child(div().flex_1().min_w_0().child(msg.to_string()))
}

/// O `mergeDlg` do protótipo: opções que vêm do MR; com o pipeline rodando, só dá para agendar.
pub struct MergeDlg {
    title: String,
    text: String,
    opts: MergeOpts,
    running: bool,
    focus: FocusHandle,
    theme: Theme,
}

/// As opções escolhidas, ou `None` no Cancelar/Esc.
pub struct Merged(pub Option<MergeOpts>);

impl EventEmitter<Merged> for MergeDlg {}

impl MergeDlg {
    pub fn new(
        theme: Theme,
        title: String,
        text: String,
        opts: MergeOpts,
        running: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            title,
            text,
            opts,
            running,
            focus,
            theme,
        }
    }
}

impl Render for MergeDlg {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        let o = self.opts.clone();
        let opt = |id: &'static str, on: bool, label: &'static str, flip: fn(&mut MergeOpts)| {
            div()
                .id(id)
                .flex()
                .gap(px(8.))
                .items_center()
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    flip(&mut this.opts);
                    cx.notify();
                }))
                .child(checkbox(&t, on))
                .child(label)
        };
        let auto = div()
            .flex()
            .gap(px(8.))
            .items_center()
            .when(!self.running, |d| d.opacity(0.5))
            .child(checkbox(&t, self.running))
            .child("Merge quando o pipeline passar")
            .child(div().text_color(t.muted).child(if self.running {
                "(pipeline rodando)"
            } else {
                "(pipeline já passou)"
            }));
        let ok_label = if self.running {
            "Agendar merge Ctrl+⏎"
        } else {
            "Merge Ctrl+⏎"
        };
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                let k = &e.keystroke;
                match k.key.as_str() {
                    "escape" => cx.emit(Merged(None)),
                    "enter" if k.modifiers.control => cx.emit(Merged(Some(this.opts.clone()))),
                    _ => {}
                }
            }))
            .w(px(480.))
            .max_w_full()
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .bg(t.raise)
            .border_1()
            .border_color(t.line)
            .rounded(px(8.))
            .shadow_lg()
            .text_color(t.fg)
            .font_family(t.sans)
            .text_size(px(14.))
            .child(
                div()
                    .text_size(px(16.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.title.clone()),
            )
            .child(div().text_color(t.muted).child(self.text.clone()))
            .child(
                div()
                    .border_1()
                    .border_color(t.line)
                    .rounded(px(6.))
                    .px(px(12.))
                    .py(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(t.muted)
                            .child("Opções (vêm do MR)"),
                    )
                    .child(opt("mg-sq", o.squash, "Squash dos commits", |o| {
                        o.squash = !o.squash
                    }))
                    .child(opt(
                        "mg-rm",
                        o.remove_branch,
                        "Apagar a branch de origem",
                        |o| o.remove_branch = !o.remove_branch,
                    ))
                    .child(auto),
            )
            .child(
                div()
                    .text_color(t.muted)
                    .child("Escreve no GitLab e notifica o autor."),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(6.))
                    .child(
                        button(&t, "mg-no", "Cancelar", false)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Merged(None)))),
                    )
                    .child(button(&t, "mg-ok", ok_label, true).on_click(
                        cx.listener(|this, _, _, cx| cx.emit(Merged(Some(this.opts.clone())))),
                    )),
            )
    }
}
