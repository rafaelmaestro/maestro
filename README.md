<p align="center">
  <img src="brand/logo/final/maestro-horizontal.svg" alt="Maestro" width="340">
</p>

<p align="center">
  <b>Uma IDE pessoal em Rust onde o Claude Code trabalha e você rege.</b><br>
  Terminal no centro, várias features em paralelo, revisão do que o Claude fez e MRs do GitLab sem sair da janela.
</p>

<p align="center">
  <a href="https://github.com/rafaelmaestro/maestro/actions/workflows/ci.yml"><img src="https://github.com/rafaelmaestro/maestro/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <img src="https://img.shields.io/badge/Rust-GPUI-orange" alt="Rust + GPUI">
  <img src="https://img.shields.io/badge/Linux-x86__64-blue" alt="Linux">
</p>

<p align="center">
  <img src="docs/media/hero.png" alt="Tela principal do Maestro" width="100%">
</p>

## Por que

O dia a dia é o Claude Code no terminal. O editor serve para **revisar depois** o que ele fez. O Maestro foi desenhado a partir disso:

- **O terminal é o centro.** É um terminal de verdade, rodando o [herdr](https://github.com/herdrdev/herdr) com o visual do seu Ghostty: zsh, vim, lazygit e qualquer TUI.
- **Várias features ao mesmo tempo, sem gerenciar worktree na mão.** Cada feature vira uma worktree com o próprio Claude, a própria porta e o próprio `docker compose`.
- **Você revisa antes de aceitar.** O diff da feature num buffer só, editável, com aceitar ou desfazer por bloco e checkpoint por turno do Claude.
- **Nada sai da máquina sem você ver.** Comentários, aprovações e merges passam por rascunho e confirmação.

## Funcionalidades

### Features em paralelo, terminal primeiro

<img src="docs/media/features.gif" alt="Criando uma feature e acompanhando o Claude" width="100%">

- **Nova feature (`Alt+N`):** branch, base e prompt. O Maestro cria a worktree, copia os `.env*`, roda o setup do repo e abre o `claude` já com a tarefa.
- **Lateral:** mostra o estado de cada agente (*precisa de você*, *trabalhando*, *pronto pra revisar*, *parado*), com custo e contexto usado.
- **Mascote e fila de atenção (`Ctrl+Shift+Espaço`):** levam você direto ao pane que pediu permissão.
- **`Ctrl+Shift+1…9`** pula de feature. **`Ctrl+Shift+←/→`** troca de pane e, na borda direita, entra no editor.
- **Melhor de N:** 2 ou 3 tentativas do mesmo prompt, com modelos diferentes, e uma tela para comparar.

### Revisão do que o Claude fez

<img src="docs/media/review.gif" alt="Revisão em multibuffer" width="100%">

- Todas as mudanças da feature num buffer só. Dá para editar ali mesmo, e a edição vai para o arquivo.
- **Aceitar** ou **Desfazer** por bloco, com `Ctrl+Z`.
- **Turnos:** cada mensagem enviada ao Claude grava um checkpoint. Você filtra a revisão por turno e pode voltar para antes de qualquer um.
- **Comentar e enviar:** comentários viram um prompt único com **Enviar ao Claude**, e as suas edições vão junto.
- **Testes afetados:** rodam num split do terminal.

### Editor com LSP

<img src="docs/media/editor.gif" alt="Editor com LSP, blame e histórico" width="100%">

- **Diagnóstico** sublinhado e "N erros · M avisos" na barra.
- **Hover**, **completar** e **ir para a definição** (`F12`), que abre outro arquivo se for preciso.
- **Servidores de linguagem:**
  - TypeScript/JavaScript: segue a versão do projeto, ou usa o TS 7 nativo;
  - Rust, Go, Python e JSON;
  - quando falta um servidor, aparece uma faixa com o comando de instalação.
- **Blame e Histórico:** blame no fim da linha, com o MR que trouxe a mudança, e aba **Histórico** do arquivo.
- **Abas e colunas:** abas com ×, com aviso se houver alteração não salva, e colunas redimensionáveis com o mouse.

### MRs do GitLab

<img src="docs/media/mrs.gif" alt="Revisão de MR lado a lado" width="100%">

- **Inbox:** *Para revisar*, *Revisados*, *Meus* e o seu grupo, com o estado do CI e um aviso de commits novos desde a sua última revisão.
- **Diff:** lado a lado por padrão (`S` alterna para unificado), arquivos colapsáveis e **Ocultar gerados** (lockfiles, snapshots, minificados).
- **Comentários:** rascunho numa linha (`C`), **Responder** em threads e **Resolver**. Tudo sai como **uma review só** no **Publicar** (`Ctrl+⏎`).
- **Aprovar** e **Merge** com squash e opção de apagar a branch. O botão fica apagado, com o motivo, quando o GitLab não permite.
- **Revisor Claude (`⇧R`):** os achados viram rascunhos que você edita ou descarta antes de publicar.
- **Notificações:** MR novo para revisar ou resposta numa thread sua aparecem dentro do app e, com a janela fora de foco, no sistema.
- **Vigia dos seus MRs:** todo MR seu de uma feature do Maestro (aberto pelo botão, pelo `/mr` ou por `glab mr create`, incluindo as fatias `<branch>-n`) é vigiado sozinho. MR fora de feature vai ao Claude solto cuja sessão rodou o `glab mr create` dele (achado no transcript em `~/.claude/projects`). Pipe que falha, thread nova e o revisor Claude, que roda uma vez depois do verde, vão ao Claude da feature, que delega a um agente em background por MR (`mr-<repo>-<iid>`). Esse agente corrige, faz push e responde na thread. Com o pipe verde, o Maestro tira o MR do draft: os bots de IA de revisão só comentam e o time só é avisado com o MR fora do draft. Depois de 3 falhas seguidas do mesmo job, o vigia para e avisa. O Claude não recebe nada enquanto espera você responder uma pergunta ou permissão. Para o agente não parar em pedido de permissão, libere `Bash(git push:*)` e `Bash(glab api:*)` no `~/.claude/settings.json`.

### Perguntar ao Claude e comando por descrição

<img src="docs/media/ask.png" alt="Perguntar ao Claude sobre um trecho e Ctrl+Shift+K" width="100%">

- **Perguntar ao Claude:** `?` num bloco do diff abre uma conversa sobre aquele trecho ("Explica este bloco", "Isso quebra quem chama?"). É só leitura: nada vira comentário sozinho.
- **Comando por descrição (`Ctrl+Shift+K`):** no terminal, você descreve o que quer e o comando é digitado no prompt **sem executar**.

### Tarefas e Problemas

<img src="docs/media/tasks.png" alt="Rodando uma tarefa e indo para o erro" width="100%">

- **▶ Rodar tarefa…:** encontra os scripts do `package.json`, os alvos do `Makefile`, `cargo` e `go`.
- Roda num split, e os erros caem na aba **Problemas** junto com os do LSP. O clique abre o arquivo na linha.

### MRs empilhados e repos extras

<img src="docs/media/stack.png" alt="Feature empilhada e rebase" width="100%">

- **Em cima de:** uma feature parte da branch de outra, e o MR aponta para ela.
- **Rebase e mudança de base:** quando a mãe recebe commits, aparece **base mudou · Rebase**. Quando ela é mergeada, **Mover para main**.
- **Feature: Adicionar repo…** (paleta): quando você descobre que precisa mexer em outro repo, a mesma branch vira uma worktree lá e o Claude passa a enxergar a pasta.

### Vault

<img src="docs/media/vault.png" alt="Vault com notas e grafo" width="100%">

- A sua memória de trabalho em Markdown, com links `[[assim]]`, backlinks e grafo.
- Cada feature nova recebe a nota do repo no contexto do Claude.
- **Escritas automáticas:** só no Histórico de MR e na daily, sempre registradas e com desfazer.

### Auditoria de código

<img src="docs/media/audit.png" alt="Relatório de auditoria" width="100%">

- **Disparo e escopo:** nunca roda sozinha. É incremental desde a última auditoria, ou completa, sempre com estimativa e teto de custo.
- **Achados:** verificação adversarial de cada um e deduplicação contra os work items abertos e fechados.
- **Depois do relatório:** o achado vira work item com confirmação, ou é marcado como "não é problema" para não voltar. "Corrigir em feature" cria a branch com o Claude.

### News

<img src="docs/media/news.png" alt="News filtrado pelo seu stack" width="100%">

- Notícias do [daily.dev](https://daily.dev), filtradas pelo que os seus repos usam (dependências, versão, imagem base) e pelo seu vault.
- Cada item diz **por que** apareceu e **o que pode afetar**.
- **Resumo do dia** na primeira abertura de cada dia.

## Instalação

**Pré-requisitos**
- Linux x86_64 com **Vulkan** funcionando (`vulkaninfo --summary`);
- [Ghostty](https://ghostty.org), cuja config o Maestro lê para fonte e cores;
- [herdr](https://github.com/herdrdev/herdr);
- [Claude Code](https://docs.claude.com/claude-code);
- [`glab`](https://gitlab.com/gitlab-org/cli) autenticado (`glab auth login`).

```sh
git clone git@github.com:rafaelmaestro/maestro.git && cd maestro
./scripts/deps.sh          # pacotes do GPUI, rustup e checagem do Vulkan
cargo build --release
./scripts/desktop.sh       # ícone e atalho no menu/dock
```

**Servidores de linguagem (opcionais, um por linguagem):**

```sh
npm i -g typescript                     # TypeScript 7: servidor nativo (tsc --lsp)
npm i -g typescript-language-server     # usa o TypeScript do projeto quando há node_modules
rustup component add rust-analyzer
go install golang.org/x/tools/gopls@latest
npm i -g pyright
```

**News (opcional):** gere um token pessoal em [daily.dev/settings/api](https://daily.dev/settings/api) e salve:

```sh
mkdir -p ~/.config/maestro
install -m 600 /dev/stdin ~/.config/maestro/dailydev.token <<< 'SEU_TOKEN'
```

## Configuração

Por repositório, um `.maestro.toml` opcional define o setup da worktree:

```toml
setup = "npm ci"
```

Variáveis de ambiente:

| Variável | Para que serve |
|---|---|
| `MAESTRO_REPOS` | pasta onde ficam os seus repositórios (inbox de MRs, News, repos extras) |
| `MAESTRO_MR_GROUP` | grupo do GitLab cuja aba aparece na inbox de MRs |
| `MAESTRO_VAULT` | pasta do vault (padrão `~/vault`) |
| `MAESTRO_HERDR_SESSION` | sessão do herdr usada pelo terminal |
| `MAESTRO_ASK_MODEL` | modelo do "Perguntar ao Claude" |
| `MAESTRO_STATE` | arquivo de estado (tutoriais, Resumo do dia) |
| `MAESTRO_FEATURES`, `MAESTRO_SEEN`, `MAESTRO_MR_NOTIFY`, `MAESTRO_MR_WATCH` | inventário das features, shas revisados, estado das notificações e do vigia de MRs |
| `MAESTRO_NEWS_TOKEN`, `MAESTRO_NEWS_CACHE`, `MAESTRO_NEWS_IGNORE`, `MAESTRO_CURL` | token, cache e ocultos do News, e o `curl` usado por ele |

As variáveis de caminho servem principalmente para rodar uma instância de teste isolada da sua.

## Atalhos

| Atalho | Ação |
|---|---|
| `Ctrl+Shift+P` | paleta de comandos |
| `Ctrl+Shift+O` / `Ctrl+Shift+F` | abrir arquivo / árvore do projeto |
| `Alt+1…5` | Features, MRs, Vault, Auditoria, News |
| `Alt+N` | nova feature |
| `Ctrl+Shift+1…9` | ir para a feature |
| `Ctrl+Shift+Espaço` | próximo item da fila de atenção |
| `Ctrl+Shift+E` | revisar mudanças da feature |
| `Ctrl+Shift+←/→` | trocar de pane; na borda, entre terminal e editor |
| `Ctrl+Shift+\` / `Ctrl+Shift+Enter` | dividir o terminal ao lado / embaixo |
| `Ctrl+Shift+L` | mandar a seleção para o Claude |
| `Ctrl+Shift+K` | comando por descrição |
| `F12` | ir para a definição |
| `Shift+Alt+←/→` | selecionar por palavra no editor |
| `?` | ajuda da tela atual |

## Como é feito

- **Interface:** [GPUI](https://www.gpui.rs) (o framework do Zed) via `gpui-kit`, sem webview.
- **Terminal:** `alacritty_terminal` com renderizador próprio, rodando o herdr. Abas, splits e detecção de agente vêm do socket dele.
- **GitLab:** tudo pelo `glab`. Toda escrita passa por um único ponto e confere o `id` da resposta.
- **Claude:** interativo no terminal. O revisor, a auditoria e o "Perguntar" rodam em `claude -p --output-format stream-json`, com ferramentas restritas a leitura.
- **LSP:** um cliente próprio e mínimo (JSON-RPC no stdio) ligado ao editor do `gpui-component`.

O desenho completo e as decisões de UX estão em [docs/DESIGN.md](docs/DESIGN.md); o andamento, em [CHECKLIST.md](CHECKLIST.md).

## Desenvolvimento

```sh
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
cargo test -- --ignored lsp     # testes contra servidores de linguagem reais
```

- O CI roda fmt, clippy e testes a cada push.
- Os testes que tocam o GitLab usam um `glab` falso. Nada de teste escreve no GitLab de verdade.
