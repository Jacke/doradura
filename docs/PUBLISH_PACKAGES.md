# Публикация `dora` в пакетные менеджеры

Инструкция описывает полный процесс публикации бинарного релиза `dora` в:
- **Homebrew** (macOS + Linux)
- **AUR** (Arch Linux / Manjaro)
- **apt / deb** (Ubuntu, Debian)

---

## Подготовка: первый релиз

### 1. Убедиться что всё на месте

```bash
# Проверить версию в Cargo.toml
grep '^version' crates/doratui/Cargo.toml
# → version = "0.6.0"

# Проверить --version работает
cargo run -p doratui -- --version
# → dora 0.6.0
```

### 2. Создать тег и запустить CI

```bash
git tag tui-v0.6.0
git push origin tui-v0.6.0
```

GitHub Actions запустит `.github/workflows/tui-release.yml` и создаст GitHub Release
с архивами:
- `dora-aarch64-apple-darwin.tar.gz`
- `dora-x86_64-apple-darwin.tar.gz`
- `dora-x86_64-unknown-linux-gnu.tar.gz`
- `checksums.txt`

### 3. Забрать SHA256 из релиза

После завершения CI:

```bash
# Через gh CLI
gh release download tui-v0.6.0 --pattern checksums.txt
cat checksums.txt
```

Будет что-то вроде:
```
abc123...  dora-aarch64-apple-darwin.tar.gz
def456...  dora-x86_64-apple-darwin.tar.gz
ghi789...  dora-x86_64-unknown-linux-gnu.tar.gz
```

Сохрани все три хеша — они понадобятся ниже.

---

## Homebrew

### Шаг 1 — Создать tap-репозиторий

1. Перейти на GitHub → **New repository**
2. Имя: `homebrew-dora` (обязательно с префиксом `homebrew-`)
3. Репозиторий должен быть **публичным**
4. Создать файл `Formula/dora.rb` (пустой или сразу с формулой)

```bash
gh repo create Jacke/homebrew-dora --public
```

### Шаг 2 — Написать формулу

Файл `Formula/dora.rb` в репозитории `Jacke/homebrew-dora`:

```ruby
class Dora < Formula
  desc "Beautiful TUI for media downloading (yt-dlp + ffmpeg)"
  homepage "https://github.com/Jacke/doradura"
  version "0.6.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/Jacke/doradura/releases/download/tui-v0.6.0/dora-aarch64-apple-darwin.tar.gz"
      sha256 "abc123..."  # ← вставить реальный хеш
    end
    on_intel do
      url "https://github.com/Jacke/doradura/releases/download/tui-v0.6.0/dora-x86_64-apple-darwin.tar.gz"
      sha256 "def456..."  # ← вставить реальный хеш
    end
  end

  on_linux do
    url "https://github.com/Jacke/doradura/releases/download/tui-v0.6.0/dora-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "ghi789..."  # ← вставить реальный хеш
  end

  depends_on "yt-dlp"
  depends_on "ffmpeg"

  def install
    bin.install "dora"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/dora --version 2>&1", 0)
  end
end
```

Шаблон с плейсхолдерами лежит в `packaging/homebrew/dora.rb`.

### Шаг 3 — Опубликовать

```bash
git clone https://github.com/Jacke/homebrew-dora
cd homebrew-dora
mkdir -p Formula
# ... заполнить Formula/dora.rb реальными sha256 ...
git add Formula/dora.rb
git commit -m "dora 0.6.0"
git push
```

### Шаг 4 — Проверить

```bash
brew tap Jacke/dora
brew install dora
dora --version
# → dora 0.6.0
```

### Автоматическое обновление при следующих релизах

Добавить секрет `HOMEBREW_TAP_TOKEN` в настройках репозитория `Jacke/doradura`:
- GitHub → Settings → Secrets and variables → Actions → New repository secret
- Name: `HOMEBREW_TAP_TOKEN`
- Value: Personal Access Token с правами `repo` на репозиторий `Jacke/homebrew-dora`

После этого workflow `tui-release.yml` будет автоматически обновлять формулу
при каждом новом теге `tui-v*`.

---

## AUR (Arch Linux / Manjaro)

### Шаг 1 — Зарегистрировать аккаунт на AUR

1. Перейти на https://aur.archlinux.org/register/
2. Создать аккаунт
3. Добавить SSH-ключ: https://aur.archlinux.org/account/ → SSH Public Key

