use std::{collections::HashSet, path::PathBuf, time::Duration};

use gpui_kit::{prelude::FluentBuilder, *};

use crate::{
    feature,
    form::button,
    mr::{self, Fetch, Status},
    preflight::{self, Check, State},
    theme::Theme,
    tour::{self, Anchors},
};

pub enum Event {
    /// (texto para o Claude da feature, toast quando chegar)
    Ask(String, String),
    Conclude,
    Toast(String),
    /// o MR do repo abriu enquanto a aba existia (iid)
    Opened(u64),
}

/// A aba MR da feature (`renderMRTab` do protótipo), com o pré-voo antes do `/mr`.
pub struct MrTab {
    root: PathBuf,
    branch: String,
    base: String,
    feature_id: Option<String>,
    pub fetch: Option<Fetch>,
    /// o vigia deste MR (lido do estado que o shell grava)
    watch: Option<crate::mr_watch::Watch>,
    checks: Option<Vec<Check>>,
    sent: HashSet<u64>,
    theme: Theme,
    anchors: Anchors,
    _poll: Task<()>,
}

impl EventEmitter<Event> for MrTab {}

fn red() -> Hsla {
    rgb(0xff5555).into()
}

fn yellow() -> Hsla {
    rgb(0xf1fa8c).into()
}

