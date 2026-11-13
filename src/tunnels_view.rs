//! O modo Bancos (`renderDb`/`renderDbLeft` do protótipo): os túneis do zsh, globais ao PC.

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
    time::Instant,
};

use gpui_kit::{prelude::FluentBuilder, *};

use crate::{
    form::{FieldEvent, TextField, button},
    theme::{self, Theme},
    tunnels::{self, Procs, State, Tunnel},
};

pub enum Event {
    Toast(String),
}

#[derive(Clone, Copy)]
struct LeftDrag;

const ENVS: [(&str, &str, &str); 5] = [
    ("all", "Todos", "·"),
    ("up", "De pé", "●"),
    ("dev", "dev", "·"),
    ("prd", "prd", "!"),
    ("func", "func", "·"),
];

pub struct TunnelsView {
    tunnels: Vec<Tunnel>,
    states: Vec<State>,
    since: HashMap<String, Instant>,
    /// subiram pelo "Ligar todos": o resumo já avisou, o "de pé" de cada um não
    quiet: HashSet<String>,
    procs: Procs,
    loaded: bool,
    env: &'static str,
    sel: usize,
    filter: Entity<TextField>,
    left_w: f32,
    focus: FocusHandle,
    theme: Theme,
    _poll: Task<()>,
}

impl EventEmitter<Event> for TunnelsView {}