```bash
# Если нет SSH-ключа
ssh-keygen -t ed25519 -C "iamjacke@gmail.com"
cat ~/.ssh/id_ed25519.pub
# → скопировать в AUR → Account → SSH Public Key
```

### Шаг 2 — Клонировать AUR-репозиторий

```bash
# Пакет ещё не существует — клонирование создаст пустой репозиторий
git clone ssh://aur@aur.archlinux.org/dora-bin.git
cd dora-bin
```

> **Почему `dora-bin`?** Суффикс `-bin` — AUR-конвенция для pre-built бинарных пакетов.
> Это отличает их от пакетов, которые собираются из исходников.

### Шаг 3 — Заполнить PKGBUILD

Шаблон лежит в `packaging/aur/PKGBUILD`. Вставить реальный sha256:

```bash
cp /path/to/doradura/packaging/aur/PKGBUILD ./PKGBUILD
# Заменить плейсхолдер реальным хешем
sed -i "s/<sha256-x86_64-unknown-linux-gnu>/ghi789.../" PKGBUILD
```

Или отредактировать вручную — строка `sha256sums=('...')`.

### Шаг 4 — Сгенерировать .SRCINFO

`.SRCINFO` — обязательный файл-манифест, который генерируется из PKGBUILD:

```bash
# Нужен установленный пакет base-devel (на Arch/Manjaro он обычно есть)
makepkg --printsrcinfo > .SRCINFO
```

Если публикуешь с macOS/Ubuntu (без `makepkg`), можно сгенерировать вручную:

```
.SRCINFO должен содержать:
pkgbase = dora-bin
pkgdesc = Beautiful TUI for media downloading (yt-dlp + ffmpeg)
pkgver = 0.6.0
pkgrel = 1
url = https://github.com/Jacke/doradura
arch = x86_64
license = MIT
depends = yt-dlp
depends = ffmpeg
provides = dora
conflicts = dora
source = dora-bin-0.6.0.tar.gz::https://...
sha256sums = ghi789...

pkgname = dora-bin
```

Проще запустить `makepkg` в Docker-контейнере с Arch:

```bash
docker run --rm -v "$PWD:/pkg" -w /pkg archlinux:latest bash -c \
  "pacman -Sy --noconfirm base-devel && makepkg --printsrcinfo > .SRCINFO"
```

### Шаг 5 — Запушить

```bash
git add PKGBUILD .SRCINFO
git commit -m "Initial release v0.6.0"
git push
```

### Шаг 6 — Проверить

На Arch/Manjaro:
```bash
yay -S dora-bin
# или
paru -S dora-bin

dora --version
# → dora 0.6.0
```

### Обновление при новой версии

```bash
# В репозитории aur/dora-bin:
# 1. Обновить pkgver и pkgrel в PKGBUILD
# 2. Обновить sha256sums
# 3. Пересоздать .SRCINFO
makepkg --printsrcinfo > .SRCINFO
git add PKGBUILD .SRCINFO
git commit -m "Update to v0.7.0"
git push
```

---

## Ubuntu / Debian (apt)

Официальная публикация в Ubuntu PPA — самый правильный способ для `apt install`.

### Вариант A — Ubuntu PPA (Launchpad)

PPA требует сборку из исходников через `dpkg-buildpackage`. Для бинарного
Rust-проекта это избыточно сложно. **Рекомендуется только если** нужен
именно `apt install` без добавления внешних источников.

> Если цель — просто чтобы пользователи могли установить через apt,
> проще использовать Вариант B.

Краткий процесс:
1. Создать аккаунт на https://launchpad.net/
2. Создать PPA: My PPAs → Create a new PPA
3. Подготовить `debian/` директорию с `control`, `rules`, `changelog`
4. Собрать source package: `debuild -S`
5. Загрузить: `dput ppa:jacke/dora *.changes`

Полная инструкция: https://help.launchpad.net/Packaging/PPA

---

### Вариант B — apt через прямую ссылку (без PPA) ✅ Рекомендуется

Самый простой способ — shell-скрипт, который скачивает `.deb` или бинарник
напрямую с GitHub Releases. Пользователи запускают:

```bash
curl -sSfL https://github.com/Jacke/doradura/releases/latest/download/dora-installer.sh | sh
```

Скрипт создаётся workflow `tui-release.yml` автоматически (shell installer).

