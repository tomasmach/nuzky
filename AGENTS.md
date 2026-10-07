# CapOpen

CapOpen je open-source desktopový střihač videa ve stylu CapCutu (GPL-3.0-or-later). Rust engine (`crates/engine`: model projektu, editace a undo, dekódování přes FFmpeg, wgpu kompozitor, mix zvuku, export), lokální Whisper (`crates/analysis`), editační autorita (`crates/session`), nástroje pro AI agenty (`crates/mcp`, `crates/cli`), desktop v Tauri 2 (`src-tauri`) a UI v React 19 + zustand (`src`). Stav: prototyp. Ověřený je Linux, macOS a Windows se zatím jen kompilují.

Uživatel stříhá videa z telefonu a často k tomu pouští AI agenta, který stříhá za něj. Změna málokdy patří jen jednomu povrchu: UI, engine, MCP nástroje, CLI a export sdílejí stejný projekt.

Tenhle soubor je pro vývoj. `skills/capopen-edit/` je návod pro agenta, který v CapOpen stříhá video.

## Co nikdy neobětujeme

Co rozbiješ, rozbiješ někomu rozdělané video.

1. **Média a projekty uživatele.** Zdrojová média nikdy nepřepiš ani nesmaž. Export do existujícího souboru proběhne jen s výslovným souhlasem a přes dočasný `.capopen-part-*`. Projekt se ukládá atomicky a úprava se neztratí ani po pádu nebo zavření okna. Projekt uložený starší verzí se vždy otevře, takže nové pole modelu má `#[serde(default)]`. Změna formátu cache zvedne verzi v názvu souboru (`PCM_VERSION`), aby se stará data nečetla jako nová.
2. **Co vidíš, to exportuješ.** Náhled i export kreslí stejný renderer. Střih sedí na snímek podle skutečných časů (VFR a HEVC z telefonu), orientace a zrcadlení z metadat sedí v náhledu, miniaturách i exportu a zvuk drží synchron s obrazem. Dekódování a vykreslení porovnávej s referencí z FFmpeg, ne s vlastním odhadem. Selfie z přední kamery (EXIF 2, 4, 5, 7) se jednou místo zrcadlení otáčela.
3. **Soubory zvenku jsou nedůvěryhodné.** Projekt `.capopen` i médium může přijít od kohokoli. FFmpeg otevírá vstupy jen přes `media::open_input`, který povolí jen protokol `file`; review našlo, že sdílený projekt s cestou `http://…` nechal FFmpeg stahovat z cizího serveru. Nevalidní projekt nesmí shodit náhled ani appku (panika na barvě `#€`). Lokální IPC zůstává 0700/0600 s tokenem a nic neposlouchá na síti. Síť se používá jen pro stažení modelů s pevným SHA-256.
4. **AI agent pracuje pod kontrolou uživatele.** Každá změna z UI, od agenta i z CLI jde přes `ProjectSession` a validaci `EditCmd`. JSON projektu nikdo nepíše přímo. Běh agenta je živě vidět a celý se vrátí jedním krokem undo. Během běhu UI ukazuje zámek s důvodem a tlačítkem Stop. Zastavení agenta nebo jobu zabere nanejvýš pár sekund, i uprostřed Whisperu nebo analýzy.
5. **Klid a rychlost.** UI nikdy nečeká na dekódování, analýzu ani export. Pomalá práce běží jako job s procenty a jde zrušit. Rozpočty odezvy jsou v `docs/INTERACTION.md` (Response time). Přehrávání nepřekresluje celou appku s každým snímkem.

Pravidlo odsud, které bojuje s úkolem, neporušuj potichu. Řekni to nahlas.

## Čtyři způsoby, jak si ublížit

1. **Skutečná data uživatele.** Appka čte projekty, nedávné projekty a modely z `$XDG_DATA_HOME/capopen`, cache zvuku z `$XDG_CACHE_HOME/capopen` a IPC socket má v `$XDG_RUNTIME_DIR/capopen`. Appku, CLI ani `capopen mcp` nespouštěj se skutečnými adresáři uživatele: otevřela by jeho poslední projekt nebo se připojila k jeho běžící appce. `scripts/repro.py` všechno izoluje sám, ruční běh izoluj stejně. `~/.cache/capopen/deps` patří build skriptům, nemaž ho.
2. **Disk a souběžné buildy.** Každý `CARGO_TARGET_DIR` je celý build s FFmpeg, wgpu a whisper.cpp. Sedmnáct `target-*` adresářů jednou zabralo 235 GB. Používej `target/` svého checkoutu. Vlastní target adresář založ jen pro souběžný build a po práci ho smaž. Dvě plné testovací sady naráz nespouštěj a velké testovací soubory nedávej do `/tmp`, je v paměti.
3. **Okna a procesy na ploše.** Appka otevírá skutečná okna, nativní dialogy a notifikace. Proklikávej přes `scripts/repro.py`: headless gamescope a vypnutý D-Bus. Vite běží na pevném portu 1420 a `tauri-driver` na 4444. Obsazený port znamená cizí session; nezabíjej ji a počkej. Ukončuj jen vlastní procesy podle PID nebo skupiny.
4. **GitHub Actions.** Repo je soukromé a minuty mají rozpočet, macOS se počítá desetkrát. Job, který skončí za pár sekund bez logu, neběžel kvůli rozpočtu. Řekni mi to a kód kvůli tomu neopravuj.