impl MrTab {
    pub fn new(
        root: PathBuf,
        owner: PathBuf,
        branch: String,
        theme: Theme,
        anchors: Anchors,
        cx: &mut Context<Self>,
    ) -> Self {
        // ponytail: relê o MR a cada 20 s com a aba aberta; webhook não chega numa máquina local
        let poll = cx.spawn({
            let (root, branch) = (root.clone(), branch.clone());
            async move |this, cx| loop {
                let (r, b) = (root.clone(), branch.clone());
                let (f, w) = cx
                    .background_executor()
                    .spawn(async move {
                        let f = mr::fetch(&r, &b);
                        let w = match &f {
                            Fetch::Mr(m) => crate::mr_watch::State::key_of(&m.web_url, m.iid)
                                .and_then(|k| crate::mr_watch::State::load().get(&k).cloned()),
                            _ => None,
                        };
                        (f, w)
                    })
                    .await;
                if this
                    .update(cx, |this, cx| {
                        // o vigia também manda comentário ao Claude: o que ele mandou fica "enviado ao Claude"
                        if let Some(id) = &this.feature_id {
                            let n = this.sent.len();
                            this.sent.extend(
                                feature::state_lines(id, "sent")
                                    .iter()
                                    .filter_map(|l| l.parse::<u64>().ok()),
                            );
                            if this.sent.len() != n {
                                cx.notify();
                            }
                        }
                        if this.watch != w {
                            this.watch = w;
                            cx.notify();
                        }
                        if this.fetch.as_ref() != Some(&f) {
                            if let (Some(Fetch::NoMr), Fetch::Mr(m)) = (&this.fetch, &f) {
                                cx.emit(Event::Opened(m.iid));
                            }
                            this.fetch = Some(f);
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_secs(20))
                    .await;
            }
        });
        // o carimbo do pré-voo é da feature do Claude (o hook do /mr roda nela), mesmo para outro repo
        let inv = feature::find_by_path(&owner);
        let sent = inv.as_ref().map_or_else(HashSet::new, |i| {
            feature::state_lines(&i.id, "sent")
                .iter()
                .filter_map(|l| l.parse().ok())
                .collect()
        });
        Self {
            base: inv
                .as_ref()
                .filter(|_| owner == root)
                .map(|i| i.base.clone())
                .unwrap_or_else(|| {
                    crate::diff::base(&root)
                        .map(|b| b.trim_start_matches("origin/").to_string())
                        .unwrap_or_else(|| "main".into())
                }),
            feature_id: inv.map(|i| i.id),
            root,
            branch,
            fetch: None,
            watch: None,
            checks: None,
            sent,
            theme,
            anchors,
            _poll: poll,
        }
    }

    fn preflight(&mut self, cx: &mut Context<Self>) {
        self.checks = Some(vec![
            Check {
                state: State::Run,
                detail: String::new(),
                log: String::new(),
            };
            preflight::ITEMS.len()
        ]);
        for i in 0..preflight::ITEMS.len() {
            let (root, base) = (self.root.clone(), self.base.clone());
            cx.spawn(async move |this, cx| {
                let c = cx
                    .background_executor()
                    .spawn(async move { preflight::run(i, &root, &base) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(v) = this.checks.as_mut() {
                        v[i] = c;
                    }
                    if this.done() && !this.failed() {
                        this.send_mr(None, cx);
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }

    pub fn has_preflight(&self) -> bool {
        self.checks.is_some()
    }

    fn done(&self) -> bool {
        self.checks
            .as_ref()
            .is_some_and(|v| v.iter().all(|c| c.state != State::Run))
    }

    fn failed(&self) -> bool {
        self.checks
            .as_ref()
            .is_some_and(|v| v.iter().any(|c| c.state == State::Fail))
    }

    /// Carimba o pré-voo (o hook libera o `/mr` para estes arquivos) e manda o `/mr`.
    fn send_mr(&mut self, overrides: Option<Vec<String>>, cx: &mut Context<Self>) {
        if let Some(id) = &self.feature_id
            && let Err(e) = preflight::stamp(id, &self.root)
        {
            return cx.emit(Event::Toast(format!(
                "Não deu para registrar o pré-voo: {e}"
            )));
        }
        let text = match overrides {
            None => "/mr".to_string(),
            Some(items) => format!(
                "/mr Registre na descrição do MR que o pré-voo passou com override em: {}.",
                items.join("; ")
            ),
        };
        self.checks = None;
        cx.emit(Event::Ask(
            text,
            format!("/mr enviado ao Claude de {}", self.branch),
        ));
    }

    fn card(&self) -> Div {
        let t = &self.theme;
        div()
            .border_1()
            .border_color(t.line)
            .rounded(px(6.))
            .bg(t.raise)
            .px(px(12.))
            .py(px(10.))
            .flex()
            .flex_col()
            .gap(px(6.))
    }

    /// O `watchCard` do protótipo: o vigia que manda pipe, threads e achados ao agente do MR.
    fn watch_card(&self, m: &mr::Mr) -> Option<Div> {
        let w = self.watch.as_ref().filter(|_| !m.merged && !m.closed)?;
        let t = &self.theme;
        let proj = crate::mr_watch::State::key_of(&m.web_url, m.iid)?
            .rsplit_once('!')?
            .0
            .rsplit('/')
            .next()?
            .to_string();
        let (pipe, pc) = match m.pipe {
            Status::Ok => ("pipe ✓", t.ok),
            Status::Fail => ("pipe ✗", red()),
            Status::Running => ("pipe ⟳", yellow()),
            Status::None => ("sem pipe", t.muted),
        };
        let rev = match (w.reviewed, w.found) {
            (false, _) => "revisão espera o verde".to_string(),
            (true, None) => "revisando ⟳".to_string(),
            (true, Some(0)) => "revisão sem achados".to_string(),
            (true, Some(n)) => format!(
                "revisão: {n} achado{} enviado{}",
                if n > 1 { "s" } else { "" },
                if n > 1 { "s" } else { "" }
            ),
        };
        let s = if w.threads.len() == 1 { "" } else { "s" };
        Some(
            self.card()
                .child(
                    self.h4("")
                        .child(if w.gave_up {
                            div().text_color(red()).child("VIGIA PARADO")
                        } else {
                            div().flex().gap(px(6.)).child(div().text_color(t.ok).child("◉")).child("CLAUDE VIGIANDO")
                        })
                        .child(div().flex_1())
                        .child(div().font_family(t.mono.clone()).child(format!("agente mr-{proj}-{}", m.iid))),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(4.))
                        .child(div().text_color(pc).child(pipe))
                        .child(format!("· {} thread{s} respondida{s} · {rev}", w.threads.len())),
                )
                .when(!w.log.is_empty(), |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .font_family(t.mono.clone())
                            .text_size(px(12.))
                            .text_color(t.muted)
                            .children(w.log.iter().map(|l| div().child(l.clone()))),
                    )
                })
                .child(div().text_size(px(12.)).text_color(if w.gave_up { red() } else { t.muted }).child(if w.gave_up {
                    "O mesmo job falhou 3 vezes seguidas: parei de mandar ao Claude. O botão do /pipe continua aí."
                } else {
                    "Pipe que falha, thread nova e a revisão depois do verde vão sozinhos ao Claude, que corrige, faz push e responde na thread sem pedir confirmação."
                })),
        )
    }

    fn h4(&self, title: impl Into<SharedString>) -> Div {
        let t = &self.theme;
        div()
            .flex()
            .gap(px(8.))
            .items_center()
            .text_size(px(13.))
            .text_color(t.muted)
            .font_weight(FontWeight::MEDIUM)
            .child(title.into().to_uppercase())
    }

    fn empty(&self, title: String, text: String) -> Div {
        let t = &self.theme;
        div()
            .m_auto()
            .max_w(px(360.))
            .p(px(24.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .text_center()
            .text_color(t.muted)
            .child(div().text_size(px(16.)).text_color(t.fg).child(title))
            .child(text)
    }

    fn checks_card(&self, cx: &mut Context<Self>) -> Option<Div> {
        let t = self.theme.clone();
        let checks = self.checks.as_ref()?;
        let mut card = self.card().child(self.h4("Pré-voo"));
        for (name, c) in preflight::ITEMS.iter().zip(checks) {
            let (g, color) = match c.state {
                State::Run => ("⟳", yellow()),
                State::Pass => ("✓", t.ok),
                State::Fail => ("✗", red()),
                State::Skip => ("–", t.muted),
            };
            card = card.child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(div().w(px(12.)).flex_none().text_color(color).child(g))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .child(
                                div()
                                    .text_color(if c.state == State::Fail { red() } else { t.fg })
                                    .child(name.to_string()),
                            )
                            .when(!c.detail.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_size(px(13.))
                                        .text_color(t.muted)
                                        .font_family(t.mono.clone())
                                        .truncate()
                                        .child(c.detail.clone()),
                                )
                            }),
                    ),
            );
        }
        let skipped = checks.iter().any(|c| c.state == State::Skip);
        Some(
            card.when(skipped, |d| {
                d.child(
                    div()
                        .text_size(px(13.))
                        .text_color(t.muted)
                        .child("– não deu para checar: não barra o /mr."),
                )
            })
            .when(self.done() && self.failed(), |d| {
                let items: Vec<String> = preflight::ITEMS
                    .iter()
                    .zip(checks)
                    .filter(|(_, c)| c.state == State::Fail)
                    .map(|(n, c)| format!("{n} ({})", c.detail))
                    .collect();
                let fix = preflight::fix_prompt(
                    &preflight::ITEMS
                        .iter()
                        .zip(checks)
                        .filter(|(_, c)| c.state == State::Fail)
                        .map(|(n, c)| (*n, c))
                        .collect::<Vec<_>>(),
                );
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(8.))
                        .items_center()
                        .child(
                            button(&t, "pf-fix", "Pedir ao Claude para corrigir", true).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.checks = None;
                                    cx.emit(Event::Ask(
                                        fix.clone(),
                                        format!(
                                            "Falhas do pré-voo enviadas ao Claude de {}",
                                            this.branch
                                        ),
                                    ));
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(
                            button(&t, "pf-again", "Rodar de novo", false)
                                .on_click(cx.listener(|this, _, _, cx| this.preflight(cx))),
                        )
                        .child(
                            button(&t, "pf-override", "Abrir mesmo assim", false)
                                .text_color(red())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.send_mr(Some(items.clone()), cx)
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(t.muted)
                                .child("o override vai registrado na descrição do MR"),
                        ),
                )
            }),
        )
    }

    fn mr(&self, m: &mr::Mr, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let line = |s: Status, ok: &str, fail: Div, run: &str, none: &str| {
            let (c, d) = match s {
                Status::Ok => (t.ok, div().child(ok.to_string())),
                Status::Fail => (red(), fail),
                Status::Running => (yellow(), div().child(run.to_string())),
                Status::None => (t.muted, div().child(none.to_string())),
            };
            d.text_color(c)
        };
        let pend: Vec<&mr::Comment> = m
            .comments
            .iter()
            .filter(|c| !self.sent.contains(&c.id))
            .collect();
        let url = m.web_url.clone();
        let mut comments = self.card().child(
            self.h4("Comentários de revisores")
                .child(div().flex_1())
                .when(!pend.is_empty(), |d| {
                    let (iid, ids): (u64, Vec<u64>) = (m.iid, pend.iter().map(|c| c.id).collect());
                    d.child(
                        button(
                            &t,
                            "mr-com",
                            &format!("Mandar {} pro Claude (/comentario)", pend.len()),
                            true,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.sent.extend(ids.iter().copied());
                            if let Some(id) = &this.feature_id {
                                for c in &ids {
                                    feature::add_state_line(id, "sent", &c.to_string());
                                }
                            }
                            cx.emit(Event::Ask(
                                format!("/comentario !{iid}"),
                                "Comentários enviados ao Claude".into(),
                            ));
                            cx.notify();
                        })),
                    )
                }),
        );
        if m.comments.is_empty() {
            comments = comments.child(div().text_color(t.muted).child("Nenhum ainda."));
        }
        for c in &m.comments {
            let sent = self.sent.contains(&c.id);
            comments = comments.child(
                div()
                    .border_1()
                    .border_color(t.line)
                    .rounded(px(6.))
                    .when(sent, |d| d.opacity(0.7))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .px(px(10.))
                            .py(px(6.))
                            .border_b_1()
                            .border_color(t.line)
                            .text_size(px(13.))
                            .text_color(t.muted)
                            .child(
                                div()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(t.fg)
                                    .child(c.who.clone()),
                            )
                            .child(div().font_family(t.mono.clone()).child(c.at.clone()))
                            .child(div().flex_1())
                            .when(sent, |d| d.child("enviado ao Claude")),
                    )
                    .child(div().px(px(10.)).py(px(8.)).child(c.body.clone())),
            );
        }
        let job = m.job.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                self.card()
                    .relative()
                    .child(tour::anchor(&self.anchors, "mrcard"))
                    .child(
                        self.h4(format!("!{} · {} → {}", m.iid, self.branch, m.target))
                            .child(div().flex_1())
                            .child(button(&t, "mr-web", "Abrir no GitLab", false).on_click(move |_, _, _| {
                                crate::spawn_reaped(std::process::Command::new("xdg-open").arg(&url));
                            })),
                    )
                    .when(m.merged, |d| {
                        d.child(
                            div()
                                .text_color(t.ok)
                                .child("Mergeado. Histórico do vault atualizado."),
                        )
                    })
                    .when(m.closed, |d| d.child(div().text_color(t.muted).child("Fechado sem merge."))),
            )
            .children(self.watch_card(m))
            .child(
                self.card()
                    .child(self.h4("Pipeline"))
                    .child(line(
                        m.pipe,
                        "✓ passou",
                        div()
                            .flex()
                            .gap(px(4.))
                            .child("✗ falhou no job")
                            .child(div().font_family(t.mono.clone()).child(job.as_ref().map_or(String::new(), |j| j.0.clone()))),
                        "⟳ rodando",
                        "– sem pipeline",
                    ))
                    .when_some(job.filter(|_| m.pipe == Status::Fail), |d, (_, link)| {
                        d.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap(px(8.))
                                .items_center()
                                .child(button(&t, "mr-pipe", "Diagnosticar com /pipe", true).on_click(cx.listener(
                                    move |_, _, _, cx| cx.emit(Event::Ask(format!("/pipe {link}"), "/pipe enviado ao Claude".into())),
                                )))
                                .child(div().text_color(t.muted).child("o Claude lê o log do job; nada é alterado sem você")),
                        )
                    }),
            )
            .child(
                self.card()
                    .child(self.h4("Sonar · quality gate"))
                    .child(line(
                        m.sonar,
                        "✓ passou",
                        div().child("✗ o quality gate reprovou (job sonar)"),
                        "⟳ aguardando análise",
                        "– sem Sonar neste pipeline",
                    ))
                    .when(m.sonar == Status::Fail, |d| {
                        d.child(button(&t, "mr-sonar", "Pedir ao Claude para cobrir", false).on_click(cx.listener(
                            |_, _, _, cx| {
                                cx.emit(Event::Ask(
                                    "cria testes para cobrir o código novo até o gate do Sonar passar (80%)".into(),
                                    "Pedido enviado ao Claude".into(),
                                ))
                            },
                        )))
                    }),
            )
            .child(comments)
            .when(m.merged, |d| {
                d.child(
                    div().child(
                        button(&t, "mr-rm", "Remover worktree", false)
                            .text_color(red())
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Conclude))),
                    ),
                )
            })
    }
}

