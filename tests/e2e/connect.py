"""Connect your agent: one click puts CapOpen into Claude Code's and Codex's own settings, after a
backup and without touching anything else there, and the agent those settings start edits the
project open in the app, live."""
import json, time

from e2e.harness import Bridge, flow, wait

CLAUDE = '{\n  "numStartups": 12,\n  "projects": {\n    "/home/me/site": {\n      "allowedTools": []\n    }\n  },\n  "mcpServers": {\n    "other": {\n      "type": "stdio",\n      "command": "other-server"\n    }\n  }\n}\n'
CODEX = '# my Codex settings\nmodel = "gpt-6.1-sol" # keep this\n\n[mcp_servers.other]\ncommand = "other"\n\n[profiles.fast]\nmodel_reasoning_effort = "low"\n'
ROWS = """return [...document.querySelectorAll('[role=dialog] [role=group]')].map((row) => ({
  text: row.textContent, button: row.querySelector('button')?.textContent}));"""


def home(r):
    """The agents' settings live in a home folder of this run, never the user's own."""
    folder = r.work / 'home'
    (folder / '.codex').mkdir(parents=True)
    (folder / '.claude.json').write_text(CLAUDE)
    (folder / '.codex/config.toml').write_text(CODEX)
    r.env['HOME'] = str(folder)
    for variable in ('CLAUDE_CONFIG_DIR', 'CODEX_HOME'):
        r.env.pop(variable, None)


def click(r, label, scope='document'):
    return r.s.run(f"""const b = [...{scope}.querySelectorAll('button')].find((b) => b.textContent.trim() === arguments[0]);
        if (!b) return false; b.focus(); b.click(); return true;""", label)


@flow('connect', 'Connect agent writes only the capopen entry into Claude Code and Codex settings, after a backup, '
      'and the agent they start edits the open project live', before=home)
def connect(r):
    folder = r.work / 'home'
    r.check('the top bar offers Connect agent', click(r, 'Connect agent'))
    rows = wait(lambda: r.s.run(ROWS) or None, 10)
    r.check('the dialog lists Claude Code and Codex, not yet connected',
            rows and [('Claude Code' in x['text'], 'Not connected' in x['text']) for x in rows][0] == (True, True)
            and 'Codex' in rows[1]['text'] and 'Not connected' in rows[1]['text'], rows)
    time.sleep(0.3)  # the dialog fades in over 200 ms
    r.shot('connect-dialog')
    for name in ('Claude Code', 'Codex'):
        r.s.run("""const row = [...document.querySelectorAll('[role=dialog] [role=group]')]
            .find((row) => row.textContent.includes(arguments[0])); row.querySelector('button').click();""", name)
        r.check(f'{name} shows connected', wait(lambda: any(name in x['text'] and 'Connected' in x['text'] for x in r.s.run(ROWS)), 10),
                r.s.run(ROWS))
    r.shot('connected')

    claude = (folder / '.claude.json').read_text()
    config = json.loads(claude)
    entry = config['mcpServers'].pop('capopen')
    r.check("Claude Code's other settings are untouched", config == json.loads(CLAUDE), config)
    backups = sorted(folder.glob('.claude.json.capopen-backup-*'))
    r.check("Claude Code's settings were backed up first", len(backups) == 1 and backups[0].read_text() == CLAUDE, backups)
    codex = (folder / '.codex/config.toml').read_text()
    block = f'[mcp_servers.capopen]\ncommand = {json.dumps(entry["command"])}\nargs = ["mcp", "--current", "--allow-write"]\n\n'
    r.check("Codex gets the same command and keeps every comment and setting", block in codex and codex.replace(block, '') == CODEX,
            codex)
    backups = sorted((folder / '.codex').glob('config.toml.capopen-backup-*'))
    r.check("Codex's settings were backed up first", len(backups) == 1 and backups[0].read_text() == CODEX, backups)
    r.check('the entry runs this app attached to the open project', entry['args'] == ['mcp', '--current', '--allow-write']
            and entry['command'].endswith('capopen-app'), entry)

    # What Claude Code starts from that entry: an MCP server that edits the project open here.
    agent = Bridge(r, command=[entry['command'], *entry['args']])
    try:
        state = agent.call('get_state', {})
        open_name = r.s.run('return window.__capopen.store.getState().snap.project.name')
        r.check('the agent sees the project open in the app', state['name'] == open_name, [state['name'], open_name])
        run = agent.call('begin_run', {'label': 'Connected agent'})
        r.check("the agent's run shows in the app", wait(lambda: r.state()['aiRun'], 10))
        agent.call('end_run', {'run_id': run['run_id'], 'action': 'discard'})
        r.check('and ends there', wait(lambda: not r.state()['aiRun'], 10))
    finally:
        agent.close()
    r.check('Done closes the dialog', click(r, 'Done') and wait(lambda: not r.s.run("return !!document.querySelector('[role=dialog]')"), 5))
    r.check('no error toast', not r.errors(), r.errors())