impl TunnelsView {
    pub fn new(theme: Theme, cx: &mut Context<Self>) -> Self {
        let filter =
            cx.new(|cx| TextField::new(theme.clone(), "", "filtrar  ( / )", false, false, cx));
        cx.subscribe(&filter, |this, _, e: &FieldEvent, cx| {
            if matches!(e, FieldEvent::Changed) {
                this.sel = 0;
            }
            cx.notify();
        })
        .detach();
        // ponytail: porta checada a cada 2 s (connect local, ~0 ms por túnel); inotify não enxerga socket
        let poll = cx.spawn(async move |this, cx| {
            let list = cx
                .background_executor()
                .spawn(async { tunnels::load() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.states = vec![State::Off; list.len()];
                this.tunnels = list;
                this.loaded = true;
                cx.notify();
            });
            let mut first = true;
            loop {
                let Ok((ports, procs)) = this.read_with(cx, |this, _| {
                    (
                        this.tunnels.iter().map(|t| t.port).collect::<Vec<_>>(),
                        this.procs.clone(),
                    )
                }) else {
                    break;
                };
                let up = cx
                    .background_executor()
                    .spawn(async move {
                        ports
                            .into_iter()
                            .map(tunnels::listening)
                            .collect::<Vec<_>>()
                    })
                    .await;
                let boot = std::mem::take(&mut first)
                    && autostart(std::env::var("MAESTRO_HERDR_SESSION").ok().as_deref());
                if this
                    .update(cx, |this, cx| {
                        this.apply(&procs, &up, cx);
                        if boot && !to_start(&this.states).is_empty() {
                            this.start_all(cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        });
        Self {
            tunnels: Vec::new(),
            states: Vec::new(),
            since: HashMap::new(),
            quiet: HashSet::new(),
            procs: Procs::default(),
            loaded: false,
            env: "all",
            sel: 0,
            filter,
            left_w: 250.,
            focus: cx.focus_handle(),
            theme,
            _poll: poll,
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn apply(&mut self, procs: &Procs, up: &[bool], cx: &mut Context<Self>) {
        let mut changed = false;
        let mut fell = Vec::new();
        for (i, t) in self.tunnels.iter().enumerate() {
            let new = procs.state(t, up.get(i).copied().unwrap_or(false));
            let old = &self.states[i];
            if *old == new {
                continue;
            }
            let quiet = *old == State::Starting && self.quiet.remove(&t.name);
            match (&old, &new) {
                (State::Starting, State::Up) if !quiet => {
                    cx.emit(Event::Toast(format!("{} de pé em :{}", t.name, t.port)))
                }
                (_, State::Down(why)) => fell.push((t.name.clone(), why.clone())),
                _ => {}
            }
            if new == State::Up {
                self.since
                    .entry(t.name.clone())
                    .or_insert_with(Instant::now);
            } else {
                self.since.remove(&t.name);
            }
            self.states[i] = new;
            changed = true;
        }
        if let Some(msg) = fell_toast(&fell) {
            cx.emit(Event::Toast(msg));
        }
        if changed {
            cx.notify();
        }
    }

    pub fn up(&self) -> usize {
        self.states.iter().filter(|s| **s == State::Up).count()
    }

    pub fn down(&self) -> usize {
        self.states
            .iter()
            .filter(|s| matches!(s, State::Down(_)))
            .count()
    }

    /// (nome, porta, de pé, prd) para o seletor do Ctrl+Shift+D.
    pub fn items(&self) -> Vec<(String, u16, bool, bool)> {
        self.tunnels
            .iter()
            .zip(&self.states)
            .map(|(t, s)| (t.name.clone(), t.port, *s == State::Up, t.env() == "prd"))
            .collect()
    }

    pub fn toggle(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(i) = self.tunnels.iter().position(|t| t.name == name) else {
            return;
        };
        let t = self.tunnels[i].clone();
        let r = if self.states[i] == State::Up {
            self.procs.stop(&t).map(|_| {
                self.states[i] = State::Off;
                self.since.remove(&t.name);
                format!("{} desligado", t.name)
            })
        } else {
            self.procs.start(&t).map(|_| {
                self.states[i] = State::Starting;
                format!("Subindo {} em :{}…", t.name, t.port)
            })
        };
        cx.emit(Event::Toast(
            r.unwrap_or_else(|e| format!("{}: {e}", t.name)),
        ));
        cx.notify();
    }

    pub fn stop_all(&mut self, cx: &mut Context<Self>) {
        let names: Vec<String> = self
            .tunnels
            .iter()
            .zip(&self.states)
            .filter(|(_, s)| **s == State::Up)
            .map(|(t, _)| t.name.clone())
            .collect();
        for n in &names {
            self.toggle(n, cx);
        }
    }

    pub fn start_all(&mut self, cx: &mut Context<Self>) {
        let xs = to_start(&self.states);
        if self.tunnels.is_empty() {
            return;
        }
        if xs.is_empty() {
            cx.emit(Event::Toast("Todos os bancos já estão de pé".into()));
            return;
        }
        let (mut ok, mut errs) = (0, Vec::new());
        for i in xs.iter().copied() {
            let t = self.tunnels[i].clone();
            match self.procs.start(&t) {
                Ok(()) => {
                    self.states[i] = State::Starting;
                    self.quiet.insert(t.name);
                    ok += 1;
                }
                Err(e) => errs.push(format!("{}: {e}", t.name)),
            }
        }
        let already = self.tunnels.len() - xs.len();
        let mut msg = format!("Subindo {ok}");
        if already > 0 {
            msg.push_str(&format!(" · {already} já de pé"));
        }
        for e in errs {
            msg.push('\n');
            msg.push_str(&e);
        }
        cx.emit(Event::Toast(msg));
        cx.notify();
    }

    fn shown(&self, cx: &App) -> Vec<usize> {
        let q = self.filter.read(cx).value.to_lowercase();
        (0..self.tunnels.len())
            .filter(|&i| {
                let (t, s) = (&self.tunnels[i], &self.states[i]);
                let env = match self.env {
                    "all" => true,
                    "up" => *s != State::Off,
                    e => t.env() == e,
                };
                env && (q.is_empty() || t.name.to_lowercase().contains(&q))
            })
            .collect()
    }

    fn on_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        if k.modifiers.control || k.modifiers.alt || !self.focus.is_focused(window) {
            if k.key == "escape" {
                self.filter.update(cx, |f, cx| f.set(String::new(), cx));
                window.focus(&self.focus, cx);
            }
            return;
        }
        let xs = self.shown(cx);
        match k.key.as_str() {
            "j" | "down" if !xs.is_empty() => self.sel = (self.sel + 1).min(xs.len() - 1),
            "k" | "up" => self.sel = self.sel.saturating_sub(1),
            "enter" => {
                if let Some(&i) = xs.get(self.sel) {
                    let n = self.tunnels[i].name.clone();
                    self.toggle(&n, cx);
                }
            }
            "/" => {
                let f = self.filter.read(cx).focus_handle();
                window.focus(&f, cx);
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn note(&self, text: impl Into<SharedString>) -> Div {
        div()
            .px(px(12.))
            .py(px(6.))
            .text_size(px(12.))
            .text_color(self.theme.muted)
            .child(text.into())
    }

    fn left(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = self.theme.clone();
        let count = |e: &str| {
            self.tunnels
                .iter()
                .zip(&self.states)
                .filter(|(x, s)| match e {
                    "all" => true,
                    "up" => **s != State::Off,
                    e => x.env() == e,
                })
                .count()
        };
        let mut rows = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .px(px(6.))
            .pt(px(2.))
            .pb(px(6.));
        for (k, label, g) in ENVS {
            let on = self.env == k;
            rows = rows.child(
                div()
                    .id(k)
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(t.accent_soft))
                    .when(!on, |d| d.hover(|d| d.bg(t.raise)))
                    .child(
                        div()
                            .w(px(16.))
                            .font_family(t.mono.clone())
                            .text_color(t.comment)
                            .child(g),
                    )
                    .child(div().flex_1().font_family(t.mono.clone()).child(label))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(t.muted)
                            .child(count(k).to_string()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.env = k;
                        this.sel = 0;
                        cx.notify();
                    })),
            );
        }
        div()
            .id("db-left")
            .w(px(self.left_w))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .bg(t.panel)
            .border_r_1()
            .border_color(t.line)
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_baseline()
                    .px(px(12.))
                    .pt(px(10.))
                    .pb(px(6.))
                    .child(div().font_weight(FontWeight::MEDIUM).child("Bancos"))
                    .child(
                        div()
                            .font_family(t.mono.clone())
                            .text_size(px(12.))
                            .text_color(t.muted)
                            .child("aliases do zsh"),
                    ),
            )
            .child(rows)
            .child(self.note(
                "Todo alias do zsh que roda cloud-sql-proxy ou kubectl port-forward. Editar ou criar túnel é no zsh; aqui só liga e desliga.",
            ))
            .child(self.note(
                "De pé vale para o PC inteiro: qualquer repo, cliente SQL ou feature conecta em localhost:porta. Fechar o Maestro não derruba os túneis.",
            ))
    }

    fn row(&self, k: usize, i: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = &self.theme;
        let (x, s) = (&self.tunnels[i], &self.states[i]);
        let red: Hsla = rgb(0xff5555).into();
        let (glyph, color) = match s {
            State::Up => ("●", t.ok),
            State::Down(_) => ("●", red),
            State::Starting => ("◐", rgb(0xf1fa8c).into()),
            State::Off => ("○", t.comment),
        };
        let mid = match s {
            State::Down(why) => div().text_color(red).child(why.clone()),
            State::Starting => div().text_color(t.muted).child("subindo…"),
            _ => div()
                .font_family(t.mono.clone())
                .text_color(t.muted)
                .child(x.target.clone()),
        };
        let since = self
            .since
            .get(&x.name)
            .map(|at| crate::sidebar::ago(at.elapsed()))
            .unwrap_or_default();
        let (name, port, log) = (x.name.clone(), x.port, x.log());
        let up = *s == State::Up;
        div()
            .id(("db", i))
            .flex()
            .gap(px(12.))
            .items_center()
            .px(px(12.))
            .py(px(8.))
            .border_b_1()
            .border_color(t.line)
            .when(k == self.sel, |d| d.bg(t.accent_soft))
            .hover(|d| d.bg(t.raise))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.sel = k;
                cx.notify();
            }))
            .child(
                div()
                    .w(px(14.))
                    .flex_none()
                    .font_family(t.mono.clone())
                    .text_color(color)
                    .child(glyph),
            )
            .child(
                div()
                    .w(px(220.))
                    .flex_none()
                    .flex()
                    .gap(px(6.))
                    .items_center()
                    .font_family(t.mono.clone())
                    .text_size(px(13.))
                    .child(div().truncate().child(x.name.clone()))
                    .when(x.env() == "prd", |d| {
                        d.child(
                            div()
                                .text_size(px(11.))
                                .px(px(6.))
                                .rounded(px(10.))
                                .border_1()
                                .border_color(red)
                                .text_color(red)
                                .child("prd"),
                        )
                    }),
            )
            .child(mid.flex_1().min_w_0().truncate())
            .child(
                div()
                    .w(px(60.))
                    .flex_none()
                    .font_family(t.mono.clone())
                    .text_size(px(12.))
                    .child(format!(":{}", x.port)),
            )
            .child(
                div()
                    .w(px(80.))
                    .flex_none()
                    .text_size(px(12.))
                    .text_color(t.muted)
                    .child(since),
            )
            .child(
                div()
                    .w(px(200.))
                    .flex_none()
                    .flex()
                    .gap(px(6.))
                    .justify_end()
                    .when(up, |d| {
                        d.child(
                            button(t, ("dbcp", i), "Copiar host", false).on_click(cx.listener(
                                move |_, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(format!(
                                        "localhost:{port}"
                                    )));
                                    cx.emit(Event::Toast(format!("Copiado: localhost:{port}")));
                                },
                            )),
                        )
                    })
                    .when(matches!(s, State::Down(_)), |d| {
                        d.child(button(t, ("dblog", i), "Ver log", false).on_click(
                            move |_, _, _| {
                                crate::spawn_reaped(
                                    std::process::Command::new("xdg-open").arg(&log),
                                );
                            },
                        ))
                    })
                    .when(*s != State::Starting, |d| {
                        d.child(
                            button(t, ("dbt", i), if up { "Desligar" } else { "Ligar" }, !up)
                                .when(up, |b| b.text_color(red))
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.toggle(&name, cx)),
                                ),
                        )
                    }),
            )
    }

    fn center(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let up = self.up();
        let bar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .py(px(8.))
            .border_b_1()
            .border_color(t.line)
            .child(div().w(px(220.)).child(self.filter.clone()))
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(t.muted)
                    .child(format!("{up} de pé · log em ~/.cache/sql-proxy/")),
            )
            .when(!to_start(&self.states).is_empty(), |d| {
                d.child(
                    button(&t, "db-on", "Ligar todos", true)
                        .on_click(cx.listener(|this, _, _, cx| this.start_all(cx))),
                )
            })
            .when(up > 0, |d| {
                d.child(
                    button(&t, "db-off", "Desligar todos", false)
                        .text_color(rgb(0xff5555))
                        .on_click(cx.listener(|this, _, _, cx| this.stop_all(cx))),
                )
            });
        let xs = self.shown(cx);
        let mut list = div().id("db-list").flex_1().min_h_0().overflow_y_scroll();
        if !self.loaded {
            list = list.child(
                div()
                    .p(px(24.))
                    .text_color(t.muted)
                    .child("Lendo os aliases do zsh…"),
            );
        } else if self.tunnels.is_empty() {
            list = list.child(self.note(
                "Nenhum alias de túnel no zsh. Crie um alias que rode cloud-sql-proxy ou kubectl port-forward (ex.: alias sql_app_dev='cloud-sql-proxy proj:região:instância --port 5433') e reabra o Maestro.",
            ));
        } else if xs.is_empty() {
            list = list.child(self.note("Nenhum banco bate com o filtro. Esc limpa."));
        }
        for (k, i) in xs.into_iter().enumerate() {
            list = list.child(self.row(k, i, cx));
        }
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(bar)
            .child(list)
    }
}

