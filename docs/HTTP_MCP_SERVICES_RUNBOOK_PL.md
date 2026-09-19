# Vetcoders HTTP MCP Services Runbook

**Wersja:** 1.0.0 (Sierpień 2026)
**Status:** Kanoniczna Architektura Multi-Agent & Multi-Machine
**Zakres:** `aicx-mcp` (Pamięć & Wyszukiwanie Intencji) + `loctree-mcp` / `loct watch --http` (Strukturalna Percepcja Kodu)

---

## 1. Dlaczego Streamable HTTP zamiast Stdio?

W środowiskach wieloagentowych (flota 10–50 agentów Codex, Claude, Gemini, Antigravity, subagenci):

| Cecha | Tryb Stdio | Tryb Streamable HTTP (Zalecany) |
| :--- | :--- | :--- |
| **Liczba procesów** | 1 proces per agent (50 agentów = 50 procesów w `ps aux`) | **1 centralny demon na maszynie** |
| **Zużycie RAM** | Duplikacja pamięci per proces (50 × 50MB+) | **Współdzielona pamięć RAM (~30–60MB na proces)** |
| **Zarządzanie cache** | Wyścigi dyskowe o pliki `.cache` | **Jedno in-memory `Arc<Snapshot>` w pamięci RAM** |
| **Szybkość zapytań** | Start binarki + cold read per wywołanie | **< 1–5 ms (natychmiastowy odczyt grafu z RAM)** |
| **Dostęp zdalny** | Tylko lokalnie na maszynie roboczej | **Dostęp po sieci / Tailscale (`dragon`, `div0`, `sztudio`)** |
| **Skalowalność** | Dławi CPU i wyczerpuje limity deskryptorów plików | **Tokio + Axum bez problemu obsługuje tysiące zapytań** |

---

## 2. Serwis 1: `aicx-mcp` (Silnik Pamięci i Wyszukiwania Intencji)

Instalowana usługa domyślnie nasłuchuje wyłącznie na `127.0.0.1`. Wystawienie
jej do Tailnetu jest jawną decyzją operatora:

```bash
AICX_MCP_HOST="$(tailscale ip -4)" \
  AICX_MCP_ALLOWED_HOSTS="$(hostname -s),$(tailscale ip -4),localhost,127.0.0.1" \
  make install-service
```

Adres klienta pozostaw na loopbacku, chyba że klient faktycznie działa na innej
zaufanej maszynie Tailnetu. Nie commituj wartości wygenerowanego tokena — użyj
pliku tokena albo mechanizmu zmiennej/nagłówka danego klienta.

### 2.1. Odświeżanie na Żywo (Live Refresh)
* **Czy `aicx serve` odświeża się na żywo?** **Tak.** `aicx serve` nie zamraża stanu indeksu w RAM na stałe. Każde zapytanie narzędzia (`aicx_search`, `aicx_steer`, `aicx_intents`) odpytuje bezpośrednio bieżący stan bazy Tantivy oraz wektorów z dysku (`~/.aicx/`).
* **Cykl indeksowania:** Tryb HTTP ma własną ograniczoną pętlę async: co 5 minut odświeża gorące 48 godzin katalogu i inkrementalnie publikuje indeks leksykalny. Blokująca praca plikowa i indeksowanie wykonują się poza workerami requestów Tokio. Interwał zmienisz przez `--refresh-interval-seconds`; `--no-auto-refresh` ma sens tylko wtedy, gdy świeżość ma innego jawnego właściciela.

### 2.2. Instalacja i Cele w Makefile
* **Standardowa instalacja (z kreatorem):**
  ```bash
  cd /Volumes/vc-workspace/Loctree/aicx
  make install
  ```
  *(Uruchamia `install.sh`, instaluje binarki, konfiguruje klientów MCP i rejestruje serwis HTTP MCP w launchd. Ten serwer jest właścicielem odświeżania indeksu).*

* **Zarządzanie serwisem LaunchAgent:**
  ```bash
  make install-service    # Instaluje i startuje LaunchAgent com.loctree.aicx.mcp
  make uninstall-service  # Zatrzymuje i wyrejestrowuje LaunchAgent com.loctree.aicx.mcp
  make install-schedule   # Legacy: osobny refresh tylko gdy nie działa serwer HTTP
  ```

