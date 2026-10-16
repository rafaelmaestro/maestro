# Checklist do Maestro

Trabalho em etapas. Uma etapa só começa quando a anterior passou no **Pronto quando**.
Cada item é marcado `[x]` só depois de verificado, não quando o código compila.
Detalhes de cada item: [docs/DESIGN.md](docs/DESIGN.md). Protótipo de UX: [prototype/maestro.html](prototype/maestro.html).

Regras:
- Menor diff que funciona. Nada de abstração "para depois".
- Toda lógica não trivial deixa um teste que quebra se ela quebrar.
- Commits sem coautoria do Claude.
- Tudo que o Maestro cria (worktree, branch, ref, porta, projeto do compose) fica registrado e tem caminho de limpeza.

---

## Etapa 0: Fundação

- [x] `rustup` estável instalado (rustc 1.99.0)
- [x] ambiente reproduzível: `scripts/deps.sh` aqui + `install.sh` do [dotfiles](https://github.com/rafaelmaestro/dotfiles)
- [x] libs de sistema do GPUI instaladas (`scripts/deps.sh`); `vulkaninfo` responde (Intel RPL-S, Mesa)
- [x] crate com `gpui-kit =0.7.0` (traz o gpui e o gpui-component já casados) e `Cargo.lock` versionado
- [x] janela vazia abre com o fundo Dracula
- [x] baseline (`MAESTRO_STARTUP=1`, build dev com dependências em opt-level 2): primeiro frame em ~100 ms (915 ms na primeira execução, com o cache de shaders frio); RSS ~100 MB
- [x] CI no GitHub Actions: `cargo fmt --check`, `clippy -D warnings`, `cargo test` (1ª execução sem cache: 26 min)
- [ ] ~~config.toml~~ adiado: entra quando existir a primeira opção configurável

**Pronto quando:** `cargo run` abre a janela em menos de 300 ms e o CI passa. ✅ concluída em 2026-10-15

## Etapa 1: Terminal (o maior risco)

- [x] PTY rodando `$SHELL` com o env do usuário (o `tty` + `EventLoop` do próprio `alacritty_terminal`; sem `portable-pty`)
- [x] `alacritty_terminal` + renderer de grid no GPUI: 16, 256 e truecolor; bold, italic e underline; cursor block (verificado com screenshot: starship, eza com ícones, ligaduras)
- [x] input: teclas e modificadores, colar (Ctrl+Shift+V, Shift+Insert, botão do meio) com bracketed paste, rolagem do histórico (roda e Shift+PageUp/Down), seleção (simples, palavra com 2 cliques, linha com 3) e cópia (Ctrl+Shift+C e copy-on-select na PRIMARY)
- [x] mouse reporting SGR e X10 para os apps, com arrasto (1002) e movimento sem botão (1003); validado com clique real no herdr
- [x] acentos e teclas mortas no layout `us intl` (ç, é, ã): exigiu patch em `vendor/xim-ctext` (o ibus manda a pré-edição em Latin-1 e a crate derrubava o processo)
- [x] resize propaga para o PTY (SIGWINCH): a grade se ajusta ao tamanho da janela
- [x] parser do `~/.config/ghostty/config` com teste: `font-family`, `font-size`, `font-feature`, `palette`, `background`, `foreground`, `selection-*`, `cursor-*`, `window-padding-*` e `background-opacity`. O `keybind` fica para a Etapa 2 (binds de split viram ações do herdr). O `unfocused-split-opacity` não se aplica: os splits são do herdr, dentro de um terminal só
- [x] grade em pixels inteiros como a do Ghostty (13 pt a 96 DPI: célula 10×22, glifo no tamanho real); cursor vazado sem foco. Conferido com screenshot lado a lado de um Ghostty isolado: colunas alinhadas
- [x] ligaduras e glifos da Nerd Font
- [x] regra de atalhos: com o foco no terminal, Ctrl+tecla vai para o PTY; Ctrl+Shift+letra (e enter, `\`, setas) é do Maestro. Ctrl+Shift+- e Ctrl+Shift+2 seguem como `^_` e `^@`. Conferido com uma sonda de bytes no PTY
- [x] `herdr`, `vim`, `btop` e `lazygit` rodam sem artefato visual (screenshot contra um Ghostty isolado; o braille do btop é desenhado como no Ghostty, não pela fonte)
- [x] Tab e Shift+Tab chegam ao shell (o Root do gpui-kit ligava o Tab à navegação de foco e o autocomplete não funcionava)
- [x] respostas ao app: cores (OSC 10/11/4), tamanho em pixels (CSI 14 t), cópia por OSC 52 (só escrita); variáveis `HERDR_*` removidas para o herdr rodar mesmo com o Maestro aberto de dentro de um pane
- [ ] (por último, opcional) aproximação do `cursor_blaze`

**Pronto quando:** num screenshot lado a lado, Ghostty e Maestro mostram `ls`, `git diff`, o prompt do starship e o `btop` iguais, com diferença só no shader do cursor. Digitar não tem atraso perceptível (medir tecla → pixel).

**Resultado (2026-10-15):** `ls`, `git diff`, prompt do starship, `btop` e `lazygit` iguais ao Ghostty, com duas diferenças conhecidas: o shader do cursor e o tamanho de alguns ícones da Nerd Font, porque o Ghostty encolhe o ícone para caber na célula (ex.: o logo do Ubuntu no starship). Latência medida no build release: da tecla até o frame montado, com eco do zsh, mediana de 5,4 ms (máx. 9,3 ms), mais até um frame para aparecer na tela.

## Etapa 2: herdr + lateral de Features + paleta

- [x] cliente do socket do herdr (protocolo 19): uma conexão por request (o servidor fecha depois de responder). Status de agente não tem evento global, só `pane.agent_status_changed` por pane; a lateral faz polling de `agent.list` a cada 500 ms (`ponytail:`), o que já cobre a reconexão
- [x] shell igual ao protótipo aprovado (conferido lado a lado): barra de título própria com os três pontos (fechar, minimizar, maximizar; arrastar move), modos e Fila, abas do herdr desenhadas pelo Maestro (`1 claude ◐`, `+`, indicador PREFIX), dicas e status (`hoje: US$`). IBM Plex Sans embutida. O que ainda não existe aparece apagado no lugar dele
- [x] lateral na ordem do protótipo: repo · Auditoria · FEATURES · cards (com `US$ · ctx %` lidos do JSONL do Claude) · Nova feature · TERMINAIS · Mudanças/Arquivos (diff da branch contra a base)
- [x] lateral: um card por agente (o seu uso real é um workspace com vários Claudes em abas), agrupado por repo, com branch, glifo de status, tempo, linha de status e filete de atenção; o clique faz `agent.focus`. Ctrl+Shift+1…9 e Ctrl+Shift+Espaço (próximo da fila)
- [x] "Terminais" = workspaces do herdr (`$ ~ · N abas · só terminal`), clique faz `workspace.focus`
- [x] herdr sem o chrome dele: o Maestro usa a sessão própria `maestro` com `~/.config/maestro/herdr.toml` (seu `config.toml` + lateral oculta e barra de abas fixa na linha 0, que o Maestro esconde). A sessão `default` do Ghostty não muda
- [x] binds de split do `ghostty/config` viram as teclas do herdr (prefixo do seu `config.toml`) escritas no PTY, para valer no cliente do Maestro
- [x] reconexão quando o server do herdr reinicia: a lateral retoma sozinha ("herdr: fora do ar" no status) e o cliente é reaberto enquanto sair com erro (servidor caiu); detach (Ctrl+A d) fecha o Maestro como antes
- [x] paleta de comandos (`Ctrl+Shift+P`, ou clique em "comandos"): o `CMDS` do protótipo na mesma ordem, com o atalho ao lado, o mesmo fuzzy e "Feature: <branch>" na busca; Esc ou clique fora fecham. Os comandos de etapas futuras aparecem apagados. Ctrl+Shift+B mostra/oculta a lateral
- [x] tutorial da área (primeira vez): os 4 passos do `TOURS.feat` do protótipo, com destaque e balão; Esc fecha; estado em `~/.config/maestro/state.toml`. O passo 4 termina em "Concluir" até a Nova feature existir
- [x] testes sempre na sessão `MAESTRO_HERDR_SESSION=maestro-test`, nunca na `default`

**Pronto quando:** um `claude` pedindo permissão muda o status na lateral em menos de 1 s, e o clique leva ao pane certo.

## Etapa 3: Features em worktrees, multi-repo e limpeza

- [x] Nova feature (Alt+N, botão da lateral ou paleta), com o diálogo do protótipo: branch, base e prompt → `git worktree add` em `<repo>.wt/<slug>` → copiar os `.env*` ignorados → `workspace.create` no herdr (abas `claude` e `zsh`) → setup do repo (`.maestro.toml` ou lockfile) → `claude` com o prompt. O `worktree.create` do herdr não aceita env, por isso a worktree é do Maestro
- [x] **inventário da feature** em `~/.local/share/maestro/features/<id>.json`: worktrees, branches (e se foram criadas pela feature), porta, projeto do compose e workspace. Gravado antes do herdr, para a Limpeza achar a worktree mesmo se algo falhar. MRs entram na Etapa 5
- [x] `PORT` (3001…) e `COMPOSE_PROJECT_NAME=<repo>-<slug>` únicos por worktree, no env do workspace do herdr; a porta aparece no card
- [x] issue do Jira opcional preenche a branch e o prompt, lida pelo MCP do Atlassian num `claude -p --model haiku` (~US$ 0,01, ~7 s)
- [x] melhor de N: 2 ou 3 tentativas `-a/-b/-c` (modelos opus/sonnet/opus[1m] se pedido), o `compareDlg` do protótipo (modelo, estado, arquivos, custo) e "Ficar com esta", que remove as outras pelo inventário, branch inclusive. A coluna "testes" e o "Ver diff" chegam na Etapa 4
- [x] **multi-repo sob demanda:** "Feature: Adicionar repo…" lista os repos irmãos, cria a worktree na mesma branch só quando pedido, manda `/add-dir` ao Claude da feature e registra no inventário. MRs ligados entre si na Etapa 5
- [x] **concluir feature** ("Feature: Remover a atual…", com o `confirmDlg` do protótipo): fecha o workspace, para o compose (sem `-v`: volume fica), remove a worktree, apaga a branch se mergeada na base, apaga `refs/maestro/<id>/*` e o inventário (libera a porta). Avisa antes do que não está commitado. Conferido num repo descartável: `worktree list` e `branch` voltaram ao estado de antes
- [x] worktree extra que terminou sem nenhuma mudança é removida na conclusão sem perguntar (o aviso só conta o que não está commitado). Teste: feature com repo extra concluída deixa os dois repos como estavam
- [x] comando **Limpeza** ("Feature: Limpeza…" na paleta): lista órfãos (worktree em `<repo>.wt/` sem inventário, inventário sem worktree, `refs/maestro` de feature removida, compose parado cujo arquivo está numa worktree do Maestro, branch local já mergeada) e apaga o que estiver marcado. Branch local e worktree com mudanças vêm desmarcadas. Conferido num repo descartável
- [x] tutorial da área: o passo 4 do tutorial de Features espera o clique em "Nova feature", como no protótipo (que não tem tutorial próprio desta área)

**Pronto quando:** você cria uma feature em `bots-api` + `k8s-configmap`, conclui, e `git worktree list`, `git for-each-ref refs/maestro`, `git branch` e `docker compose ls` voltam exatamente ao estado de antes, nos dois repos.

## Etapa 4: Revisão local (multibuffer)

- [x] diff da worktree contra a base, incluindo não commitados e arquivos novos (painel "Revisar" do protótipo, `Ctrl+Shift+E` ou clique em Mudanças)
- [x] multibuffer: os blocos de todos os arquivos num painel só; dois cliques numa linha editam e Enter salva no arquivo da worktree (`ponytail:` campo de linha única, cursor no fim)
- [x] aceitar e desfazer por bloco (`git apply -R` do bloco), com "Desfazer" no toast (reaplica o patch)
- [x] comentários por linha (`C` ou o `+`) + suas edições → "Enviar ao Claude" (`agent.prompt`) num prompt só, no formato do protótipo. Conferido com `pane.read` no pane do agente
- [x] checkpoints por turno: hook `UserPromptSubmit` só na worktree (`.claude/settings.local.json`, fora do git pelo `info/exclude`; o settings global fica intacto) chama `maestro checkpoint <id>`, que grava uma foto de todos os arquivos em `refs/maestro/<id>/t<n>` sem tocar no índice; seletor de turno no painel; "⏪ Voltar para antes do turno N" com Desfazer. Conferido byte a byte (arquivo novo, apagado e sem newline final)
- [x] testes afetados num split do herdr embaixo do Claude: `jest --findRelatedTests`, `vitest related` ou `cargo test` com os arquivos mudados (`ponytail:` sem o ripwire por enquanto; entra quando a lista de arquivos ficar grande)
- [x] editor de arquivo (o `EditorState` do gpui-component com o realce `hl()` do protótipo): "Abrir no editor" no diff, aba "Arquivos" da lateral e `Ctrl+Shift+O` abrem o arquivo numa aba ao lado de Revisar (● quando sujo), `Ctrl+S` salva, Ln/Col na barra de status. `Ctrl+Shift+L` digita `@arquivo:a-b` (seleção do editor) ou `@arquivo:n` (linha marcada no diff) no Claude da feature, sem Enter. Desvio: o número da linha do cursor fica claro (o gpui-component pinta assim)
- [x] arquivo alterado por fora recarrega sozinho sem edição sua; com edição, banner "Claude alterou este arquivo enquanto você editava" com Ver diff · Manter meu · Aceitar dele (o protótipo não tem; segue o desenho §2.1). Conferido num repo descartável
- [x] tutorial da área: os 3 passos do `TOURS.review` do protótipo na primeira vez que o painel abre

**Pronto quando:** você comenta 2 linhas e edita 1 e o prompt chega no pane certo (conferir com `herdr agent read`), e "voltar turno" restaura os arquivos byte a byte.

## Etapa 5: Pré-voo e ciclo de vida do MR

- [x] **pré-voo** (card "Pré-voo" na aba MR, antes do `/mr`; o protótipo não tem esta tela, então segue os `.card2` dele), com resultado por item. ✓ passou, ✗ barra, – não deu para checar (não barra e nunca aparece como ✓):
  - branch atualizada com a base (`git fetch` + commits atrás de `origin/<base>`)
  - testes afetados verdes (o mesmo comando do botão da Etapa 4)
  - lint e typecheck (`npm run lint/typecheck/tsc` do `package.json`; `cargo clippy` em repo Rust). `ponytail:` sem `.maestro.toml` até algum repo precisar de outro comando
  - Sonar local: `claude -p --model haiku` com o MCP `analyze_code_snippet` em até 10 arquivos mudados (~US$ 0,02), na worktree (o container do MCP monta o diretório atual) mas com `--setting-sources user --no-session-persistence`, para não virar turno nem custo da feature. Barra só bug, vulnerabilidade, bloqueante ou crítico; o resto aparece como aviso. Com vitest e `ctbz-check` no PATH, também `ctbz-check sonar-vitest`. Todo item tem teto de tempo (testes e lint 10 min, fetch 30 s, Sonar 3 min): estourou, vira "não deu para checar"
  - nenhum segredo nas linhas novas (tokens do GitLab/GitHub/Slack/AWS/Google, chave privada, `password = "…"` literal), olhando o que o `/mr` commitaria (worktree inteira, não commitados e arquivos novos). `ponytail:` gitleaks quando estiver instalado
  - nenhum arquivo proibido (`.env*` menos `.env.example`, `node_modules`, `dist`, `build`, `target`, `coverage`, `.next`)
- [x] o `/mr` só roda com o pré-voo verde: o botão manda o `/mr` sozinho quando tudo passa; com item vermelho há "Abrir mesmo assim", que manda `/mr Registre na descrição do MR que o pré-voo passou com override em: …`. O `/mr` digitado no terminal também passa pelo pré-voo: o hook `UserPromptSubmit` da worktree bloqueia (`decision: block`) se os arquivos atuais não são os que passaram (árvore da foto gravada em `<id>.preflight`); qualquer erro do hook deixa passar
- [x] aba MR da feature (o `renderMRTab` do protótipo): cabeçalho com "Abrir no GitLab", pipeline (job que falhou), quality gate do Sonar (o job `sonar`/`sonar-merge-request`, que roda com `sonar.qualitygate.wait=true`; sem ele, "sem Sonar neste pipeline") e comentários de revisores (discussões abertas de outras pessoas; os resumos do GitLab Duo e dos bots `group_…_bot_…` ficam de fora, os comentários de linha deles entram). Tudo pelo `glab api` com o login do glab, só leitura, a cada 20 s com a aba aberta. Remoto que não é GitLab e glab sem login têm estado vazio próprio. O card da lateral ganhou o `mrSum` (`· !42 CI✗ Sonar✓ 1 coment.`)
- [x] botões que chamam a skill certa: "Diagnosticar com /pipe" (`/pipe <link do job>`), "Mandar N pro Claude (/comentario)" (`/comentario !iid`), "Pedir ao Claude para cobrir" e "Abrir MR draft com /mr"
- [x] merge detectado (a lateral relê os MRs a cada 60 s) → "!N foi mergeado. Remover a worktree X?" com o aviso da conclusão da Etapa 3, para a feature do MR mesmo em segundo plano, uma vez por MR (gravado em `<id>.merged`, sobrevive ao reinício; os comentários já enviados ficam em `<id>.sent` e saem da fila e do card). O "Histórico do vault atualizado" do protótipo fica para a Etapa 7
- [x] tutorial da área: o passo do `TOURS.mrtab` no primeiro card da aba

Conferido com um `glab` falso no PATH do app de teste (CI vermelho, sem MR, mergeado), sem tocar no GitLab: o token `glpat-…` montado num arquivo novo e um `.env.local` barram o pré-voo, e o `/mr` só sai pelo override; o hook bloqueia `/mr` sem pré-voo e volta a bloquear depois de qualquer edição.

**Pronto quando:** um MR de teste com um token falso no diff é barrado no pré-voo, e um CI vermelho aparece na aba MR com "Diagnosticar com /pipe".

## Etapa 6: Atenção

- [x] fila de atenção (o `attention()` do protótipo): permissão > CI e Sonar vermelhos > comentários novos, prontas pra revisar e tentativas prontas para comparar > contexto ≥ 80%. "Fila N" no título abre a paleta da fila com o `.pr` colorido por prioridade; `Ctrl+Shift+Espaço` vai ao primeiro item e o toast diz quantos faltam. Cada item leva ao lugar certo (o pane, a aba MR, o painel Revisar, a comparação). `ponytail:` MR alheio, auditoria e daily entram com as Etapas 8, 9 e 7
- [x] mascote no app (o `notify`/`nextMascot` do protótipo): as poses `front`/`blink`/`happy` com as cores do `poses.py`, ? roxo ou ✓ verde, card com a feature, "clique para ir · Esc dispensa · +N na fila", pulo, piscar a cada 450 ms, entrada e saída em passos. Aparece quando uma feature que você não está olhando pede permissão ou termina (com a janela fora de foco também). O clique foca o pane, ir para a feature por qualquer caminho dispensa, o Esc dispensa antes de chegar ao terminal. O som segue o `~/.config/claude-mascot/config.json` (`pw-play`), mas fica mudo com o plugin claude-mascot ligado, porque ele já toca o som do mesmo pedido; fora do foco o plugin continua cuidando do desktop
- [x] permissões que aprendem: hook `PermissionRequest` na worktree (`maestro perm <id>`, só anota) conta pedidos por repo e regra (`Bash(<2 palavras>:*)`, como o `learnPerm`); com 5 na semana, o `choiceDlg` do protótipo: "Agora não", "Só pra mim (.claude/settings.local.json)" (vale para as worktrees abertas do repo e entra nas próximas pelo `install_hook`) e "Para o time (.claude/settings.json, vai no git)" (na worktree da feature que pediu). A pergunta não volta. `ponytail:` conta pedidos, não aprovações (o hook não sabe a resposta); 5 é o `LEARN_AFTER`
- [x] custo e contexto por feature lidos do JSONL da sessão (no card desde a Etapa 2), com `ctx N% ⚠` laranja a partir de 80% e o item "contexto" na fila

Nota: os hooks rodam o binário do próprio Maestro; o build de debug (1 GB) leva ~1,5 s só no `execve` nesta máquina, o de release não.

**Pronto quando:** uma permissão numa feature de fundo mostra o mascote, e o clique foca o pane certo.

## Etapa 7: Vault

- [x] parser do `~/vault` (ou `MAESTRO_VAULT`): notas por pasta menos `templates/`, frontmatter, `[[links]]` (com `|apelido` e `#seção`) e backlinks. `ponytail:` relido quando muda alguma data de arquivo (a cada 2 s), sem o crate `notify`
- [x] modo Vault (o `renderVaultTree`/`renderVaultMain` do protótipo, no título Features · MRs · Vault e na paleta): busca, Grafo, Hoje, Atividade, notas por pasta com `←N` menções; a nota renderizada (frontmatter numa linha, títulos, listas, `código` em mono, **negrito**, `[[links]]` clicáveis, quebrados em laranja), "Editar" → "Salvar no vault", "No contexto das features" (nota de repo), "Links desta nota" e "Mencionado em". Hoje marca `[x]` com um clique e o "Gerar resumo do dia" só grava com "Salvar na daily"; sem daily do dia, "Criar daily de hoje" a partir do template
- [x] toda feature nova recebe `repos/<repo>.md` e as notas que ela linka: o Maestro grava `<id>.context.md` e o `claude` da feature abre com `--append-system-prompt "$(cat …)"`
- [x] escritas permitidas pelo `vault/CLAUDE.md`: MR aberto → linha no Histórico de `repos/<repo>.md` (criada do template se faltar) no formato que já está lá (`- data: MR !N \`branch\` — título (aberto)`); mergeado → tira o "(aberto)" e marca `[x]` na tarefa da daily que cita a branch, o `!N` ou a chave do Jira. Uma vez cada (`<id>.hist`). Toda escrita vai para a Atividade (`vault-activity.log`) e para o toast com Desfazer. `decisions/` de gotcha é o Claude quem escreve (ele segue o contrato); o Maestro observa o vault o tempo todo, mesmo fora do modo Vault, e toda nota criada, mudada ou apagada fora dele entra na Atividade como "(fora do Maestro)", com Desfazer. O log de um `MAESTRO_VAULT` de teste é outro arquivo
- [x] grafo (o `mountGraph` do protótipo): notas, links, quebrados tracejados, features `⎇` e MRs `!N` ligados ao repo; cor por pasta, tamanho pelo número de links, anel na nota atual; hover destaca vizinhos, arrastar nó e fundo, roda = zoom, clique abre a nota ou foca a feature; busca, "só <repo> (2 saltos)", links quebrados, features e MRs; a primeira abertura já vem calculada e enquadrada. `ponytail:` O(n²) por passo, Barnes-Hut a partir de ~500 notas
- [x] tutorial da área: os 3 passos do `TOURS.vault` na primeira vez no modo Vault

Conferido numa cópia do vault (`MAESTRO_VAULT`): a feature nova de um repo `bots-api` abriu o Claude com `--append-system-prompt` e a nota dele; o MR aberto escreveu a linha com "(aberto)", o merge tirou o "(aberto)" e marcou a tarefa da daily que citava o `!77`.

**Pronto quando:** uma feature nova traz a nota do repo no contexto, e um merge escreve a linha certa no Histórico.

## Etapa 8: MRs de outras pessoas + revisor

- [x] inbox (o `renderInbox` do protótipo, modo MRs no título e "Ir para: MRs"): Para revisar (`reviewer_username`), Meus e o grupo (`MAESTRO_MR_GROUP`, padrão `ctbz/experts`, paginado); selo de CI lido do detalhe de cada MR; "novo", "revisado" e "commits novos" pelo `~/.mr-review-seen` (o mesmo do script, então as revisões do cron contam); `j`/`k`, Enter, `/`, `R`; relê a cada 60 s. Conferido lendo o GitLab de verdade. `ponytail:` "+N commits" sem o N (precisaria de uma chamada de compare por MR)
- [x] diff do MR com as threads (resolver/reabrir) e rascunho manual (`C` ou `+` na linha, Editar/Descartar), "Arquivos do MR" na lateral, "Descartar todos" e "Publicar review" com a confirmação do protótipo
- [x] revisor (port do `~/mr-review.sh`), com as 11 armadilhas cobertas por teste (`mrs::tests::armadilhas_do_script`):
  - [x] `old_line` em linha de contexto (o awk do script, com `\ No newline`)
  - [x] header `Content-Type` (toda escrita passa por `mrs::write`)
  - [x] sucesso = status ok **e** `.id`
  - [x] fetch de `refs/merge-requests/N/head`
  - [x] colisão de basename
  - [x] gate de pipeline (aviso na gaveta)
  - [x] dedup por todas as notas existentes, GitLab Duo e bots inclusive
  - [x] fallback para nota geral quando a linha está fora do diff (achado desmarcado, `arquivo:linha` no corpo)
  - [x] MR fechado ou merged não publica
  - [x] pular sha já revisado (e "Só commits novos desde a última revisão" quando o sha mudou)
  - [x] rubrica + ferramentas restritas
- [x] saída via `--json-schema` (`result.structured_output`), progresso por `stream-json --verbose` na gaveta "Revisão Claude", achados marcáveis viram rascunhos. Worktree do MR em `~/.cache/maestro/wt/<path>/<iid>`, clone base em `MAESTRO_REPOS` (padrão `~/repos-ctbz`). O parser foi conferido com uma saída real gravada em `src/fixtures/`; a gaveta não rodou de ponta a ponta num MR (custa uma revisão de verdade)
- [x] publicar como uma review só: cada rascunho vira draft note e um `bulk_publish` solta tudo; se algum falha, nada é publicado e os que entraram não são reenviados na próxima tentativa; commit novo no MR depois do rascunho deixa ele vermelho com "Publicar como nota geral"; avisa se você tem rascunhos seus no GitLab feitos fora do Maestro (o `bulk_publish` leva junto); "Descartar todos" apaga os draft notes já criados. Testado contra um `glab` falso que grava cada escrita (`publica_como_uma_review`)
- [x] tutorial da área: o passo do `TOURS.mrs` na lista

**Falta (precisa de você):** o "Pronto quando" mexe no GitLab de verdade e notifica gente, então não rodei. Para conferir: abrir um MR de teste seu, comentar com `C` numa linha adicionada e numa de contexto, Publicar, e ver no GitLab os dois inline e uma notificação só.

**Pronto quando:** num MR de teste, um comentário em linha adicionada e outro em linha de contexto aparecem inline no GitLab, e a review gera uma notificação só.

## Etapa 9: Auditoria

- [x] relatório salvo por sha (`.maestro/audits/<sha>.json`, fora do `git status` pelo `info/exclude`); abrir o projeto nunca roda auditoria, só lê o relatório e o `git`
- [x] incremental (diff desde a última + vizinhos de 1 salto; sem relatório, pede o sha) e completa, com estimativa e `--max-budget-usd` dividido entre as duas passadas
- [x] verificação adversarial (segunda passada com todos os achados) + dedup contra work items deste repo, dos irmãos (mesmo `squad:` no vault) e as decisões do vault; rede de segurança em código (`audit::dedup`, testada com os títulos reais do email-service)
- [x] achados marcáveis, prévia editável, "abrir N work items" com confirmação (só pelo `mrs::write`), "não é problema" persistido em `.maestro/audit-ignore.json` com desfazer
- [x] "Corrigir em feature" a partir do work item (`fix/<iid>-slug`, sem commitar)
- [x] tutorial da área, card na lateral, fila de atenção, status bar, paleta e resumo do dia
- [x] dados de produção via `gcloud`/`kubectl` só de leitura (`audit::PROD_READ`; escrita negada, conferido); ponytail: sem séries do Cloud Monitoring, o `gcloud` não lê

**Pronto quando:** a incremental no email-service desde um sha antigo não devolve nenhum dos #15 a #50 como "novo".

Verificado em 2026-10-15: incremental num clone do email-service desde `74cf8d2` (49 commits, 79 arquivos), US$ 1,66 de 4. Voltaram 2 achados, nenhum dos #15–#50 (P1 `jobId` fixo do dispatch deixa o reprocess de FAILED sem efeito; P2 cardinalidade da label `application` das métricas). GitLab só lido; escrita bloqueada por um `glab` falso. Essa corrida ainda usava `--setting-sources user`; o `--restricted` (com `--json-schema` e o `npx` negado) foi conferido depois com chamadas de centavos.

---

## Rodada 3: LSP, revisão de MR completa, foco, colunas e extras

**0. Preparação**
- [x] checklist criado
- [x] instalar `typescript-language-server`/`typescript` (npm -g) e `rust-analyzer` (rustup)

**A. Protótipo (portão: só implemento a UI nova depois da sua aprovação)**
- [x] MR: Aprovar/Remover aprovação, Merge (apagado com motivo) e diálogo de merge
- [x] MR: Lado a lado | Unificado (tecla S; Ctrl+Alt+D é do GNOME), Ocultar gerados (N), Expandir/Colapsar todos
- [x] MR: arquivo colapsável na lista e no cabeçalho, pílula "gerado"
- [x] MR: Responder → rascunho dentro da thread; seção Discussões; threads em linha removida
- [x] inbox com ponto de novo/nova resposta; toast de notificação
- [x] editor: diagnóstico, hover, completar, faixa "sem servidor", problemas na status bar
- [x] nova feature: Outros repos e Em cima de; lateral com grupo multi-repo e pilha
- [x] blame na linha, aba Histórico, paleta de tarefas, painel Problemas
- [x] painel Perguntar ao Claude; Ctrl+Shift+K comando por descrição
- [x] modo News e Resumo do dia
- [x] abas de arquivo do editor com × (● quando há alteração; o segundo × descarta) e divisas arrastáveis entre colunas e na gaveta, que já existiam no app
- [x] screenshots conferidos e artifact republicado
- [x] aprovado por você (2026-10-15)

**B. Foco por setas e colunas**
- [x] Ctrl+Shift+→ no pane mais à direita foca o editor (com vizinho, troca de pane no herdr); Ctrl+Shift+← no editor ou na aba Revisar volta ao terminal
- [x] seleção por palavra no editor em Shift+Alt+←/→ (o GNOME usa Ctrl+Shift+Alt+←/→ para mover a janela de workspace, então esse nunca chegaria ao app)
- [x] lista de arquivos do MR redimensionável (o centro do MR não estoura mais a janela quando fica estreito)
- [x] lateral do Vault redimensionável
- [x] gaveta do MR redimensionável
- [x] conferido no app de teste

**C. Revisão de MR**
- [x] `diff::side_by_side` + teste
- [x] `diff::boilerplate` + teste
- [x] `mrs::approve`/`unapprove`/`merge` + rascunho de resposta (`in_reply_to_discussion_id`), cobertos no teste do glab falso
- [x] lado a lado (padrão, salvo) e unificado
- [x] colapsar/expandir por arquivo e todos
- [x] filtro de boilerplate com seção "Gerados (N)"
- [x] Discussões gerais, threads em linha removida, Responder
- [x] Resolver fora da thread de UI
- [x] Aprovar e Merge no cabeçalho, com confirmação
- [x] conferido no app de teste (MR real só leitura; escrita no glab falso)

**D. Notificações de MR**
- [x] polling desde a abertura do app
- [x] `mr_notify` (MR novo, resposta a nota minha, sem renotificar) + teste
- [x] toast com Abrir + ponto na inbox
- [x] `notify-send` com ação Abrir quando a janela está fora de foco (disparo conferido; o clique em "Abrir" na notificação do GNOME não deu para automatizar)
- [x] conferido no app de teste

**E. LSP**
- [x] `lsp.rs`: processo, JSON-RPC, initialize, diagnósticos + teste do enquadramento
- [x] didOpen/didChange/didSave/didClose no editor
- [x] completar, hover, ir para definição (F12, inclusive outro arquivo), code actions. O Ctrl+clique não navegou no teste; ficou F12
- [x] diagnósticos no editor e contagem na status bar
- [x] faixa com o comando de instalação quando falta o servidor
- [x] conferido num clone TS (erro proposital, hover, completar, definição) e num `.py`

**F. Extras**
- [x] F1 feature multi-repo (worktrees, lateral, revisão por repo, aviso de env fora do configmap, remoção). Por decisão sua, os outros repos não aparecem mais na criação: entram depois, pela paleta "Feature: Adicionar repo…". "Relacionados" no MR depende do `/mr` seguir o contexto
- [x] F2 blame na linha (com MR de origem) e aba Histórico ("Abrir MR !n" sem teste contra remote real)
- [x] F3 tarefas por repo num split + painel Problemas (matcher com teste)
- [x] F4 Perguntar ao Claude sobre um trecho (só leitura, com continuação)
- [x] F5 MRs empilhados (rebase, Mover para main) + teste da decisão
- [x] F6 Ctrl+Shift+K: comando por descrição, digitado sem executar
- [x] F7 News do daily.dev (perfil local, pontuação com teste, reordenação diária, modo News, Resumo do dia). Bookmark testado só com curl falso

**Pendente com você:** aprovar e dar merge de verdade num MR de teste seu; clicar "Abrir" numa notificação real do GNOME.

---

## README

- [x] ambiente demo isolado (repos `loja-api`/`infra-config`, vault, MR fictício via glab falso, News por fixture)
- [x] print hero
- [x] GIFs das principais: features e terminal; revisão da feature; editor com LSP; MRs
- [x] prints das demais: Perguntar ao Claude e Ctrl+Shift+K; tarefas e Problemas; pilha e repo extra; vault; auditoria; News
- [x] README em PT-BR: o porquê, funcionalidades, instalação, configuração, atalhos, arquitetura, desenvolvimento
- [x] varredura: nenhuma menção à empresa no texto nem nos frames; sem token; mídia ≤ 25 MB
- [x] commit, push, CI verde, ambiente demo apagado (CI conferido após o push)

---

## Issues da auditoria (GitHub #1–#12)

- [x] #1 revisor de MR com `--setting-sources user` e `--strict-mcp-config` (o `.claude/` do MR não carrega)
- [x] #2 testes afetados com `sh_quote`
- [x] #3 `--paginate` lido em stream, sem `][` → `,`
- [x] #4 Ctrl+S relê o disco e abre o aviso de conflito
- [x] #5 auditoria prod sem `kubectl get` (bloqueado em `--disallowedTools`)
- [x] #6 `notify-send -A` numa thread do SO
- [x] #7 PID do revisor zerado no `Done`
- [x] #8 `--max-budget-usd` no revisor (2.00) e no Perguntar (0.50)
- [x] #9 `pw-play`/`xdg-open` colhidos (`spawn_reaped`)
- [x] #10 falha no `install_hook` vira toast
- [x] #11 inventário com escrita atômica e log de JSON inválido
- [x] #12 actions fixadas por SHA + Dependabot

## Bancos (túneis dos aliases do zsh)

- [x] Protótipo: tela global Bancos (Alt+6), filtros, seletor Ctrl+Shift+D, indicador na barra de status
- [x] `tunnels.rs`: lê os aliases do zsh que rodam cloud-sql-proxy/port-forward, porta e alvo, liga (grupo de processos próprio, log em ~/.cache/sql-proxy) e desliga (quem escuta na porta)
- [x] `tunnels_view.rs`: lista com status (de pé, subindo, caiu com a última linha do log), Copiar host, Ver log, Desligar todos
- [x] Shell: modo no título com contagem, Alt+6, Ctrl+Shift+D, comandos na paleta, dicas, "N bancos" na barra de status
- [x] Teste no app com aliases falsos: liga, cai, desliga, sobrevive ao fechar o app e é reconhecido ao reabrir

## Árvore de arquivos editável (extensão: o protótipo não cobre)

- [x] Protótipo: menu do botão direito (Novo arquivo, Nova pasta, Renomear, Excluir) e campo em linha na aba Arquivos
- [x] `changes.rs`: criar, renomear e excluir com o caminho preso à worktree (recusa vazio, absoluto, `..`, existente) + teste
- [x] Lateral: menu no botão direito, campo em linha, pasta vazia recém-criada aparece na árvore
- [x] Shell: arquivo novo abre no editor; Excluir pede Confirm vermelho
- [x] Editor: abas seguem o renomear e fecham no excluir; aba suja bloqueia com toast
- [x] Teste no app num repo descartável
- Desvio: no protótipo o Renomear já seleciona só o nome; no app o campo (`TextField`) não tem seleção, o cursor fica no fim e apaga com Backspace

## Editor completo e simples (extensão: o protótipo não cobre)

- [x] Protótipo: itens novos no menu da árvore, destaque ao arrastar, campo "Ir para linha", busca no projeto na aba Arquivos, atalhos na ajuda
- [x] Busca no arquivo (Ctrl+F/Ctrl+H, já vem do componente) com o tema escuro; "Replace"/"Replace All" ficam em inglês (o componente não tem tradução pt)
- [x] Árvore: Copiar, Recortar, Colar, Duplicar e Copiar caminho (menu e Ctrl+C/X/V com a árvore em foco); colar com nome livre (`x cópia.ts`); pasta não entra nela mesma + teste
- [x] Árvore: arrastar e soltar move para a pasta (no vazio vai para a raiz); abas seguem
- [x] Árvore: teclado (↑/↓, →/←, Enter, F2, Delete)
- [x] Editor: Ctrl+/ comenta, Alt+↑/↓ move linha, Ctrl+G vai para a linha, Ctrl+W fecha a aba, Ctrl+Tab troca de aba (Ctrl+Z desfaz cada um)
- [x] Busca no projeto: campo no topo da aba Arquivos (`git grep`), clique abre na linha
- [x] Ajuda (?) e paleta com os atalhos novos
- [x] Teste no app num repo descartável
- Desvios do protótipo: o "Ir para a linha" não mostra o intervalo (1–N); ao arrastar acende só a pasta sob o mouse (no vazio, a lista inteira, mais fraca); o termo achado só é marcado quando o minúsculo da linha tem o mesmo tamanho
- Fora desta rodada: formatar, renomear símbolo (LSP), dobrar código, dividir o editor, Ctrl+D (o gpui-base não expõe adicionar seleção), multi-cursor por Ctrl+Alt+↑/↓ (o sistema pega o atalho)

## Backlog (não entra sem decisão)

- Conflitos entre features (ripwire branch conflicts)
- Do merge à produção (rollout + Cloud Logging)
- Resumo de ausência
- Notificação no celular quando você está fora