## Jak to funguje

- Rust vlastní projekt. UI posílá `EditCmd` a dostává zpět nový snapshot. Agenti a CLI jdou přes stejný `ProjectSession` (`crates/session`), kde jsou i autosave, obnova po pádu a běhy agenta.
- Náhled kreslí vlákno v `src-tauri/src/engine.rs` přes wgpu (`crates/engine/src/render.rs`, `gpu.rs`) a posílá snímky přes loopback WebSocket s tajemstvím a kontrolou originu. Export používá stejný renderer.
- Zvuk každého souboru se jednou dekóduje do 48kHz cache (`crates/engine/src/audio.rs`). Přehrávání, waveformy, export i titulky míchají z ní. Cache patří souboru (cesta, velikost, mtime), ne jen ID assetu, protože agent může pod stejné ID vložit jiný soubor.
- Dlouhá práce běží jako job: export, titulky, příprava zvuku a modely v `src-tauri/src/jobs.rs`, práce agenta v `crates/session/src/jobs.rs`. Každý job hlásí průběh a reaguje na zrušení.
- Přístup každého MCP nástroje je v jedné tabulce `crates/mcp/src/rules.rs`. Návod pro agenty `skills/capopen-edit/SKILL.md` je zároveň `capopen://guide`. Architektura AI je v `docs/AI-ARCHITECTURE.md`.
- Chybové kódy jsou prefixy zpráv (`RUN_ACTIVE`, `CANCELLED`, `READ_ONLY`, `OUTPUT_EXISTS`…) a frontend je čte přes `startsWith`. Kód nepřejmenovávej bez úpravy všech míst, která ho čtou.

## Zasáhni všechny povrchy

Nejčastější vada je změna, která funguje jen na cestě, kterou jsi zkoušel.

- Nové pole modelu (`crates/engine/src/model.rs`): `#[serde(default)]`, typ v `src/lib/types.ts` (ruční kopie), validace v `crates/session/src/validate.rs`, MCP schéma a literály ve všech crates. Projdi náhled, miniatury (`src-tauri/src/thumbs.rs`), export i CLI `frame`.
- Nový `EditCmd`: validace, undo, MCP schéma a návod, frontend (`src/lib/api.ts`, `src/lib/store.ts`) a chování v `docs/INTERACTION.md`.
- Nový MCP nástroj: řádek v `rules.rs`, popis v `crates/mcp/src/lib.rs`, návod v `SKILL.md` a test přes stdio v `crates/cli/tests/mcp_stdio.rs`.
- Změnu chování nebo klávesy zapiš do `docs/INTERACTION.md` ve stejném PR, vizuál do `DESIGN.md`.
- Nový ovládací prvek respektuje zámek AI (`useLockReason`, `lockedProps` v `src/components/ui.tsx`), má cestu z klávesnice a ukazuje, proč je vypnutý.
- Nové čtení média jde přes `media::open_input` a má test na médium z telefonu (rotace, VFR, HEVC) proti referenci z FFmpeg.
- Platformy: Linux je ověřený, macOS a Windows CI jen kompiluje. Neříkej, že na nich něco funguje.

## Ověřování

