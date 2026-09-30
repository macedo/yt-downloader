# YT Downloader

App para Windows que baixa vídeos e áudio do YouTube (e de outros sites suportados pelo [yt-dlp](https://github.com/yt-dlp/yt-dlp)), com interface gráfica nativa escrita em Rust ([egui](https://github.com/emilk/egui)).

O app é uma interface para o `yt-dlp`: ele monta os argumentos, acompanha o progresso e mostra o resultado. Os downloads e conversões são feitos pelo yt-dlp e pelo FFmpeg.

## Funcionalidades

- **Vídeo (MP4)** em qualidade de "Melhor disponível" até 360p
- **Somente áudio** em MP3, M4A, Opus, FLAC, WAV ou no formato original, com escolha de bitrate
- **Prévia do link**: ao colar um link, mostra miniatura, título, canal, duração, capítulos e o tamanho aproximado do arquivo
- **Vários links** de uma vez (um por linha) e **playlists inteiras**
- **Separar por capítulos**: um arquivo por capítulo, útil para álbuns completos
- **Baixar só um trecho**, por exemplo de `1:30` a `2:45`
- **Capa e metadados** embutidos no arquivo
- **Aviso de versão nova do yt-dlp** ao abrir o app, com atualização em um clique
- Tema claro, escuro ou do sistema; configurações salvas entre usos
- Atalhos: **Ctrl+Enter** baixa, **Esc** cancela

## Instalação

Baixe e execute o instalador `YT-Downloader-Setup-<versão>.exe`:

- Não precisa de administrador; instala para o usuário atual em `%LOCALAPPDATA%\Programs\YT Downloader`.
- Por padrão, instala ou atualiza as dependências pelo **winget**: yt-dlp, FFmpeg e Deno. O Deno é exigido pelo YouTube atualmente.
- Para atualizar o app, execute o instalador da versão nova por cima da atual.

O instalador e o app **não têm assinatura digital**. Por isso:

- O Windows SmartScreen pode mostrar um alerta de "editor desconhecido". Clique em **Mais informações → Executar assim mesmo**.
- Em PCs com o **Smart App Control** ligado, o app é bloqueado.

### Requisitos

- Windows 10 ou 11 (64 bits)
- [winget](https://learn.microsoft.com/windows/package-manager/winget/), já incluído no Windows 11, para instalar as dependências
- Conexão com a internet

Se o yt-dlp não estiver instalado, o app mostra o botão **Instalar yt-dlp** na barra de ferramentas.

## Uso

1. Cole um ou mais links no campo **Links**.
2. Escolha **Vídeo** ou **Áudio** na barra de ferramentas.
3. Ajuste o formato, a qualidade e as opções no painel à esquerda.
4. Clique em **Baixar** ou pressione Ctrl+Enter.

Os arquivos vão para a pasta de **Destino**, que por padrão é a pasta Downloads. As configurações ficam em `%APPDATA%\yt-downloader\config.json`.

### Observações

- **Tamanho estimado**: segue a mesma escolha de formato do yt-dlp. Em alguns vídeos o YouTube não informa o tamanho da melhor qualidade (formatos "Premium"); nesse caso o app mostra "Tamanho não informado". Para MP3 VBR e FLAC o valor é aproximado.
- **Separar por capítulos**: o yt-dlp mantém também o arquivo completo. Os arquivos de cada capítulo recebem o nome certo, mas o título gravado nos metadados é o do vídeo.
- **Corte de trecho**: o trecho é recodificado para o corte sair exato. Em vídeos 4K isso pode demorar.

## Desenvolvimento

### Ambiente

O projeto usa o toolchain **GNU** do Rust, que dispensa o Visual Studio:

```bash
winget install -e --id Rustlang.Rustup
winget install -e --id BrechtSanders.WinLibs.POSIX.UCRT
winget install -e --id yt-dlp.yt-dlp
```

```bash
rustup default stable-x86_64-pc-windows-gnu
```

O MinGW (WinLibs) precisa estar no `PATH` durante a compilação, porque o `dlltool` que vem com o rustup não funciona sozinho. O winget adiciona o MinGW ao PATH; abra um terminal novo depois de instalá-lo.

> Com o **Smart App Control** ligado, o Windows bloqueia os executáveis gerados pelo `cargo`, inclusive os scripts de build, e a compilação falha.

### Compilar e testar

```bash
cargo build --release
```

```bash
cargo test --release
```

O executável fica em `target\release\yt-downloader.exe`.

### Gerar o instalador

O instalador é feito com o [Inno Setup](https://jrsoftware.org/isinfo.php):

```bash
winget install -e --id JRSoftware.InnoSetup
```

```bash
powershell -ExecutionPolicy Bypass -File installer\build.ps1
```

O script compila o app em release e gera `dist\YT-Downloader-Setup-<versão>.exe`, com a versão lida do `Cargo.toml`.

### Estrutura

| Caminho | Conteúdo |
|---|---|
| `src/main.rs` | Todo o app: interface, montagem dos argumentos do yt-dlp, prévia do link, checagem de versão e testes |
| `installer/yt-downloader.iss` | Script do Inno Setup (instalação, atalhos, dependências via winget, desinstalação) |
| `installer/build.ps1` | Compila o app e gera o instalador |

### Versões

Cada versão tem uma tag no Git (`v0.4.0`, …). Para lançar uma versão nova:

1. Atualize `version` no `Cargo.toml`.
2. Gere o instalador com o `build.ps1`.
3. Faça o commit e crie a tag.