Или чуть более правильно — создать `.deb` пакет:

### Создание .deb вручную

```bash
VERSION=0.6.0
ARCH=amd64

# Скачать бинарник
curl -L "https://github.com/Jacke/doradura/releases/download/tui-v${VERSION}/dora-x86_64-unknown-linux-gnu.tar.gz" \
  | tar xz --strip-components=1

# Создать структуру .deb
mkdir -p dora_${VERSION}_${ARCH}/usr/bin
mkdir -p dora_${VERSION}_${ARCH}/DEBIAN

cp dora dora_${VERSION}_${ARCH}/usr/bin/

cat > dora_${VERSION}_${ARCH}/DEBIAN/control << EOF
Package: dora
Version: ${VERSION}
Architecture: ${ARCH}
Maintainer: Stan <iamjacke@gmail.com>
Depends: yt-dlp, ffmpeg
Description: Beautiful TUI for media downloading (yt-dlp + ffmpeg)
 dora is a terminal UI for downloading audio and video
 using yt-dlp and ffmpeg as backends.
Homepage: https://github.com/Jacke/doradura
EOF

# Собрать .deb
dpkg-deb --build --root-owner-group dora_${VERSION}_${ARCH}
# → dora_0.6.0_amd64.deb
```

Загрузить `.deb` в GitHub Release вместе с другими архивами.

Пользователи устанавливают:
```bash
# Скачать и установить .deb
curl -LO https://github.com/Jacke/doradura/releases/latest/download/dora_0.6.0_amd64.deb
sudo dpkg -i dora_0.6.0_amd64.deb
# Установить зависимости если нет
sudo apt-get install -f
```

### Автоматизация .deb в CI

Добавить в `tui-release.yml` шаг сборки `.deb` после сборки Linux-бинарника:

```yaml
- name: Build .deb package
  if: matrix.target == 'x86_64-unknown-linux-gnu'
  run: |
    VERSION="${GITHUB_REF_NAME#tui-v}"
    mkdir -p dora_${VERSION}_amd64/usr/bin dora_${VERSION}_amd64/DEBIAN
    cp target/${{ matrix.target }}/release/dora dora_${VERSION}_amd64/usr/bin/
    cat > dora_${VERSION}_amd64/DEBIAN/control << EOF
    Package: dora
    Version: ${VERSION}
    Architecture: amd64
    Maintainer: Stan <iamjacke@gmail.com>
    Depends: yt-dlp, ffmpeg
    Description: Beautiful TUI for media downloading (yt-dlp + ffmpeg)
    Homepage: https://github.com/Jacke/doradura
    EOF
    dpkg-deb --build --root-owner-group dora_${VERSION}_amd64
    sha256sum dora_${VERSION}_amd64.deb > dora_${VERSION}_amd64.deb.sha256
```

---

## Чеклист перед первым релизом

- [ ] `dora --version` возвращает правильную версию
- [ ] Тег запушен: `git push origin tui-v0.6.0`
- [ ] GitHub Actions завершился успешно (зелёный)
- [ ] Архивы появились в GitHub Releases
- [ ] `checksums.txt` содержит все три хеша
- [ ] Репозиторий `Jacke/homebrew-dora` создан
- [ ] `Formula/dora.rb` заполнен реальными sha256
- [ ] `brew tap Jacke/dora && brew install dora` работает
- [ ] AUR: PKGBUILD + .SRCINFO запушены
- [ ] `yay -S dora-bin` работает на Arch
- [ ] Опционально: `.deb` загружен в GitHub Release

---

## Обновление версии (стандартный процесс)

```bash
# 1. Поднять версию в Cargo.toml
# crates/doratui/Cargo.toml: version = "0.7.0"

# 2. Закоммитить
git add crates/doratui/Cargo.toml
git commit -m "feat: ... (tui v0.7.0)"

# 3. Тег
git tag tui-v0.7.0
git push && git push origin tui-v0.7.0

# 4. CI автоматически:
#    - Собирает бинарники
#    - Создаёт GitHub Release
#    - Обновляет homebrew-dora/Formula/dora.rb (если HOMEBREW_TAP_TOKEN задан)

# 5. AUR — обновить вручную (5 минут):
#    - Поднять pkgver в PKGBUILD
#    - Обновить sha256sums (из checksums.txt нового релиза)
#    - makepkg --printsrcinfo > .SRCINFO
#    - git commit && git push
```
