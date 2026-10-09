import subprocess, time

from e2e.harness import AI_EDITING, CLI, FIXTURES, Bridge, flow, wait


def agent_project(r):
    project = r.work / 'data/nuzky/projects/agent.nuzky'
    subprocess.run([str(CLI), 'new', str(project), str(FIXTURES / 'talk.mp4')], env=r.env, check=True, capture_output=True)


@flow('agent', 'An AI agent edits the open project live: the UI locks with a reason, the edit appears, one undo removes the run',
      before=agent_project)
def agent(r):
    clip = r.track()[0]
    r.check('the app opened the project the agent will edit', len(r.track()) == 1)
    bridge = Bridge(r, r.saved_project())
    try:
        run = bridge.call('begin_run', {'label': 'Tighten the intro'})
        r.check('the UI knows an agent is editing', wait(lambda: r.state()['aiRun'], 10))
        button = r.s.run("""const b = [...document.querySelectorAll('button')].find((b) => b.textContent.includes('Import media'));
            return b && {disabled: b.getAttribute('aria-disabled'), reason: b.title};""")
        r.check('controls are locked and say why', button and button['disabled'] == 'true' and button['reason'], button)
        r.focus_clip(clip['id'])
        for _ in range(2):
            r.key('Delete')
            time.sleep(0.5)
        notices = [t for t in r.state()['toasts'] if t['text'] == AI_EDITING]
        r.check('a user edit during the run is refused with one notice', len(r.track()) == 1 and len(notices) == 1, r.state()['toasts'])
        r.shot('locked')
        edit = {'type': 'splitClip', 'clipId': clip['id'], 'atUs': 3_000_000}
        bridge.call('apply_edits', {'run_id': run['run_id'], 'request_id': 'split', 'edits': [edit]})
        r.check("the agent's edit appears live", wait(lambda: len(r.track()) == 2, 10), r.track())
        bridge.call('end_run', {'run_id': run['run_id'], 'action': 'keep'})
        r.check('the lock ends with the run', wait(lambda: not r.state()['aiRun'], 10))
        r.shot('after-run')
        r.s.run('document.activeElement?.blur()')
        r.key('z', ctrlKey=True)
        r.check('one undo removes the whole run', wait(lambda: len(r.track()) == 1, 10), r.track())
        r.check('no error toast', not r.errors(), r.errors())
    finally:
        bridge.close()