### 2.3. Weryfikacja Działania (Smoke Test)
```bash
# 1. Healthcheck
curl -s http://127.0.0.1:8044/health && echo " (Health: OK)"

# 2. Handshake MCP
curl -s \
  -H "Authorization: Bearer $(cat ~/.aicx/auth-token)" \
  -H "Accept: application/json, text/event-stream" \
  -H "Content-Type: application/json" \
  -X POST http://127.0.0.1:8044/mcp \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke-test","version":"1.0"}}}'
```

---

## 3. Serwis 2: `loctree-mcp` (Uniwersalna Percepcja Kodu)

### 3.0. Uwierzytelnianie i polityka bind (przeczytaj zanim cokolwiek wystawisz)

`loctree-mcp --transport http` serwuje 12 narzędzi MCP czytających system plików
plus `/context_pack`. Obie trasy czytają dowolny katalog projektu podany przez
klienta — to powierzchnia odczytu kodu, nie healthcheck.

Serwer wybiera postawę na podstawie adresu bind, przy starcie, **zanim powstanie
gniazdo**:

| `--bind` | tokeny skonfigurowane | `--allow-unauthenticated` | zachowanie |
| :--- | :--- | :--- | :--- |
| loopback (`127.0.0.0/8`, `::1`) | nie | — | startuje otwarty — lokalna ścieżka zero-config |
| loopback | tak | — | bearer auth wymuszony |
| nie-loopback (w tym `0.0.0.0`, `::`) | tak | — | bearer auth wymuszony |
| nie-loopback | nie | tak | startuje otwarty, głośny log `SECURITY:` |
| **nie-loopback** | **nie** | **nie** | **odmawia startu; port nie zostaje otwarty** |

Adres bind, którego nie da się rozwiązać, traktowany jest jako nie-loopback
(fail-safe). `--allow-unauthenticated` jest ignorowane, gdy tokeny istnieją —
skonfigurowanego auth nie da się po cichu obniżyć.

Odmowa wygląda tak:

```console
$ loctree-mcp --transport http --bind 0.0.0.0:5174
[loctree-mcp] Error: refusing to start: --bind 0.0.0.0:5174 is not loopback and no bearer tokens are configured.
...
Pick one:
  1. Mint a token, then restart: loctree-mcp token create --id <name> --scope context-read
  2. Export one shared token: LOCTREE_MCP_AUTH_TOKEN=<secret>
  3. Keep it local: --bind 127.0.0.1:5174
  4. Accept the risk out loud: --allow-unauthenticated (or LOCTREE_MCP_ALLOW_UNAUTHENTICATED=1)
```

#### Wystawianie tokena

```bash
# Domyślny store: ~/.rmcp-servers/loctree-mcp/tokens.json (hash argon2id at rest)
loctree-mcp token create --id dragon-tailnet --scope context-read

# id:         dragon-tailnet
# store:      ~/.rmcp-servers/loctree-mcp/tokens.json
# scopes:     context-read
# namespaces: *
# expires:    never
# token:      loct_2f1c…            <- pokazany raz, nie do odzyskania

loctree-mcp token list
loctree-mcp token rotate --id dragon-tailnet
loctree-mcp token revoke --id dragon-tailnet
```

Przydatne flagi: `--expires-in-days N`, powtarzalne `--scope` / `--namespace`,
`--token-store PATH` (albo `LOCTREE_MCP_TOKEN_STORE`). Wszystkie 12 narzędzi MCP
jest read-only, więc `context-read` to cała powierzchnia; `tool-execute` /
`cli-full` / `admin` są zarezerwowane pod przyszłą stronę zapisu.

#### Zmienne środowiskowe

| Zmienna | Efekt |
| :--- | :--- |
| `LOCTREE_MCP_TOKEN_STORE` | ścieżka token store (to samo co `--token-store`) |
| `LOCTREE_MCP_AUTH_TOKEN` | jeden współdzielony token, bez pliku store; mapuje się na wpis wildcard-admin i loguje ostrzeżenie o deprecjacji |
| `LOCTREE_MCP_ALLOW_UNAUTHENTICATED` | `1`/`true`/`yes`/`on` — to samo co `--allow-unauthenticated` |
| `LOCTREE_MCP_ALLOWED_ROOTS` | istniejąca wcześniej, ortogonalna: ogranicza, jakie rooty projektów może podać klient |

`tools/install-mcp-service.sh` przekazuje wszystkie trzy zmienne auth do
`EnvironmentVariables` LaunchAgenta (launchd nie dziedziczy twojego shella)
i ostrzega, gdy `LOCTREE_MCP_BIND` nie jest loopbackiem.

#### Czego to NIE chroni

Bez ozdobników:

