# Yazi2D

**English** · [Русский](#yazi2d--русский)

A fast dual-pane terminal file manager. A spiritual fork of
[Yazi](https://github.com/sxyazi/yazi): Yazi's speed and clean TUI, with a
deliberately simplified Total Commander workflow.

> **Status:** working MVP `0.5.2`. Two independent panels, a path bar on each,
> persistent favorites, copy/move between panels, quick preview, thumbnail grid,
> background directory loading, diagnostics log, sorting, per-type colours,
> bulk rename, rebindable keys and trackpad support.

```
┌ Favorites ─────────────┐┌ Left ──────────────────────┐┌ Right · preview ───────────┐
│ ● Home                 ││ /Users/me/Projects         ││ /Users/me/Projects         │
│   Downloads            ││   Name       Ext     Size  ││  README.md · text · 8.1K   │
│   Documents            ││ ▸ src/                     ││  # Yazi2D                  │
│   Desktop              ││ ▸ assets/                  ││                            │
│                        ││   README.md  .md     8.1K  ││  A fast dual-pane terminal │
│                        ││   photo.jpg  .jpg    2.3M  ││  file manager…             │
└ Ctrl+B add ────────────┘└────────────────────────────┘└────────────────────────────┘
 Copied 3 item(s).                                  README.md  file  2026-11-11 09:41
 Yazi2D · Tab switch · § favs · Ctrl+E settings · Ctrl+Q preview · Ctrl+T thumbs · s sort · F5/c copy · ? help
```

## Features

- **Two panels** with independent working directories, cursor and selection.
- **Path bar** at the top of each panel (left-truncated, the tail is always visible).
- **Favorites sidebar** — bookmarks that survive restarts.
- **Copy/move between panels** (`F5`/`F6` or `c`/`m`), recursive, symlink-aware, confirmed before running.
- **Background operations** with progress; the UI stays navigable.
- **Quick preview** (`Ctrl+Q`/`Ctrl+P`): the **opposite** panel shows text, an
  image, or a folder grid and follows the cursor. Read-only.
- **Thumbnail grid** (`Ctrl+T`) with cached image tiles and block-art icons per file type.
- **Favorites, hidden files, mkdir, rename, delete, bulk rename, select-all.**
- **Sorting** by name, size, type or date, with reverse.
- **Per-type colours** with three presets and a preview menu.
- **Rebindable keys** and trackpad inversion in an in-app settings menu.
- **Layout-independent shortcuts**: they work with the Russian keyboard layout.
- **Background directory loading** — no freezes on slow or cloud folders.
- **Diagnostics log** with timings and `WARN` lines for slow paths.
- **macOS app bundle** with a custom icon (`packaging/macos/build-app.sh`).

## Install

```sh
git clone https://github.com/wonderrozz-coder/yazi2d.git
cd yazi2d
cargo build --release
./target/release/yazi2d ~/Projects ~/Downloads
```

Rust stable (1.80+) is required. If Rust is not installed:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
source "$HOME/.cargo/env"
```

The binary lands in `target/release/yazi2d` (≈2 MB) and can be copied to
`~/.local/bin` or `/usr/local/bin`.

### macOS app icon

```sh
./packaging/macos/build-app.sh
```

This builds **Yazi2D.app** with a custom icon and installs it to
`/Applications`, so it appears in Launchpad, Spotlight and the Dock. Because
Yazi2D is a terminal UI, the app opens a terminal window and starts there.

The launcher picks the best terminal automatically, preferring ones with
**truecolor** so image previews and thumbnails render correctly:

```
Ghostty → kitty → WezTerm → iTerm2 → Terminal.app (last resort)
```

Terminal.app has no truecolor support, so previews appear as black blocks
there. Install [Ghostty](https://ghostty.org) to get the full experience.
Pass `--no-copy` to build into `dist/` without installing.

The icon is generated from `packaging/icon-source.png`; regenerate it with
`python3 packaging/render-icon.py` (no dependencies, pure stdlib).

> If the repository lives in a synced folder (Nextcloud/Dropbox), keep build
> artifacts out of it: `export CARGO_TARGET_DIR="$HOME/.cache/yazi2d-target"`.

## Keys

On macOS, function keys may require `Fn`; every essential action has a letter
alternative.

| Keys | Action |
|---|---|
| `Tab` | switch active panel |
| `↑`/`k`, `↓`/`j` | move cursor (scroll in preview) |
| `PgUp`/`PgDn`, `Home`/`g`, `End`/`G` | page, top, bottom |
| `Enter` / `l` / `→` | open folder; for a file, open with its associated app |
| `Backspace` / `h` / `←` | go to parent directory |
| `Space` / `Insert` | select and move down |
| `Esc` | clear selection / close preview / cancel |
| `F5` / `c` | copy to the other panel (confirm with `Enter`/`y`) |
| `F6` / `m` | move to the other panel (confirm with `Enter`/`y`) |
| `F7` / `n` | create folder |
| `F2` / `r` | rename |
| `F8` / `d` / `Delete` | delete (confirm with `Enter`/`y`) |
| `Ctrl+Q` / `Ctrl+P` | quick preview in the **other** panel |
| `Ctrl+T` | thumbnail grid |
| `F9`, then `↑`/`↓` | thumbnails on/off (fallback) |
| `F9`, then `←`/`→` | open cursor folder in that panel (fallback) |
| `Shift+←` / `Shift+→` | open cursor folder in that panel |
| `Alt+←` / `Alt+→` | quick preview in that panel |
| `Ctrl+O` | open cursor folder in the other panel |
| `§` / `Shift+Tab` / `Ctrl+D` | focus favorites / back to panels |
| `Ctrl+B` / `a` | add current folder to favorites |
| `Ctrl+F` | show / hide the favorites sidebar |
| `Ctrl+A` | select everything |
| `Ctrl+G` | bulk rename by pattern |
| `s` / `Ctrl+S` | sort menu (name/size/type/date, `r` reverses) |
| `Ctrl+K` | colour menu for file types |
| `Ctrl+E` | settings: key bindings and trackpad |
| `Ctrl+R` | refresh both panels |
| `Ctrl+H` / `.` | toggle hidden files |
| `?` / `F1` | help |
| `q` / `Ctrl+C` | quit |

Favorites sidebar: `↑`/`↓` select, `Enter` open, `Delete`/`d` remove,
`Esc`/`Shift+Tab` back to panels.

Preview: `↑`/`↓` scroll, `←`/`→` horizontal scroll,
`Ctrl+Q`/`Ctrl+P`/`Esc` close.

Thumbnails: arrows/`hjkl` move, `Enter` opens, `Ctrl+T` back to the list.

**Trackpad:** two-finger scroll moves the list (inverted, as if dragging).
**Double tap with two fingers** opens the entry under the cursor: a folder is
entered, a file is opened with the system default app (`open`, `xdg-open`,
`explorer`). Single taps and horizontal scroll are deliberately inert — macOS
emits horizontal scroll inside vertical gestures and it used to cause random
navigation.

**Settings (`Ctrl+E`):** `i` flips the trackpad direction, arrows + `Enter`
rebind an action, `r` restores its defaults. Stored in
`$YAZI2D_CONFIG_DIR/settings.toml`. `Ctrl+E` and `Esc` cannot be rebound.

**Layout independence:** shortcuts work with the Russian layout — `Ctrl+и` is
`Ctrl+B`, `ы` is `s`, `в` is `d`, matched by physical key. Prompts still type
exactly the characters you press.

**Modifier hints:** hold `Ctrl`, `Option` (`Alt`), `Shift` or `Command` and the
footer shows only the chords for that modifier (requires the kitty keyboard
protocol: Ghostty, kitty, WezTerm, foot, iTerm2).

> **Modifiers are split on purpose.** `Shift+←/→` opens the folder in the other
> panel, `Option+←/→` previews there. `Ctrl+←/→` switches macOS Spaces, and
> Ghostty rewrites `Cmd+←/→` into `Ctrl+A`/`Ctrl+E` before the app sees them, so
> neither is used. Ghostty encodes `Option+←/→` as `Esc b`/`Esc f`; Yazi2D accepts
> `Option+b`/`Option+f` too, so it works without touching the terminal config.

## Configuration

Favorites: `~/.config/yazi2d/favorites.toml`
(macOS: `~/Library/Application Support/yazi2d/favorites.toml`).
Override the directory with `YAZI2D_CONFIG_DIR`.

```toml
favorites = [
  "/Users/me",
  "/Users/me/Projects",
  "/Volumes/Backup",
]
```

Settings: `$YAZI2D_CONFIG_DIR/settings.toml`.

```toml
invert_scroll = true
key.copy = "ctrl+y"
key.delete = "f9"
```

## Logging

```sh
YAZI2D_LOG=off yazi2d                    # disable
YAZI2D_LOG=/tmp/yazi2d.log yazi2d        # custom path
tail -f ~/Library/Application\ Support/yazi2d/yazi2d.log
```

Every directory load is logged with its duration; loads over 250 ms are marked
`WARN slow directory load`. Look for the last `INFO load start` line to find the
directory that stalled.

## Development

```sh
source "$HOME/.cargo/env"

cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo build --release

python3 scripts/e2e_pty.py target/debug/yazi2d   # real-PTY end-to-end test
```

MIT licensed — see [`LICENSE`](LICENSE). Design notes live in
[`docs/DECISIONS.md`](docs/DECISIONS.md), the module map in
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md), and the handoff guide in
[`AGENTS.md`](AGENTS.md).

## Limitations

- No overwrite prompt: conflicting names get a ` (1)` suffix instead.
- No cancelling a running background operation.
- Images are drawn with half blocks in truecolor; no sixel/Kitty/iTerm2 protocol.
- No file editor, search, archives or SSH/SFTP.
- Deletion is permanent (no trash).

## Origin

Yazi2D is an independent Rust + [ratatui](https://ratatui.rs) codebase inspired
by Yazi (architecture ideas, speed, TUI aesthetics) and Total Commander (the
two-panel model). No Yazi source is included.

---

# Yazi2D — Русский

[English](#yazi2d) · **Русский**

Быстрый двухпанельный файловый менеджер для терминала. Идейный форк
[Yazi](https://github.com/sxyazi/yazi): скорость и лаконичный TUI от Yazi,
модель работы — упрощённый Total Commander.

> **Статус:** рабочий MVP `0.5.2`. Две независимые панели, строка текущего пути,
> избранное, копирование и перемещение между панелями, быстрый просмотр,
> режим миниатюр, фоновая загрузка каталогов, файловый лог, сортировка, цвета
> по типам, массовое переименование, переназначаемые клавиши и трекпад.

```
┌ Favorites ─────────────┐┌ Left ──────────────────────┐┌ Right · preview ───────────┐
│ ● Дом                  ││ /Users/me/Projects         ││ /Users/me/Projects         │
│   Загрузки             ││   Name       Ext     Size  ││  README.md · text · 8.1K   │
│   Документы            ││ ▸ src/                     ││  # Yazi2D                  │
│   Рабочий стол         ││ ▸ assets/                  ││                            │
│                        ││   README.md  .md     8.1K  ││  Быстрый двухпанельный     │
│                        ││   photo.jpg  .jpg    2.3M  ││  файловый менеджер…        │
└ Ctrl+B add ────────────┘└────────────────────────────┘└────────────────────────────┘
 Copied 3 item(s).                                  README.md  file  2026-11-11 09:41
 Yazi2D · Tab switch · § favs · Ctrl+E settings · Ctrl+Q preview · Ctrl+T thumbs · s sort · F5/c copy · ? help
```

## Возможности

- **Две панели** с независимыми текущими каталогами, курсором и выделением.
- **Строка пути** в верхней части каждой панели (обрезается слева, хвост всегда виден).
- **Избранное** — боковая панель с закладками; сохраняется между запусками.
- **Копирование и перемещение между панелями** (`F5`/`F6` или `c`/`m`),
  рекурсивно, с поддержкой симлинков и подтверждением перед запуском.
- **Фоновые операции** с прогрессом: интерфейс не блокируется.
- **Быстрый просмотр** (`Ctrl+Q`/`Ctrl+P`): **противоположная** панель показывает
  текст, изображение или сетку каталога и следует за курсором. Только чтение.
- **Сетка миниатюр** (`Ctrl+T`) с кэшем изображений и блочными иконками по типам.
- **Избранное, скрытые файлы, создание, переименование, удаление, массовое переименование, выделить всё.**
- **Сортировка** по имени, размеру, типу или дате, с реверсом.
- **Цвета по типам файлов**: три темы и меню предпросмотра.
- **Переназначаемые клавиши** и инверсия трекпада в меню настроек.
- **Горячие клавиши не зависят от раскладки**: работают и в русской.
- **Фоновая загрузка каталогов** — без зависаний на медленных и облачных папках.
- **Диагностический лог** с таймингами и `WARN` для медленных путей.
- **Приложение для macOS** с иконкой (`packaging/macos/build-app.sh`).

## Установка и запуск

```sh
git clone https://github.com/wonderrozz-coder/yazi2d.git
cd yazi2d
cargo build --release
./target/release/yazi2d ~/Projects ~/Downloads
```

Нужен Rust stable (1.80+). Если Rust ещё не установлен:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
source "$HOME/.cargo/env"
```

Готовый бинарник: `target/release/yazi2d` (≈2 МБ). Можно скопировать его в
`~/.local/bin` или `/usr/local/bin` и запускать как `yazi2d`.

### Приложение с иконкой для macOS

```sh
./packaging/macos/build-app.sh
```

Собирает **Yazi2D.app** с иконкой и устанавливает в `/Applications` — приложение
появится в Launchpad, Spotlight и Dock. Yazi2D — терминальный интерфейс, поэтому
при запуске открывается окно терминала и программа стартует в нём.

Лаунчер сам выбирает лучший терминал, отдавая приоритет тем, что поддерживают
**truecolor** — без этого превью и миниатюры выглядят чёрными квадратами:

```
Ghostty → kitty → WezTerm → iTerm2 → Terminal.app (крайний случай)
```

В Terminal.app нет truecolor, поэтому картинки там будут плоскими. Поставьте
[Ghostty](https://ghostty.org), чтобы получить полную картинку. Флаг
`--no-copy` собирает бандл в `dist/` без установки.

Иконка генерируется из `packaging/icon-source.png`; перерисовать её можно
командой `python3 packaging/render-icon.py` (без зависимостей, только stdlib).

> Проект может лежать в синхронизируемой папке (Nextcloud/Dropbox). Чтобы не
> синхронизировать гигабайты артефактов, вынесите каталог сборки:
> `export CARGO_TARGET_DIR="$HOME/.cache/yazi2d-target"`.

## Управление

`Fn` на macOS: функциональные клавиши могут требовать `Fn`; для всех основных
действий есть буквенные дубликаты.

| Клавиши | Действие |
|---|---|
| `Tab` | переключить активную панель |
| `↑`/`k`, `↓`/`j` | движение курсора (в просмотре — прокрутка) |
| `PgUp`/`PgDn`, `Home`/`g`, `End`/`G` | страница, начало, конец |
| `Enter` / `l` / `→` | войти в каталог; для файла — открыть приложением по умолчанию |
| `Backspace` / `h` / `←` | подняться на уровень выше |
| `Space` / `Insert` | выделить и перейти ниже |
| `Esc` | снять выделение / закрыть просмотр / отменить |
| `F5` / `c` | копировать в другую панель (подтверждение `Enter`/`y`) |
| `F6` / `m` | переместить в другую панель (подтверждение `Enter`/`y`) |
| `F7` / `n` | создать каталог |
| `F2` / `r` | переименовать |
| `F8` / `d` / `Delete` | удалить (`Enter` или `y` подтверждает) |
| `Ctrl+Q` / `Ctrl+P` | быстрый просмотр в **противоположной** панели |
| `Ctrl+T` | режим миниатюр в активной панели |
| `F9`, затем `↑` или `↓` | миниатюры вкл/выкл (резервный путь) |
| `F9`, затем `←` или `→` | открыть папку под курсором в указанной панели |
| `Shift+←` / `Shift+→` | открыть папку под курсором в указанной панели |
| `Alt+←` / `Alt+→` | быстрый просмотр в указанной панели |
| `Ctrl+O` | открыть папку под курсором в соседней панели |
| `§` / `Shift+Tab` / `Ctrl+D` | войти в избранное / вернуться к панелям |
| `Ctrl+B` / `a` | добавить текущий каталог в избранное |
| `Ctrl+F` | показать / скрыть панель избранного |
| `Ctrl+A` | выделить всё |
| `Ctrl+G` | массовое переименование по шаблону |
| `s` / `Ctrl+S` | меню сортировки (имя/размер/тип/дата, `r` — реверс) |
| `Ctrl+K` | меню цветов для типов файлов |
| `Ctrl+E` | настройки: горячие клавиши и трекпад |
| `Ctrl+R` | обновить обе панели |
| `Ctrl+H` / `.` | показать/скрыть скрытые файлы |
| `?` / `F1` | справка |
| `q` / `Ctrl+C` | выход |

В боковой панели избранного: `↑`/`↓` — выбор, `Enter` — перейти,
`Delete`/`d` — удалить закладку, `Esc`/`Shift+Tab` — вернуться к панелям.

В режиме просмотра: `↑`/`↓` — прокрутка, `←`/`→` — горизонтальная прокрутка,
`Ctrl+Q`/`Ctrl+P`/`Esc` — закрыть.

В режиме миниатюр: стрелки/`hjkl` — движение по плиткам, `Enter` — войти,
`Ctrl+T` — вернуться к списку.

**Трекпад:** два пальца вверх/вниз листают список (направление «тянем
содержимое»). **Двойной тап двумя пальцами** открывает элемент под курсором:
папку — входит в неё, файл — открывает приложением по умолчанию (`open`,
`xdg-open`, `explorer`). Одиночный тап и горизонтальные движения намеренно
ничего не делают: macOS присылает горизонтальный скролл и внутри вертикальных
жестов, из-за чего панель непредсказуемо проваливалась в папки.

**Настройки (`Ctrl+E`):** `i` переключает направление трекпада, стрелки +
`Enter` переназначают действие, `r` возвращает стандартные клавиши. Хранится в
`$YAZI2D_CONFIG_DIR/settings.toml`. `Ctrl+E` и `Esc` переназначить нельзя.

**Раскладка:** горячие клавиши работают и в русской раскладке — `Ctrl+и` это
`Ctrl+B`, `ы` это `s`, `в` это `d`, сопоставление по физической клавише. В
полях ввода вводится ровно тот символ, который вы набрали.

**Подсказки в футере:** зажмите `Ctrl`, `Option` (`Alt`), `Shift` или `Command` —
нижняя строка покажет только сочетания для этой клавиши (нужен kitty keyboard
protocol: Ghostty, kitty, WezTerm, foot, iTerm2).

> **Модификаторы разведены намеренно.** `Shift+←/→` открывает папку в соседней
> панели, `Option+←/→` включает там просмотр. `Ctrl+←/→` на macOS переключают
> рабочие пространства, а `Cmd+←/→` Ghostty превращает в `Ctrl+A`/`Ctrl+E` ещё
> до приложения, поэтому они не используются. Ghostty кодирует `Option+←/→` как
> `Esc b`/`Esc f`; Yazi2D принимает и `Option+b`/`Option+f`, так что работает без
> правки конфига терминала.

## Быстрый просмотр и миниатюры

- `Ctrl+Q`/`Ctrl+P` переводит **противоположную** панель в режим просмотра;
  активная панель остаётся списком, и её курсор управляет просмотром.
  Повторное нажатие закрывает просмотр.
- Просмотр папки — это **сетка миниатюр**: изображения видны сразу, папки и
  прочие файлы показывают блочную иконку своего типа.
- Текст читается до 512 КБ; двоичные файлы — hex-дамп.
- Изображения декодируются в фоне, ограничены 1600 px по длинной стороне и
  масштабируются Lanczos3; файлы больше 64 МБ не декодируются.
- `Ctrl+T` — сетка миниатюр: изображения half-блоками, остальные файлы —
  крупной иконкой по типу (папка, картинка, видео, музыка, текст, архив,
  временный, 3D-модель, код, прочее). Под иконкой имя и расширение.
- В списке расширение вынесено в колонку `Ext` перед размером. На узких панелях
  оно возвращается к имени, чтобы тип файла не терялся.
- Пока каталог загружается, панель показывает `Loading <path>…`; `Esc` отменяет.
- **Сортировка** (`s`): имя, размер, тип, дата; `r` меняет направление.
  `..` всегда сверху, папки перед файлами.
- **Цвета** (`Ctrl+K`): папки, изображения, видео, аудио, текст, архивы,
  временные, 3D-модели, код, прочее. Темы `default`, `vivid`, `mono`.
- **Массовое переименование** (`Ctrl+G`): `{name}`, `{ext}`, `{n}` (счётчик с 1),
  `%e=txt` (только расширение). Пример: `photo_{n}` → `photo_1.jpg`.
- **Размеры и даты** догружаются в фоне: список появляется мгновенно, колонки
  заполняются через доли секунды. Это убирает зависания на папках Nextcloud/iCloud.

## Избранное и конфигурация

Файл: `~/.config/yazi2d/favorites.toml`
(macOS: `~/Library/Application Support/yazi2d/favorites.toml`).
Каталог можно переопределить через `YAZI2D_CONFIG_DIR`.

```toml
favorites = [
  "/Users/me",
  "/Users/me/Projects",
  "/Volumes/Backup",
]
```

При первом запуске в избранное попадают `~`, `Desktop`, `Documents`, `Downloads`,
если они существуют.

Настройки: `$YAZI2D_CONFIG_DIR/settings.toml`.

```toml
invert_scroll = true
key.copy = "ctrl+y"
key.delete = "f9"
```

## Логи и зависания

```
~/.config/yazi2d/yazi2d.log                         # Linux
~/Library/Application Support/yazi2d/yazi2d.log     # macOS
$YAZI2D_CONFIG_DIR/yazi2d.log                       # если переменная задана
```

```sh
YAZI2D_LOG=off yazi2d                    # выключить лог
YAZI2D_LOG=/tmp/yazi2d.log yazi2d        # свой путь
tail -f ~/Library/Application\ Support/yazi2d/yazi2d.log
```

Каждая загрузка каталога пишется с временем и числом элементов; загрузки
дольше 250 мс помечаются `WARN slow directory load`. Ищите последнюю строку
`INFO load start` — в ней путь, на котором приложение застряло.

## Разработка

```sh
source "$HOME/.cargo/env"

cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo build --release

python3 scripts/e2e_pty.py target/debug/yazi2d   # сквозной тест в настоящем PTY
```

Лицензия MIT — см. [`LICENSE`](LICENSE). Решения описаны в
[`docs/DECISIONS.md`](docs/DECISIONS.md), карта модулей — в
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md), руководство для продолжения — в
[`AGENTS.md`](AGENTS.md).

## Ограничения

- Нет диалога «перезаписать/пропустить»: конфликты получают суффикс ` (1)`.
- Нельзя отменить уже запущенную фоновую операцию.
- Изображения рисуются half-блоками в truecolor; sixel/Kitty/iTerm2 не используются.
- Нет редактора файлов, поиска, архивов, SSH/SFTP.
- Удаление безвозвратное (корзина в планах).

## Происхождение

Yazi2D — самостоятельная кодовая база на Rust + [ratatui](https://ratatui.rs),
вдохновлённая Yazi (архитектурные идеи, скорость, TUI-эстетика) и Total Commander
(двухпанельная модель). Исходники Yazi в проект не входят.