- Nový checkout: `git config core.hooksPath .githooks`. Nástroje pro bránu na Fedoře a Nobaře: `sudo dnf install gamescope webkitgtk6.0 espeak-ng python3-pillow python3-xlib`, `cargo install --locked tauri-driver@2.1.0 cargo-deny@0.20.2` a gitleaks 8.30.1. První kompilace testů v čistém checkoutu trvá asi 7 minut.
- Bug reprodukuj přesně v toku, kde se stal. Když první oprava nezabere, přestaň hádat a najdi stav, který ho spouští.
- E2E jsou hlavní testy. Je to tok ve skutečné appce (`tests/e2e/<oblast>.py`, spouští ho `python3 scripts/repro.py <flow>`, `--list` ukáže toky) a integrační test nad skutečnou binárkou a médii (`crates/cli/tests/`, `crates/engine/tests/qa_*.rs`).
- Nová funkce, oprava UI, toku nebo bugu není hotová bez E2E scénáře, který by bez ní selhal. Přidej nový flow, nebo kontrolu do existujícího. Scénář ověřuje, co uživatel uvidí: stav projektu, uložený soubor, pixely náhledu nebo exportu. Nestačí, že nic nespadlo.
- Kontrola musí umět selhat. U nové kontroly jednou rozbij hlídané chování, nebo ji pusť před opravou, a ukaž, že selže.
- Unit test piš jen na čistou logiku, kterou E2E levně nepokryje (časy v mikrosekundách, hraniční výpočty, parsování), nebo na chybu, kterou E2E nechytí. Nepiš ho až po implementaci jako ozdobu. Nejdřív sepiš, jak může systém selhat.
- Do PR dej screenshot z `tmp-test/repro/<flow>/` a příkaz s revizí z `result.json`.
- Engine ověřuj na skutečných souborech proti referenci z FFmpeg. `crates/engine/tests/qa_*.rs` si média generují přes `ffmpeg`.
- Rust testy spouštěj cíleně: `cargo test -p <crate> <název>`. Testy s `#[ignore]` potřebují média a modely. Připrav je přes `scripts/fixtures.sh` a spusť `XDG_DATA_HOME=$PWD/tmp-test/xdg/data cargo test -p <crate> -- --ignored`. Při změně řeči, analýzy, miniatur nebo dekódování je spusť vždy.
- Plnou bránu `scripts/check.sh` spusť před merge do `main` a před vydáním. Obsahuje fmt, clippy, typy, build, všechny testy včetně ignorovaných, audity závislostí a všechny repro toky. Trvá desítky minut, pusť ji na pozadí.
- Frontend před pushem: `npm run build` (typy a Vite build).
- Výkon: nejdřív změř, pak měň, a uveď čísla před a po.

## Pull requesty a release

- Base je `main`. Větev založ z čerstvého `origin/main` a před otevřením PR ji rebasni.
- Před merge do `main` projde `scripts/check.sh` na přesném commitu, který mergeuješ. GitHub na PR testy nespouští, jen hledá tajné hodnoty.
- Pre-push hook (`.githooks/pre-push`, zapne ho `git config core.hooksPath .githooks`) hledá tajné hodnoty a blokuje video a audio soubory. Neobcházej ho přes `--no-verify`. Testovací média se generují a do gitu nepatří, protože to můžou být něčí soukromá videa.
- Release: stejná verze v `src-tauri/tauri.conf.json`, `Cargo.toml` a `package.json`, pak tag `v<verze>`. Tag spustí build a testy na Linuxu, macOS a Windows a připraví draft release. Publikuje jen člověk po ověření na každém systému podle `docs/BUILDING.md`.
- Komentáře botů ověř proti kódu: reálný nález oprav, falešný zamítni s důvodem.
- Uklízej jen po sobě. `tmp-test/` sdílí víc sessions.

## Vkus

- `DESIGN.md` a `docs/INTERACTION.md` jsou zákon. Komponenty z `src/components/ui.tsx` rozšiřuj, nekopíruj.
- UI je anglicky. Copy říká obyčejnými slovy, co se stane. Chyba říká, co dělat dál, bez interních kódů.
- Každá akce myší má i cestu z klávesnice.
- Licence je GPL-3.0-or-later. Nepřidávej závislost ani build FFmpeg, který s ní nejde dohromady (`libfdk_aac`, varianty „nonfree“). Licence Rust závislostí hlídá `cargo deny`.

## Code Review Rules

Platí pro automatické review PR (Codex). Najdi to, co by uživatel skutečně pocítil, a po pár kolech skonči.

- Piš nálezy P0 a P1: ztráta nebo přepsání médií či projektu, projekt, který se po změně neotevře, náhled odlišný od exportu, posun střihu nebo zvuku, pád nebo zaseknutí na běžném médiu z telefonu, nedůvěryhodný soubor, který dosáhne na síť nebo cizí soubory, AI agent obcházející zámek nebo undo, job, který nejde zastavit, a selhání, které kód spolkne bez zprávy uživateli.
- Rychlost hlídej vždy: práce na UI vlákně, překreslení celé appky za snímek, dekodér nebo alokace navíc na každý snímek, celé médium v paměti. Když to uživatel pozná na trhání nebo čekání, je to P1.
- Každý nález potřebuje dosažitelný scénář a dopad. Chybějící test sám o sobě není vada.
- Nepiš teoretické souběhy bez realistického scénáře, exotické vstupy bez doloženého výskytu, styl, pojmenování ani rozšíření za zadání PR. `std::fs::rename` na Windows existující cíl přepíše, to není nález.
- Při opakovaném review se drž commitů od posledního kola a nevracej se k vyřešeným vláknům.
