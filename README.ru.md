[English](README.md)

# gramtty

Терминальный клиент Telegram на Rust и TDLib.

## Содержание

- [Возможности](#возможности)
- [Установка](#установка)
- [Утилиты времени выполнения](#утилиты-времени-выполнения)
- [Ключи Telegram API](#ключи-telegram-api)
- [Первый вход](#первый-вход)
- [Клавиши](#клавиши)
- [Настройка](#настройка)
- [Лицензия](#лицензия)

## Возможности

- Вход по QR-коду, номеру телефона, коду из Telegram, коду с почты и паролю 2FA
- Чаты, папки, топики форумов, секретные чаты
- Медиа прямо в терминале: фото, альбомы, видео, кружки, стикеры, голосовые, файлы (графика kitty; запасные варианты sixel, iTerm2 и полублоки); анимированные стикеры и GIF играют прямо в чате в kitty; видео, кружки, GIF, голосовые и музыка играют со звуком прямо в просмотрщике в kitty (libmpv) или в отдельном окне mpv
- Отправка: файлы, альбомы, вставка из буфера обмена, голосовые, стикеры и GIF; прогресс загрузки и отмена
- Реакции, ответы, редактирование, удаление, пересылка, закрепление, копирование, перевод, сохранение в «Избранное»
- Поиск по сообщениям, закреплённые сообщения, центр уведомлений
- Переходы к следующему непрочитанному чату, упоминанию или ответу, реакции
- Команды ботов и кнопки inline-клавиатуры
- Уведомления на рабочем столе и звук уведомления
- Приватность: «не отмечать прочитанным», режим призрака (нет статуса онлайн, «печатает» и отметок о прочтении)
- Локальная история сообщений, удалённых другими, и прежних версий изменённых; включается вручную (для защищённых и секретных чатов не ведётся)
- Темы (пресеты, в том числе следующий палитре терминала, и свой `theme.toml`), шесть раскладок сообщений, стили рамок, формы аватарок
- Переназначаемые клавиши

## Установка

Готовые бинарники публикуются для:

| Платформа | Что нужно |
|---|---|
| Linux x86_64 | glibc 2.39 или новее (Ubuntu 24.04, Debian 13, Fedora 40, Arch) |
| macOS на Apple Silicon | macOS 15 или новее |
| macOS на Intel | macOS 26 или новее |

Windows пока не поддерживается. Подойдёт любой терминал; для изображений, анимаций и видео в просмотрщике рекомендуется kitty.

### Скрипт установки (Linux, macOS)

```
curl -fsSL https://raw.githubusercontent.com/methamphetaminelab/gramtty/main/scripts/install.sh | sh
```

Скачивает последний релиз, сверяет контрольную сумму SHA-256 и кладёт `gramtty` в `~/.local/bin`. Другой каталог задаётся через `GRAMTTY_INSTALL_DIR`, конкретная версия через `GRAMTTY_VERSION` (например `1.1.0`).

### Homebrew (macOS, Linux)

```
brew tap methamphetaminelab/gramtty https://github.com/methamphetaminelab/gramtty
brew install gramtty
```

### AUR (Arch Linux)

```
yay -S gramtty-bin   # готовый бинарник
yay -S gramtty       # сборка из исходников
```

Подойдёт любой AUR-помощник (`paru -S ...`) или вручную: `git clone https://aur.archlinux.org/gramtty-bin.git && cd gramtty-bin && makepkg -si`.

### Скачать вручную

Возьмите архив для своей платформы на [странице релизов](https://github.com/methamphetaminelab/gramtty/releases/latest), распакуйте и положите `gramtty` в каталог из `PATH`.

### Сборка из исходников

```
cargo build --release
./target/release/gramtty
```

Нужен Rust 1.99 или новее (`rust-version` в `Cargo.toml`). На Linux также нужны статические `libc++.a` и `libc++abi.a` (TDLib линкуется статически):

- Arch: `pacman -S libc++ libc++abi`
- Debian/Ubuntu: `apt install libc++-dev libc++abi-dev libclang-dev`
- Fedora: `dnf install libcxx-static libcxxabi-static`
- Другое расположение: `LIBCXX_DIR` указывает на каталог с обоими файлами

## Утилиты времени выполнения

Все необязательны и ищутся в `PATH`; без утилиты не работает только её функция.

| Для чего | Linux | macOS |
|---|---|---|
| Воспроизведение со звуком прямо в просмотрщике (только kitty) | `libmpv` (пакет `mpv`), загружается динамически | `libmpv` (`brew install mpv`) |
| Воспроизведение в отдельном окне (`x`) | `mpv` | `mpv` |
| Превью видео и стикеров, WebM-стикеры и GIF в чате, миниатюры при отправке | `ffmpeg` | `ffmpeg` |
| Анализ медиа перед отправкой | `ffprobe` | `ffprobe` |
| Запись голосовых | `pw-record` (PipeWire) и `opusenc` или `ffmpeg` с libopus | `ffmpeg` (терминалу нужен доступ к микрофону) |
| Уведомления на рабочем столе | `notify-send` (libnotify) | `osascript` (встроен) |
| Звук уведомления | `pw-play` или `canberra-gtk-play` | `afplay` (встроен) |
| Копирование и вставка `Ctrl+V` | `wl-copy`, `wl-paste` (wl-clipboard); только Wayland | `pbcopy`, `pbpaste` (встроены); только текст |
| Открытие файлов и ссылок во внешней программе | `xdg-open` | `open` (встроен) |

## Ключи Telegram API

Получите `api_id` и `api_hash` на https://my.telegram.org и положите в `~/.config/gramtty/api.toml`:

```toml
api_id = 12345
api_hash = "0123456789abcdef0123456789abcdef"
```

Либо задайте обе переменные `GRAMTTY_API_ID` и `GRAMTTY_API_HASH` (они приоритетнее файла).

## Первый вход

Показывается QR-код. В Telegram откройте Настройки > Устройства > Подключить устройство и отсканируйте его. Чтобы войти по номеру телефона, нажмите `p`.

## Клавиши

Как в vim. `Space` это лидер: после нажатия всплывает список всех продолжений.

| Клавиша | Действие |
|---|---|
| `1` `2` `3` `4` | фокус: папки, чаты, сообщения, информация о чате |
| `Tab` `h` `l` | следующая или предыдущая панель |
| `j` `k`, стрелки | движение |
| `Ctrl-d` `Ctrl-u`, `PgDn` `PgUp` | прокрутка |
| `gg` `G` | в начало, в конец |
| `/` | фильтр панели |
| `Enter` | открыть чат или топик; на сообщении меню действий |
| `Space Space` | найти чат |
| `Space /` | поиск по всем чатам |
| `i` | писать сообщение |
| `Enter` / `Shift-Enter`, `Alt-Enter` или `Ctrl-j` | отправить / перенос строки (в поле ввода) |
| `Esc` | выйти из поля ввода |
| `R` `e` `d` | ответить, изменить, удалить |
| `r` `f` `y` `P` | реакция, переслать, копировать текст, закрепить или открепить |
| `o` | открыть ссылку из сообщения |
| `v` / `x` | посмотреть вложение (медиа играет в нём) / воспроизвести в mpv или открыть |
| в просмотрщике: `Space` `←` `→` `↑` `↓` `m` | пауза, перемотка ∓5 с, громкость ±5, без звука |
| `Space u` | прикрепить файл (в поле ввода: `Ctrl-o`) |
| `Space v` | записать голосовое (в поле ввода: `Ctrl-r`) |
| `Space s` | стикеры и GIF (в поле ввода: `Ctrl-s`, `Ctrl-g`) |
| `Space k` | кнопки бота (в поле ввода: `Ctrl-k`) |
| `]u` `]m` `]r` | следующий непрочитанный чат, упоминание или ответ, реакция |
| `gp`, `Space P` | к закреплённому сообщению, список закреплённых |
| `Space n` | центр уведомлений |
| `Space m` | отметить чат прочитанным |
| `Space p` | профиль |
| `Space a` | действия для выбранного чата или сообщения |
| `Space 1` `2` `4` | показать или скрыть панели папок, чатов, информации |
| `Space r` | перерисовать экран |
| `Space o` | настройки |
| `Space L` | выйти из аккаунта |
| `q` | выход |

Клавиши переназначаются в `~/.config/gramtty/keymap.toml`, секция `[keymap]`: `leader = "<C-x>"`, `ReplyMessage = "<leader>R"`, `Quit = false` снимает привязку. Имена действий смотрите в исходниках (`src/keymap.rs`).

## Настройка

Файлы в `~/.config/gramtty/` (учитывается `$XDG_CONFIG_HOME`):

- `api.toml`: ключи API
- `config.toml`: настройки; `Space o` меняет и сохраняет его. Секции: `[notifications]`, `[privacy]`, `[appearance]`, `[navigation]`, `[chats]`, `[media]`. Неизвестные ключи отвергаются
- `theme.toml`: своя тема, работает при `appearance.theme = "custom"`. Перечитывается при сохранении, перезапуск не нужен
- `keymap.toml`: клавиши

Данные (база TDLib, логи, история удалённых сообщений, временные файлы отправки): `~/.local/share/gramtty` (`$XDG_DATA_HOME`, либо `GRAMTTY_DATA_DIR`).

`gramtty --print-theme [terminal|default|high-contrast|light|mono|custom]` печатает тему в TOML, это основа для `theme.toml`. `Space o` > «Write theme.toml» сохраняет текущую тему туда же.

Основные параметры:

- `notifications.desktop`, `sound`, `volume` (0-100), `sound_file`: уведомления и их звук
- `privacy.dont_mark_read`, `ghost_mode`: отметки о прочтении и статус онлайн
- `privacy.keep_deleted`, `keep_deleted_limit` (по умолчанию 5000): локальная история удалённых и изменённых сообщений
- `privacy.hide_blocked`, `hide_stories`, `hide_phone`
- `appearance.theme`: `terminal` (по умолчанию), `default`, `high-contrast`, `light`, `mono`, `custom`
- `appearance.layout`: `classic`, `cards`, `compact`, `minimal`, `split`, `bubbles` (по умолчанию)
- `appearance.border_style`, `avatar_shape`, `seconds`, `show_ids`, `exact_numbers`
- `appearance.folder_counter` (`chats`, `messages`, `off`), `folder_counter_unmuted_only`, `hide_all_chats`
- `navigation.open_chat_focus` (`smart`, `composer`, `messages`), `after_send_next_unread`, `skip_muted`
- `chats.translate_to`, `photos_as_files`, `save_to_chat`

Протокол изображений определяется по терминалу; принудительно: `GRAMTTY_IMAGE_PROTOCOL` = `auto`, `kitty`, `sixel`, `iterm2` или `halfblocks`.

## Лицензия

GPL-3.0-or-later, см. `LICENSE`. Звук уведомления взят из Telegram Desktop (GPL-3.0); см. `NOTICE`.