* **Brak TLS.** Proces mówi czystym HTTP. Na bindzie nie-loopback tokeny lecą
  otwartym tekstem, chyba że coś przed nim terminuje TLS. Zdalne wystawienie to
  odpowiedzialność reverse proxy (`caddy`/`nginx`) albo tailnetu — nie tej binarki.
* **Brak egzekwowania namespace.** Tokeny niosą ACL `namespaces` i store go
  honoruje, ale granica HTTP sprawdza wyłącznie uwierzytelnienie i scope. `/mcp`
  niesie projekt w ciele JSON-RPC, którego middleware nie parsuje, więc
  wymuszanie tylko na `/context_pack` byłoby gorsze niż niewymuszanie w ogóle.
  Traktuj każdy ważny token jako sięgający każdego projektu, który proces serwera
  może odczytać. Twardą granicę daje `LOCTREE_MCP_ALLOWED_ROOTS`.
* **Brak rate limitingu, audit logu i autoryzacji per-request.** Odmowy są
  logowane; udane odczyty nie.
* **Transport stdio nie ma auth i nie potrzebuje** — to proces potomny klienta,
  z uprawnieniami klienta.
* **Loopback to nadal zaufanie z lokalności.** Każdy lokalny proces lub użytkownik
  hosta może odpytać nieuwierzytelniony serwer na loopbacku.

### 3.1. Dwa Modele Działania

#### Tryb A: Uniwersalny Serwer Centralny (Współdzielony dla wszystkich repozytoriów)
Jeden serwer na porcie `5174` obsługuje dowolne repozytorium. Każde wywołanie narzędzia (`slice`, `impact`, `find`, `focus`, `repo-view`, `tree`) przyjmuje parametr `project: "/sciezka/do/repo"`.
* **Pojemność:** Działa z flagą `--snapshot-cache-capacity 20`, trzymając w RAM snapshoty dla nawet 20 repozytoriów jednocześnie.

#### Tryb B: Dedykowany Watcher per-repo (`loct watch --http`)
W katalogu aktywnie rozwijanego projektu:
```bash
loct watch --http --port 5174 &
```
* Obserwuje zmiany w plikach i natychmiast przelicza snapshot po zapisie.
* Samorządnie nadzoruje proces potomny `loctree-mcp` na `127.0.0.1:5174/mcp`.

### 3.2. Instalacja i Cele w Makefile
* **Standardowa instalacja w `loctree-suite`:**
  ```bash
  cd /Volumes/vc-workspace/Loctree/loctree-suite
  make install-all
  ```
  *(Kompiluje `loct`, `loctree`, `loctree-mcp`, `loctree-lsp`, podpisuje binarki i rejestruje serwis launchd `com.loctree.loctree.mcp`).*

* **Zarządzanie serwisem LaunchAgent:**
  ```bash
  make install-service    # Instaluje i startuje LaunchAgent com.loctree.loctree.mcp
  make uninstall-service  # Zatrzymuje i wyrejestrowuje LaunchAgent com.loctree.loctree.mcp
  ```

* **Wystawienie na tailnet (jawne, uwierzytelnione):**
  ```bash
  loctree-mcp token create --id dragon-tailnet --scope context-read   # skopiuj token
  LOCTREE_MCP_BIND="$(tailscale ip -4):5174" make install-service
  ```
  Bez tokena w store ta instalacja da serwis, który odmówi startu — celowo.
  Sprawdź `~/.loctree/logs/loctree-serve-http.log`.

### 3.3. Weryfikacja Działania (Smoke Test)
```bash
# Loopback bez tokenów — nagłówek niepotrzebny.
curl -s \
  -H "Accept: application/json, text/event-stream" \
  -H "Content-Type: application/json" \
  -X POST http://127.0.0.1:5174/mcp \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke-test","version":"1.0"}}}'

# Gdziekolwiek auth jest skonfigurowane — /mcp i /context_pack wymagają nagłówka.
curl -s \
  -H "Authorization: Bearer $LOCTREE_MCP_TOKEN" \
  -H "Accept: application/json, text/event-stream" \
  -H "Content-Type: application/json" \
  -X POST http://127.0.0.1:5174/mcp \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke-test","version":"1.0"}}}'

# Bez nagłówka spodziewaj się 401 z `WWW-Authenticate: Bearer`:
curl -s -o /dev/null -w '%{http_code}\n' \
  "http://127.0.0.1:5174/context_pack?project=/sciezka/do/repo"
```
---

## 4. Matryca Konfiguracji Klientów MCP