impl Render for TunnelsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .bg(t.win)
            .text_color(t.fg)
            .font_family(t.sans)
            .text_size(px(14.))
            .on_drag_move(cx.listener(|this, e: &DragMoveEvent<LeftDrag>, _, cx| {
                let (b, x) = (e.bounds, e.event.position.x);
                let max = (f32::from(b.size.width) - 300.).max(180.);
                this.left_w = f32::from(x - b.left()).clamp(180., max);
                cx.notify();
            }))
            .child(self.left(cx))
            .child(theme::resize_handle("rz-db", LeftDrag, &t))
            .child(self.center(cx))
    }
}

fn to_start(states: &[State]) -> Vec<usize> {
    states
        .iter()
        .enumerate()
        .filter(|(_, s)| matches!(s, State::Off | State::Down(_)))
        .map(|(i, _)| i)
        .collect()
}

/// Credencial do gcloud vencida derruba todos de uma vez: um toast só, não um por túnel.
fn fell_toast(fell: &[(String, String)]) -> Option<String> {
    match fell {
        [] => None,
        [(name, why)] => Some(format!("{name} caiu: {why}")),
        [(_, why), ..] => Some(format!("{} caíram: {why}", fell.len())),
    }
}

/// A sessão de teste (`MAESTRO_HERDR_SESSION`) nunca sobe túnel sozinha: poderia abrir proxy de prd.
fn autostart(session: Option<&str>) -> bool {
    session.is_none()
}

#[cfg(test)]
mod tests {
    use super::{State, autostart, fell_toast, to_start};

    #[test]
    fn ligar_todos_pula_quem_esta_de_pe_ou_subindo() {
        let s = [
            State::Up,
            State::Off,
            State::Starting,
            State::Down("caiu".into()),
            State::Off,
        ];
        assert_eq!(to_start(&s), [1, 3, 4]);
        assert!(to_start(&[State::Up, State::Starting]).is_empty());
    }

    #[test]
    fn quedas_da_mesma_checagem_viram_um_toast() {
        let why = "could not find default credentials".to_string();
        assert_eq!(fell_toast(&[]), None);
        assert_eq!(
            fell_toast(&[("sql_a".into(), why.clone())]).unwrap(),
            "sql_a caiu: could not find default credentials"
        );
        assert_eq!(
            fell_toast(&[("sql_a".into(), why.clone()), ("sql_b".into(), why)]).unwrap(),
            "2 caíram: could not find default credentials"
        );
    }

    #[test]
    fn sessao_de_teste_nao_sobe_sozinha() {
        assert!(autostart(None));
        assert!(!autostart(Some("maestro-test")));
    }
}