impl Render for MrTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        let body = match self.fetch.clone() {
            None => div().p(px(24.)).text_color(t.muted).child("Lendo o MR…"),
            Some(Fetch::NoGitlab) => self.empty(
                "Sem GitLab neste repositório".into(),
                "O remoto origin não é do GitLab: aqui não há MR, pipeline nem Sonar para mostrar."
                    .into(),
            ),
            Some(Fetch::NoAuth) => self.empty(
                "GitLab sem login".into(),
                "Rode glab auth login no terminal; a aba atualiza sozinha.".into(),
            ),
            Some(Fetch::Err(e)) => self.empty("Não deu para ler o MR".into(), e),
            Some(Fetch::NoMr) => {
                let running = self.checks.is_some() && !self.done();
                div()
                    .m_auto()
                    .w_full()
                    .max_w(px(560.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(self.empty(
                        format!("{} ainda não tem MR", self.branch),
                        "O /mr commita, faz push e abre o MR em draft. Depois disso o pipeline, o Sonar e os comentários aparecem aqui.".into(),
                    ).m_0().mx_auto().pb_0())
                    .children(self.checks_card(cx))
                .when(self.checks.is_none() || running, |d| {
                    d.child(
                        div().flex().justify_center().relative().child(tour::anchor(&self.anchors, "mrcard")).child(
                            button(&t, "mr-open", if running { "Pré-voo rodando…" } else { "Abrir MR draft com /mr" }, true)
                                .when(running, |b| b.opacity(0.45))
                                .when(!running, |b| b.on_click(cx.listener(|this, _, _, cx| this.preflight(cx)))),
                        ),
                    )
                })
            }
            Some(Fetch::Mr(m)) => self.mr(&m, cx),
        };
        div()
            .id("mr-tab")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(12.))
            .flex()
            .flex_col()
            .font_family(t.sans)
            .text_size(px(14.))
            .child(body)
    }
}