### 4.1. Antigravity / Gemini IDE (`~/.gemini/config/mcp_config.json`)

```json
{
  "mcpServers": {
    "aicx-http": {
      "type": "streamable-http",
      "url": "http://127.0.0.1:8044/mcp",
      "headers": {
        "Authorization": "Bearer <TOKEN_Z_~/.aicx/auth-token>"
      }
    },
    "loctree-http": {
      "type": "streamable-http",
      "url": "http://127.0.0.1:5174/mcp",
      "headers": {
        "Authorization": "Bearer <LOCTREE_MCP_TOKEN>"
      }
    }
  }
}
```

### 4.2. Claude Code (`~/.claude/mcp.json` lub projektowe `.mcp.json`)

```json
{
  "mcpServers": {
    "aicx": {
      "url": "http://127.0.0.1:8044/mcp",
      "headers": {
        "Authorization": "Bearer <TOKEN_Z_~/.aicx/auth-token>"
      }
    },
    "loctree": {
      "url": "http://127.0.0.1:5174/mcp",
      "headers": {
        "Authorization": "Bearer <LOCTREE_MCP_TOKEN>"
      }
    }
  }
}
```

Lub przez CLI:
```bash
claude mcp add --scope user --transport http \
  --header "Authorization: Bearer $(cat ~/.aicx/auth-token)" \
  aicx http://127.0.0.1:8044/mcp

claude mcp add --scope user --transport http \
  --header "Authorization: Bearer $LOCTREE_MCP_TOKEN" \
  loctree http://127.0.0.1:5174/mcp
```

Nagłówek `Authorization` jest wymagany wszędzie tam, gdzie serwer ma
skonfigurowane tokeny (czyli zawsze przy bindzie nie-loopback). Na serwerze
loopback bez tokenów jest po prostu ignorowany, więc ta sama konfiguracja
klienta działa w obu trybach.

### 4.3. Codex (`~/.codex/config.toml`)

```toml
[mcp_servers.aicx]
url = "http://127.0.0.1:8044/mcp"
bearer_token_env_var = "AICX_MCP_TOKEN"

[mcp_servers.aicx.tools.aicx_search]
approval_mode = "approve"

[mcp_servers.aicx.tools.aicx_steer]
approval_mode = "approve"

[mcp_servers.aicx.tools.aicx_rank]
approval_mode = "approve"

[mcp_servers.loctree]
url = "http://127.0.0.1:5174/mcp"
bearer_token_env_var = "LOCTREE_MCP_TOKEN"
```

---

## 5. Serwisy macOS launchd LaunchAgents

Serwisy są zarejestrowane jako per-user LaunchAgents w `~/Library/LaunchAgents/` z parametrami `RunAtLoad=true` i `KeepAlive=true`:

| Identyfikator Serwisu | Polecenie | Domyślny Port | Cel Logowania |
| :--- | :--- | :--- | :--- |
| **`com.loctree.aicx.mcp`** | `aicx serve --transport http` | `8044` | `~/.aicx/logs/aicx-serve-http.log` |
| **`com.loctree.loctree.mcp`** | `loctree-mcp --transport http` | `5174` | `~/.loctree/logs/loctree-serve-http.log` |

Oba serwisy domyślnie bindują loopback i żaden nie terminuje TLS w procesie.

Poprzedni timer `io.vetcoders.aicx.reindex` jest usuwany przy instalacji serwera HTTP, aby nie utrzymywać dwóch konkurujących writerów indeksu.

---

## 6. Przydatne Aliasy i Skrypty Operacyjne

Dodaj poniższe aliasy do swojego `~/.zshrc`:

```bash
# Sprawdzenie statusu nasłuchujących portów MCP
alias mcp-status='lsof -nP -iTCP:8044,5174 -sTCP:LISTEN'

# Podgląd logów obu serwerów na żywo
alias mcp-logs='tail -f ~/.aicx/logs/aicx-serve-http.log ~/.loctree/logs/loctree-serve-http.log'

# Sprawdzenie stanu serwisów w launchd
alias mcp-launchd='launchctl list | grep -E "vetcoders|loctree"'

# Restart obu serwisów launchd
alias mcp-restart='launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.loctree.aicx.mcp.plist 2>/dev/null; \
  launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.loctree.aicx.mcp.plist; \
  launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.loctree.loctree.mcp.plist 2>/dev/null; \
  launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.loctree.loctree.mcp.plist; \
  echo "🚀 Serwisy zrestartowane w launchd"'
```
