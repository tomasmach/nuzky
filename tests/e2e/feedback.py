"""Send feedback. The foot of the home sidebar and the editor's top bar open one menu: Report a bug and Share an idea
open a new GitHub issue in the browser, prefilled with this version, system and architecture, and Message me on X
opens the author's profile. Nothing is sent; the flow records the pages the app would open instead of a browser."""
import platform, sys, time, urllib.parse

from e2e.harness import MACOS, flow, wait, webdriver

MENU = """const m = document.querySelector('[role=menu][aria-label="Send feedback"]');
return m && [...m.querySelectorAll('[role^=menuitem]')].map((i) => i.textContent);"""
CHOOSE = """const i = [...document.querySelectorAll('[role=menu][aria-label="Send feedback"] [role^=menuitem]')]
  .find((i) => i.textContent.startsWith(arguments[0]));
if (!i) return false; i.click(); return true;"""
# A click without a pointer, as Enter or Space on the focused control sends it.
OPEN = "const b = document.querySelector(arguments[0]); b.focus(); b.click();"
IN_MENU = "return !!document.activeElement?.closest('[role=menu][aria-label=\"Send feedback\"]')"
FOCUSED = "return document.activeElement === document.querySelector(arguments[0])"
TOASTS = "return window.__nuzky.store.getState().toasts.map((t) => ({kind: t.kind, text: t.text}));"
ROW, BUTTON = 'nav + div [data-feedback]', 'header [data-feedback]'
ITEMS = ['Report a bug…', 'Share an idea…', 'Message me on X…']


def record_opened(r):
    r.opened = r.work / 'opened.txt'
    r.env['NUZKY_OPENED_URLS'] = str(r.opened)


def opened(r):
    return r.opened.read_text().splitlines() if r.opened.exists() else []


def system():
    """What the app should name this computer as, as a person reading the issue would want it."""
    if MACOS:
        return 'macOS'
    if sys.platform.startswith('linux'):
        try:
            name = platform.freedesktop_os_release()['PRETTY_NAME']
            return f'Linux ({name})'
        except (OSError, KeyError):
            return 'Linux'
    return platform.system()


def choose(r, opener, item):
    """Opens the menu from `opener` and picks `item`; returns the page it opened."""
    before = len(opened(r))
    r.s.run(OPEN, opener)
    if not wait(lambda: r.s.run(MENU), 3) or not r.s.run(CHOOSE, item):
        return None
    wait(lambda: len(opened(r)) > before, 5)
    return opened(r)[before] if len(opened(r)) > before else None


def issue(url):
    parts = urllib.parse.urlsplit(url or '')
    query = urllib.parse.parse_qs(parts.query)
    return f'{parts.scheme}://{parts.netloc}{parts.path}', {k: v[0] for k, v in query.items()}


@flow('feedback', 'Send feedback opens a prefilled GitHub issue for a bug or an idea, or the X profile, with nothing sent',
      before=record_opened, home=True)
def feedback(r):
    version = r.s.run('return window.__nuzky.updates.getState().version')
    footer = f'Nuzky {version} · {system()} · {platform.machine().replace("arm64", "aarch64")}'

    r.check('the foot of the home sidebar offers Send feedback, above the version',
            r.s.run("""const f = document.querySelector(arguments[0]), v = document.querySelector('nav + div p, [data-version]');
                       return !!f && f.textContent.trim() === 'Send feedback' && !!v && f.getBoundingClientRect().bottom <= v.getBoundingClientRect().top;""", ROW))
    r.s.run(OPEN, ROW)
    menu = wait(lambda: r.s.run(MENU), 3)
    r.check('it opens a menu: Report a bug, Share an idea and Message me on X', menu == ITEMS, menu)
    r.check('opened from the keyboard, the menu has the focus', wait(lambda: r.s.run(IN_MENU), 3))
    r.shot('home-menu')
    r.key('Escape')
    r.check('Esc closes the menu and focus returns to the row', wait(lambda: not r.s.run(MENU) and r.s.run(FOCUSED, ROW), 3))
    r.check('opening the menu opened nothing', opened(r) == [], opened(r))

    page, query = issue(choose(r, ROW, 'Report a bug'))
    r.check('Report a bug opens a new issue on the Nuzky repository labelled as a bug',
            page == 'https://github.com/tomasmach/nuzky/issues/new' and query.get('labels') == 'bug', (page, query))
    body = query.get('body', '')
    r.check('the issue asks what happened and ends with this version, system and architecture',
            body.startswith('**What happened?**') and body.rstrip().endswith(footer), (body, footer))

    page, query = issue(choose(r, ROW, 'Share an idea'))
    r.check('Share an idea opens a new issue labelled as an enhancement, with the same version and system',
            page == 'https://github.com/tomasmach/nuzky/issues/new' and query.get('labels') == 'enhancement'
            and query.get('body', '').startswith('**What would you like Nuzky to do?**') and query.get('body', '').rstrip().endswith(footer), (page, query))

    url = choose(r, ROW, 'Message me on X')
    r.check('Message me on X opens the profile on X', url == 'https://x.com/mach_builds', url)
    r.check('each choice opened exactly one page and showed no error',
            len(opened(r)) == 3 and [t for t in r.s.run(TOASTS) if t['kind'] == 'error'] == [], (opened(r), r.s.run(TOASTS)))

    webdriver('POST', r.s.path + '/window/rect', {'width': 1024, 'height': 640})
    if wait(lambda: r.s.run('return window.innerWidth') == 1024, 5):
        r.check('in a 1024 x 640 window Send feedback still sits whole in the sidebar foot',
                r.s.run("""const b = document.querySelector(arguments[0]).getBoundingClientRect();
                           const nav = document.querySelector('nav[aria-label=Collections]').getBoundingClientRect();
                           return b.bottom <= window.innerHeight - 6 && b.top >= nav.bottom && b.width > 180;""", ROW))
        r.shot('home-small')
    webdriver('POST', r.s.path + '/window/rect', {'width': 1440, 'height': 900})

    r.s.run("window.__nuzky.store.setState({view: 'editor'})")
    r.check('in the editor a button next to Projects says Send feedback',
            wait(lambda: r.s.run("return document.querySelector(arguments[0])?.getAttribute('aria-label')", BUTTON), 3) == 'Send feedback')
    r.s.run(OPEN, BUTTON)
    menu = wait(lambda: r.s.run(MENU), 3)
    r.check('it opens the same menu, with the focus in it', menu == ITEMS and wait(lambda: r.s.run(IN_MENU), 3), menu)
    r.shot('editor-menu')
    r.key('Escape')
    r.check('Esc closes it and focus returns to the button', wait(lambda: not r.s.run(MENU) and r.s.run(FOCUSED, BUTTON), 3))
    page, query = issue(choose(r, BUTTON, 'Report a bug'))
    r.check('Report a bug from the editor opens the same prefilled issue',
            page == 'https://github.com/tomasmach/nuzky/issues/new' and query.get('body', '').rstrip().endswith(footer), (page, query))
    time.sleep(0.5)
    r.check('focus goes back to the button after a choice', r.s.run(FOCUSED, BUTTON))
    r.shot('editor')
