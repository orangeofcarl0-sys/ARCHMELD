// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

'use strict';

/**
 * Runs inside the VS Code extension host. It starts a real archmeld-serve,
 * activates the development extension, opens the fixture repo, executes
 * `archmeld.openGraph` at a precise symbol position, asserts the graph panel
 * exists, and captures a PNG screenshot of the desktop.
 *
 * Environment:
 *   ARCHMELD_UI_STATE       prepared ledger directory
 *   ARCHMELD_UI_PROJECT     project id
 *   ARCHMELD_UI_SNAPSHOT    snapshot id
 *   ARCHMELD_UI_SCREENSHOT  output PNG path
 *   ARCHMELD_SERVE          archmeld-serve binary
 */

const vscode = require('vscode');
const cp = require('child_process');
const fs = require('fs');
const readline = require('readline');

function waitFor(predicate, timeoutMs, label) {
  return new Promise((resolve, reject) => {
    const deadline = Date.now() + timeoutMs;
    const timer = setInterval(() => {
      const value = predicate();
      if (value) {
        clearInterval(timer);
        resolve(value);
      } else if (Date.now() > deadline) {
        clearInterval(timer);
        reject(new Error(`timeout waiting for ${label}`));
      }
    }, 200);
  });
}

async function startServer() {
  const binary = process.env.ARCHMELD_SERVE;
  const state = process.env.ARCHMELD_UI_STATE;
  if (!binary || !fs.existsSync(binary)) {
    throw new Error(`archmeld-serve binary missing: ${binary}`);
  }
  const child = cp.spawn(binary, ['--state', state, '--port', '0', '--no-auth'], {
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  const lines = readline.createInterface({ input: child.stdout });
  const [line] = await new Promise((resolve) => lines.once('line', (value) => resolve([value])));
  lines.close();
  return { child, announcement: JSON.parse(line) };
}

async function cdpScreenshot(port, outPath) {
  const response = await fetch(`http://127.0.0.1:${port}/json/list`);
  const targets = await response.json();
  // The screenshot is workbench evidence (tab strip plus rendered panel), so
  // it always targets the page, not the isolated webview frame target.
  const target =
    targets.find(
      (candidate) =>
        candidate.type === 'page' &&
        /Extension Development Host|扩展开发宿主/.test(candidate.title || ''),
    ) || targets.find((candidate) => candidate.type === 'page');
  if (!target || !target.webSocketDebuggerUrl) {
    throw new Error('no CDP target exposes a webSocketDebuggerUrl');
  }
  if (typeof WebSocket === 'undefined') {
    throw new Error('global WebSocket is unavailable in this extension host');
  }
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.onopen = resolve;
    socket.onerror = () => reject(new Error('CDP websocket failed'));
    setTimeout(() => reject(new Error('CDP websocket timeout')), 10000);
  });
  let nextId = 1;
  const pending = new Map();
  socket.onmessage = (event) => {
    const message = JSON.parse(event.data);
    const entry = pending.get(message.id);
    if (entry) {
      pending.delete(message.id);
      entry.resolve(message.result);
    }
  };
  const call = (method, params) =>
    new Promise((resolve, reject) => {
      const id = nextId++;
      const timer = setTimeout(() => {
        pending.delete(id);
        reject(new Error(`CDP ${method} timeout`));
      }, 15000);
      pending.set(id, {
        resolve: (value) => {
          clearTimeout(timer);
          resolve(value);
        },
      });
      socket.send(JSON.stringify({ id, method, params }));
    });
  // First-run modals (sign-in, customize walkthrough) cover the workbench in a
  // fresh profile. Escape closes the current modal; the label clicks handle
  // localized variants that do not respond to Escape.
  for (let attempt = 0; attempt < 3; attempt += 1) {
    for (const type of ['keyDown', 'keyUp']) {
      await call('Input.dispatchKeyEvent', {
        type,
        key: 'Escape',
        code: 'Escape',
        windowsVirtualKeyCode: 27,
        nativeVirtualKeyCode: 27,
      }).catch(() => undefined);
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  for (let attempt = 0; attempt < 4; attempt += 1) {
    await call('Runtime.evaluate', {
      expression:
        "(() => { const close=document.querySelector('.monaco-dialog-box .codicon-close, [aria-label=\"Close\"], .codicon-close'); if (close) { close.click(); return 'clicked:close'; } return 'none'; })()",
      returnByValue: true,
    }).catch(() => undefined);
    await call('Runtime.evaluate', {
      expression:
        "(() => { const labels=['Continue without Signing In','Get Started','Skip','继续而不登录','稍后再说','以后再说','跳过','关闭','Dismiss']; const nodes=[...document.querySelectorAll('a,button')]; for (const label of labels) { const el=nodes.find(n => n.textContent && n.textContent.trim()===label); if (el) { el.click(); return 'clicked:'+label; } } return 'none'; })()",
      returnByValue: true,
    }).catch(() => undefined);
    await new Promise((resolve) => setTimeout(resolve, 900));
  }
  await new Promise((resolve) => setTimeout(resolve, 1200));
  await call('Performance.enable', {});
  const performance = await call('Performance.getMetrics', {});
  const metrics = {};
  for (const entry of performance?.metrics || []) {
    metrics[entry.name] = entry.value;
  }
  const reply = await call('Page.captureScreenshot', { format: 'png', fromSurface: true });
  socket.close();
  if (!reply || !reply.data) {
    throw new Error('CDP screenshot returned no data');
  }
  fs.writeFileSync(outPath, Buffer.from(reply.data, 'base64'));
  return {
    target: `${target.type || 'page'}:${target.title || target.url || ''}`,
    metrics,
  };
}

async function listTargets(port) {
  const response = await fetch(`http://127.0.0.1:${port}/json/list`);
  return response.json();
}

function connectCdp(webSocketDebuggerUrl) {
  if (typeof WebSocket === 'undefined') {
    return Promise.reject(new Error('global WebSocket is unavailable in this extension host'));
  }
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(webSocketDebuggerUrl);
    let nextId = 1;
    const pending = new Map();
    const listeners = new Map();
    const fail = (error) => {
      for (const entry of pending.values()) {
        entry.reject(error);
      }
      pending.clear();
    };
    socket.onopen = () => {
      const call = (method, params = {}, sessionId) =>
        new Promise((resolveCall, rejectCall) => {
          const id = nextId++;
          const timer = setTimeout(() => {
            pending.delete(id);
            rejectCall(new Error(`CDP ${method} timeout`));
          }, 15000);
          pending.set(id, {
            resolve: (value) => {
              clearTimeout(timer);
              resolveCall(value);
            },
            reject: (error) => {
              clearTimeout(timer);
              rejectCall(error);
            },
          });
          const message = { id, method, params };
          if (sessionId) {
            message.sessionId = sessionId;
          }
          socket.send(JSON.stringify(message));
        });
      const on = (method, handler) => {
        const registered = listeners.get(method) || [];
        registered.push(handler);
        listeners.set(method, registered);
      };
      socket.onmessage = (event) => {
        const message = JSON.parse(event.data);
        if (message.id && pending.has(message.id)) {
          const entry = pending.get(message.id);
          pending.delete(message.id);
          entry.resolve(message.result ?? message);
          return;
        }
        for (const handler of listeners.get(message.method) || []) {
          handler(message.params, message.sessionId);
        }
      };
      socket.onclose = () => fail(new Error('CDP socket closed'));
      resolve({ call, on, close: () => socket.close() });
    };
    socket.onerror = () => reject(new Error('CDP websocket failed'));
    setTimeout(() => reject(new Error('CDP websocket timeout')), 10000);
  });
}

function percentile(values, fraction) {
  if (!values.length) {
    return null;
  }
  const sorted = [...values].sort((left, right) => left - right);
  const index = Math.min(sorted.length - 1, Math.max(0, Math.ceil(fraction * sorted.length) - 1));
  return sorted[index];
}

function waitForContext(records, frameId, sessionId, timeoutMs) {
  return new Promise((resolve) => {
    const deadline = Date.now() + timeoutMs;
    const timer = setInterval(() => {
      const found = records.find(
        (record) =>
          record.context.auxData?.frameId === frameId &&
          record.context.auxData?.isDefault &&
          (sessionId === null || record.sessionId === sessionId),
      );
      if (found || Date.now() > deadline) {
        clearInterval(timer);
        resolve(found || null);
      }
    }, 50);
  });
}

async function evaluateInContext(cdp, contextId, expression, { sessionId = null, awaitPromise = false } = {}) {
  const params = { expression, returnByValue: true, awaitPromise };
  if (contextId !== null && contextId !== undefined) {
    params.contextId = contextId;
  }
  const reply = await cdp.call('Runtime.evaluate', params, sessionId);
  if (reply?.exceptionDetails) {
    throw new Error(`webview evaluation failed: ${reply.exceptionDetails.text || 'exception'}`);
  }
  return reply?.result ? reply.result.value : undefined;
}

const WEBVIEW_PROBE_EXPRESSION = `new Promise((resolve) => {
  function findContentWindow(win, depth) {
    if (!win || depth > 4) { return { win: null, depth: depth }; }
    const doc = win.document;
    if (doc && (doc.getElementById('cy') || doc.querySelector('[data-entity]') || doc.querySelector('.state'))) {
      return { win: win, depth: depth };
    }
    const frames = doc ? Array.from(doc.querySelectorAll('iframe')) : [];
    for (const frame of frames) {
      let child = null;
      try { child = frame.contentWindow; } catch (error) { child = null; }
      if (child) {
        const found = findContentWindow(child, depth + 1);
        if (found.win) { return found; }
      }
    }
    return { win: null, depth: depth };
  }
  const found = findContentWindow(window, 0);
  if (!found.win) {
    resolve({ accessible: false, iframe_count: document.querySelectorAll('iframe').length, wrapper_url: location.href });
    return;
  }
  const win = found.win;
  const doc = win.document;
  const paints = win.performance.getEntriesByType('paint').map((entry) => ({ name: entry.name, start_ms: entry.startTime }));
  const fcp = paints.find((entry) => entry.name === 'first-contentful-paint');
  const lcpEntries = win.performance.getEntriesByType('largest-contentful-paint');
  const navigation = win.performance.getEntriesByType('navigation')[0];
  const started = win.performance.now();
  win.requestAnimationFrame((first) => {
    win.requestAnimationFrame((second) => {
      resolve({
        accessible: true,
        depth: found.depth,
        href: doc.location ? doc.location.href : '',
        ready_state: doc.readyState,
        time_origin_ms: win.performance.timeOrigin,
        paint_entries: paints,
        first_contentful_paint_ms: fcp ? fcp.start_ms : null,
        largest_contentful_paint_ms: lcpEntries.length ? lcpEntries[lcpEntries.length - 1].startTime : null,
        navigation: navigation ? {
          response_end_ms: navigation.responseEnd,
          dom_content_loaded_ms: navigation.domContentLoadedEventEnd,
          load_event_ms: navigation.loadEventEnd
        } : null,
        node_count: doc.querySelectorAll('[data-entity]').length,
        edge_count: doc.querySelectorAll('[data-edge]').length,
        cytoscape_container: Boolean(doc.getElementById('cy')),
        header_text: doc.querySelector('header strong') ? doc.querySelector('header strong').textContent : null,
        double_raf: {
          start_ms: started,
          first_raf_ms: first,
          second_raf_ms: second,
          double_raf_ms: second - first,
          elapsed_ms: win.performance.now() - started
        }
      });
    });
  });
})`;

const WEBVIEW_REPAINT_EXPRESSION = `(() => {
  function findContentWindow(win, depth) {
    if (!win || depth > 4) { return null; }
    const doc = win.document;
    if (doc && (doc.getElementById('cy') || doc.querySelector('[data-entity]') || doc.querySelector('.state'))) {
      return win;
    }
    for (const frame of Array.from(doc ? doc.querySelectorAll('iframe') : [])) {
      let child = null;
      try { child = frame.contentWindow; } catch (error) { child = null; }
      if (child) {
        const found = findContentWindow(child, depth + 1);
        if (found) { return found; }
      }
    }
    return null;
  }
  const win = findContentWindow(window, 0);
  if (!win) { return null; }
  const doc = win.document;
  doc.body.style.opacity = '0.999';
  void doc.body.offsetHeight;
  win.requestAnimationFrame(() => { doc.body.style.opacity = '1'; });
  return win.performance.now();
})()`;

function screencastSummary(timestamps, triggerEpochMs, scope) {
  const intervals = [];
  for (let index = 1; index < timestamps.length; index += 1) {
    intervals.push(Math.round((timestamps[index] - timestamps[index - 1]) * 1000) / 1000);
  }
  // Chromium reports screencast timestamps either as epoch seconds or as
  // monotonic seconds; only compare with the wall clock when the scale fits.
  const first = timestamps.length ? timestamps[0] : null;
  const epochScale = first !== null && Math.abs(first - triggerEpochMs) < 600000;
  const afterTrigger = epochScale ? timestamps.find((value) => value >= triggerEpochMs) : null;
  return {
    frame_count: timestamps.length,
    first_frame_after_trigger_ms: afterTrigger
      ? Math.round((afterTrigger - triggerEpochMs) * 1000) / 1000
      : null,
    timestamp_scale: epochScale ? 'epoch_ms' : 'monotonic_or_unknown',
    intervals_ms: intervals,
    interval_p95_ms: percentile(intervals, 0.95),
    scope: scope,
  };
}

async function measureWebview(port) {
  const targets = await listTargets(port);
  const targetSummary = targets.map((target) => ({
    type: target.type || '',
    title: (target.title || '').slice(0, 120),
    url: (target.url || '').slice(0, 160),
  }));
  // VS Code renders a webview as an out-of-process `iframe` target whose URL
  // uses the vscode-webview:// scheme; the extension document is nested
  // inside it.
  const webview = targets.find(
    (target) => (target.url || '').startsWith('vscode-webview://') && target.webSocketDebuggerUrl,
  );
  if (!webview) {
    return {
      available: false,
      limitations: ['no vscode-webview:// target with a webSocketDebuggerUrl in /json/list'],
      targets: targetSummary,
    };
  }
  const limitations = [];
  const webviewCdp = await connectCdp(webview.webSocketDebuggerUrl);
  try {
    await webviewCdp.call('Runtime.enable');
    await webviewCdp.call('Page.enable');
    const frames = [];
    try {
      const frameTree = await webviewCdp.call('Page.getFrameTree');
      const walk = (frame) => {
        frames.push({ id: frame.id, url: (frame.url || '').slice(0, 160) });
        for (const child of frame.childFrames || []) {
          walk(child);
        }
      };
      walk(frameTree.frameTree);
    } catch (error) {
      limitations.push(`Page.getFrameTree failed: ${error.message}`);
    }
    const probe = await evaluateInContext(webviewCdp, null, WEBVIEW_PROBE_EXPRESSION, {
      awaitPromise: true,
    });
    if (!probe || !probe.accessible) {
      limitations.push('webview shell did not expose the extension document (cross-origin nesting)');
    }
    // Compositor frame cadence comes from a workbench page screencast while
    // only the webview document mutates; the webview target itself does not
    // emit screencast frames in Electron.
    const page = targets.find(
      (target) =>
        target.type === 'page' &&
        target.webSocketDebuggerUrl &&
        /Extension Development Host|扩展开发宿主/.test(target.title || ''),
    ) || targets.find((target) => target.type === 'page' && target.webSocketDebuggerUrl);
    let screencast = null;
    if (!page) {
      limitations.push('no workbench page target for screencast');
    } else {
      const pageCdp = await connectCdp(page.webSocketDebuggerUrl);
      const framesSeen = [];
      try {
        await pageCdp.call('Page.enable');
        pageCdp.on('Page.screencastFrame', (params) => {
          framesSeen.push((params.metadata?.timestamp || 0) * 1000);
          pageCdp.call('Page.screencastFrameAck', { sessionId: params.sessionId }).catch(() => undefined);
        });
        const triggerEpochMs = Date.now();
        await pageCdp.call('Page.startScreencast', {
          format: 'png',
          maxWidth: 1280,
          maxHeight: 800,
          everyNthFrame: 1,
        });
        await evaluateInContext(webviewCdp, null, WEBVIEW_REPAINT_EXPRESSION);
        await new Promise((resolve) => setTimeout(resolve, 1200));
        await pageCdp.call('Page.stopScreencast', {}).catch(() => undefined);
        screencast = screencastSummary(
          framesSeen.filter((value) => value > 0),
          triggerEpochMs,
          'workbench page screencast while only the webview document mutated (includes workbench compositing)',
        );
        if (screencast.frame_count < 2) {
          limitations.push('page screencast produced fewer than two frames; compositor cadence unavailable');
        }
      } catch (error) {
        limitations.push(`page screencast unavailable: ${error.message}`);
        screencast = { frame_count: framesSeen.length, error: error.message };
      } finally {
        pageCdp.close();
      }
    }
    return {
      available: Boolean(probe && probe.accessible),
      frame_url: webview.url,
      frames: frames,
      probe: probe,
      screencast: screencast,
      targets: targetSummary,
      limitations: limitations,
    };
  } finally {
    webviewCdp.close();
  }
}

async function run() {
  let server;
  try {
    process.env.ARCHMELD_PAINT_PROBE = '1';
    const started = await startServer();
    server = started.child;
    process.env.ARCHMELD_ENDPOINT = started.announcement.endpoint;
    process.env.ARCHMELD_PROJECT = process.env.ARCHMELD_UI_PROJECT;
    process.env.ARCHMELD_SNAPSHOT = process.env.ARCHMELD_UI_SNAPSHOT;

    const extension = vscode.extensions.getExtension('archmeld.archmeld-workbench');
    if (!extension) {
      throw new Error('development extension not found');
    }
    await extension.activate();
    const commands = await vscode.commands.getCommands(true);
    for (const id of [
      'archmeld.openGraph',
      'archmeld.openSource',
      'archmeld.back',
      'archmeld.openTraceFrame',
    ]) {
      if (!commands.includes(id)) {
        throw new Error(`command not registered: ${id}`);
      }
    }

    const traceRun = process.env.ARCHMELD_UI_TRACE_RUN;
    if (traceRun) {
      const traceStartedAt = Date.now();
      const traceResult = await vscode.commands.executeCommand('archmeld.openTraceFrame', {
        runId: traceRun,
        spanIndex: 0,
      });
      const traceCommandMs = Date.now() - traceStartedAt;
      const editor = vscode.window.activeTextEditor;
      if (!editor) {
        throw new Error('no active editor after opening the trace frame');
      }
      const documentText = editor.document.getText();
      if (documentText !== traceResult.document_text) {
        throw new Error('the editor text differs from the source-file excerpt');
      }
      const expectedLine = traceResult.line - traceResult.line_start;
      if (editor.selection.active.line !== expectedLine) {
        throw new Error(
          `the cursor is on line ${editor.selection.active.line}, expected ${expectedLine}`,
        );
      }
      const screenshot = process.env.ARCHMELD_UI_SCREENSHOT;
      let captureTarget = '';
      let captureMethod = 'none';
      if (screenshot) {
        try {
          const captured = await cdpScreenshot(process.env.ARCHMELD_UI_CDP_PORT, screenshot);
          captureTarget = captured.target;
          captureMethod = 'cdp';
        } catch (error) {
          captureTarget = `unavailable: ${error.message}`;
          captureMethod = 'failed';
        }
      }
      const budgetFailures = [];
      if (traceCommandMs > 10000) {
        budgetFailures.push(`openTraceFrame ${traceCommandMs}ms > 10000ms`);
      }
      const result = {
        status: budgetFailures.length ? 'budget_failed' : 'passed',
        mode: 'trace_frame',
        snapshot_id: process.env.ARCHMELD_UI_SNAPSHOT,
        project_id: process.env.ARCHMELD_UI_PROJECT,
        endpoint: started.announcement.endpoint,
        run_id: traceResult.run_id,
        span_id: traceResult.span_id,
        path: traceResult.path,
        line: traceResult.line,
        line_start: traceResult.line_start,
        line_end: traceResult.line_end,
        active_document: editor.document.uri.toString(),
        active_line: editor.selection.active.line,
        expected_line: expectedLine,
        trace_command_ms: traceCommandMs,
        screenshot,
        capture_method: captureMethod,
        capture_target: captureTarget,
        captured_at: new Date().toISOString(),
        budgets: { trace_command_ms: 10000 },
        budget_failures: budgetFailures,
      };
      fs.writeFileSync(process.env.ARCHMELD_UI_RESULT, JSON.stringify(result, null, 2));
      if (budgetFailures.length) {
        throw new Error(`UI trace budgets failed: ${budgetFailures.join('; ')}`);
      }
      console.log(`UI TRACE PASS run=${traceRun} path=${traceResult.path}:${traceResult.line}`);
      return;
    }

    const folder = vscode.workspace.workspaceFolders?.[0];
    if (!folder) {
      throw new Error('fixture workspace folder was not opened');
    }
    const document = await vscode.workspace.openTextDocument(vscode.Uri.joinPath(folder.uri, 'src/app.py'));
    const editor = await vscode.window.showTextDocument(document);
    // Cursor inside `main` on line 0; the extension must find the exact anchor.
    const position = new vscode.Position(0, 5);
    editor.selection = new vscode.Selection(position, position);
    const commandStartedAt = Date.now();
    await vscode.commands.executeCommand('archmeld.openGraph');
    const commandReturnedMs = Date.now() - commandStartedAt;

    const tab = await waitFor(
      () =>
        vscode.window.tabGroups.all
          .flatMap((group) => group.tabs)
          .find((candidate) => candidate.label === 'ARCHMELD Graph'),
      20000,
      'ARCHMELD Graph panel',
    );
    if (!tab) {
      throw new Error('graph panel did not open');
    }
    const panelReadyMs = Date.now() - commandStartedAt;
    const activeBefore = vscode.window.tabGroups.activeTabGroup.activeTab?.label || '';
    if (/welcome/i.test(activeBefore)) {
      const available = await vscode.commands.getCommands(true);
      if (available.includes('workbench.action.closeActiveEditor')) {
        await vscode.commands.executeCommand('workbench.action.closeActiveEditor');
      }
    }
    const groups = vscode.window.tabGroups.all;
    const groupIndex = groups.findIndex((group) => group.tabs.includes(tab));
    const focusCommand = ['workbench.action.focusFirstEditorGroup', 'workbench.action.focusSecondEditorGroup', 'workbench.action.focusThirdEditorGroup'][groupIndex];
    if (focusCommand && (await vscode.commands.getCommands(true)).includes(focusCommand)) {
      await vscode.commands.executeCommand(focusCommand);
    }
    await new Promise((resolve) => setTimeout(resolve, 3000));

    const screenshot = process.env.ARCHMELD_UI_SCREENSHOT;
    if (!screenshot) {
      throw new Error('ARCHMELD_UI_SCREENSHOT is not set');
    }
    let captureMethod = 'cdp';
    let captureTarget = '';
    let captureTargetMetrics = null;
    const cdpPort = process.env.ARCHMELD_UI_CDP_PORT;
    try {
      if (!cdpPort) {
        throw new Error('ARCHMELD_UI_CDP_PORT is not set');
      }
      const captured = await cdpScreenshot(cdpPort, screenshot);
      captureTarget = captured.target;
      captureTargetMetrics = captured.metrics;
    } catch (cdpError) {
      captureMethod = 'desktop-fallback';
      const captureScript = require('node:path').join(__dirname, 'capture.ps1');
      cp.execFileSync(
        'powershell',
        ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', captureScript, '-Out', screenshot, '-ExtensionHostPid', String(process.pid), '-Marker', process.env.ARCHMELD_UI_MARKER || ''],
        { stdio: 'inherit' },
      );
      captureTarget = `fallback: ${cdpError.message}`;
    }
    if (!fs.existsSync(screenshot) || fs.statSync(screenshot).size < 10000) {
      throw new Error(`screenshot was not written or is empty: ${screenshot}`);
    }
    // Dedicated webview-scoped measurement: paint timeline read inside the
    // actual vscode-webview:// document, a forced double-rAF frame interval,
    // and compositor screencast frames if the platform exposes them.
    let webviewMeasurement = null;
    if (cdpPort) {
      try {
        webviewMeasurement = await measureWebview(cdpPort);
      } catch (error) {
        webviewMeasurement = {
          available: false,
          limitations: [`webview measurement failed: ${error.message}`],
        };
      }
    } else {
      webviewMeasurement = { available: false, limitations: ['ARCHMELD_UI_CDP_PORT is not set'] };
    }

    const availableCommands = await vscode.commands.getCommands(true);
    const paintProbeEnabled = availableCommands.includes('archmeld.diagnostics.paintReport');
    let paintDiagnostics = { paint_reports: [], render_reports: [], pending_probes: 0 };
    if (paintProbeEnabled) {
      const deadline = Date.now() + 15000;
      while (Date.now() < deadline) {
        paintDiagnostics = await vscode.commands.executeCommand('archmeld.diagnostics.paintReport');
        const reports = paintDiagnostics.paint_reports || [];
        const hasLoad = reports.some((report) => report.phase === 'load' || report.phase === 'settled');
        if (hasLoad && (paintDiagnostics.render_reports || []).length > 0) {
          break;
        }
        await new Promise((resolve) => setTimeout(resolve, 250));
      }
    }
    const paintProbes = [];
    if (paintProbeEnabled && availableCommands.includes('archmeld.diagnostics.paintProbe')) {
      for (let attempt = 0; attempt < 3; attempt += 1) {
        try {
          paintProbes.push(await vscode.commands.executeCommand('archmeld.diagnostics.paintProbe'));
        } catch (error) {
          paintProbes.push({ error: error.message });
        }
        await new Promise((resolve) => setTimeout(resolve, 150));
      }
    }

    const paintReports = paintDiagnostics.paint_reports || [];
    const renderReports = paintDiagnostics.render_reports || [];
    const loadReport =
      paintReports.find((report) => report.phase === 'load') ||
      paintReports[paintReports.length - 1] ||
      null;
    // Paint entries only exist after the first presentation; the settled
    // report (one second after load) carries first-paint/FCP/LCP.
    const settledReport = paintReports.find((report) => report.phase === 'settled') || null;
    const probeRoundtrips = paintProbes
      .map((probe) => probe.roundtrip_ms)
      .filter((value) => typeof value === 'number');
    const probeRaf = paintProbes
      .map((probe) => probe.report?.double_raf_ms)
      .filter((value) => typeof value === 'number');
    const probeElapsed = paintProbes
      .map((probe) => probe.report?.protocol_elapsed_ms)
      .filter((value) => typeof value === 'number');
    const firstContentfulPaint =
      webviewMeasurement?.probe?.first_contentful_paint_ms ??
      settledReport?.first_contentful_paint_ms ??
      loadReport?.first_contentful_paint_ms ??
      null;
    const frameIntervalP95 = webviewMeasurement?.screencast?.interval_p95_ms ?? null;
    const firstFrameAfterTrigger = webviewMeasurement?.screencast?.first_frame_after_trigger_ms ?? null;

    const timings = {
      open_graph_command_ms: commandReturnedMs,
      panel_ready_ms: panelReadyMs,
      settle_ms: 3000,
      webview_paint: {
        source: 'graph webview postMessage paintReport/renderReport',
        first_contentful_paint_ms: firstContentfulPaint,
        largest_contentful_paint_ms:
          webviewMeasurement?.probe?.largest_contentful_paint_ms ??
          settledReport?.largest_contentful_paint_ms ??
          null,
        double_raf_ms: loadReport?.double_raf_ms ?? null,
        webview_probe_double_raf_ms: webviewMeasurement?.probe?.double_raf?.double_raf_ms ?? null,
        time_origin_ms: loadReport?.time_origin_ms ?? null,
        load_report: loadReport,
        settled_report: settledReport,
        render_reports: renderReports,
        probe_roundtrips_ms: probeRoundtrips,
        probe_double_raf_ms: probeRaf,
        probe_protocol_elapsed_ms: probeElapsed,
      },
      webview_frame: webviewMeasurement?.screencast ?? null,
      webview_frame_scope:
        'requestAnimationFrame double-frame interval and compositor screencast presentation timestamps; not GPU execution time',
      capture_target_metrics: captureTargetMetrics,
      metrics_scope:
        'CDP page capture metrics; webview paint/frame values come from the webview document and frame observations recorded separately',
    };
    const budgetFailures = [];
    if (timings.open_graph_command_ms > 10000) {
      budgetFailures.push(`openGraph command ${timings.open_graph_command_ms}ms > 10000ms`);
    }
    if (timings.panel_ready_ms > 20000) {
      budgetFailures.push(`panel ready ${timings.panel_ready_ms}ms > 20000ms`);
    }
    if (captureTargetMetrics && captureTargetMetrics.TaskDuration > 30) {
      budgetFailures.push(`capture target TaskDuration ${captureTargetMetrics.TaskDuration}s > 30s`);
    }
    if (!paintProbeEnabled) {
      budgetFailures.push('paint diagnostics commands were not registered (ARCHMELD_PAINT_PROBE)');
    }
    if (!paintReports.length) {
      budgetFailures.push('no paintReport was received from the graph webview');
    }
    if (!renderReports.length) {
      budgetFailures.push('no renderReport was received from the graph webview');
    }
    if (!webviewMeasurement || !webviewMeasurement.available) {
      budgetFailures.push(
        `dedicated webview document was not measurable: ${(webviewMeasurement?.limitations || ['unknown']).join('; ')}`,
      );
    }
    if (typeof firstContentfulPaint !== 'number') {
      budgetFailures.push('no first-contentful-paint sample inside the webview document');
    } else if (firstContentfulPaint > 5000) {
      budgetFailures.push(`webview first contentful paint ${firstContentfulPaint}ms > 5000ms`);
    }
    for (const roundtrip of probeRoundtrips) {
      if (roundtrip > 2000) {
        budgetFailures.push(`webview paint probe roundtrip ${Math.round(roundtrip)}ms > 2000ms`);
      }
    }
    for (const raf of probeRaf) {
      if (raf > 1000) {
        budgetFailures.push(`webview double-rAF frame ${Math.round(raf)}ms > 1000ms`);
      }
    }
    const cdpRaf = webviewMeasurement?.probe?.double_raf?.double_raf_ms;
    if (typeof cdpRaf === 'number' && cdpRaf > 1000) {
      budgetFailures.push(`webview CDP double-rAF frame ${Math.round(cdpRaf)}ms > 1000ms`);
    }
    if (typeof firstFrameAfterTrigger !== 'number') {
      budgetFailures.push('no compositor frame was observed after the webview repaint trigger');
    } else if (firstFrameAfterTrigger > 500) {
      budgetFailures.push(
        `webview first compositor frame after trigger ${firstFrameAfterTrigger}ms > 500ms`,
      );
    }
    const result = {
      status: budgetFailures.length ? 'budget_failed' : 'passed',
      snapshot_id: process.env.ARCHMELD_UI_SNAPSHOT,
      project_id: process.env.ARCHMELD_UI_PROJECT,
      endpoint: started.announcement.endpoint,
      opened_tab: tab.label,
      active_tab_before_capture: vscode.window.tabGroups.activeTabGroup.activeTab?.label || '',
      group_index: groupIndex,
      commands: ['archmeld.openGraph', 'archmeld.openSource', 'archmeld.back'],
      screenshot,
      capture_method: captureMethod,
      capture_target: captureTarget,
      captured_at: new Date().toISOString(),
      activator: 'code --extensionDevelopmentPath --extensionTestsPath --wait',
      timings,
      paint_reports: paintReports,
      paint_probes: paintProbes,
      webview_measurement: webviewMeasurement,
      budgets: {
        open_graph_command_ms: 10000,
        panel_ready_ms: 20000,
        capture_target_task_duration_s: 30,
        webview_first_contentful_paint_ms: 5000,
        webview_probe_roundtrip_ms: 2000,
        webview_double_raf_ms: 1000,
        webview_first_frame_after_trigger_ms: 500,
      },
      budget_failures: budgetFailures,
    };
    // Raw measurements are written before a budget failure is raised.
    fs.writeFileSync(process.env.ARCHMELD_UI_RESULT, JSON.stringify(result, null, 2));
    if (budgetFailures.length) {
      throw new Error(`UI budgets failed: ${budgetFailures.join('; ')}`);
    }
    console.log(`UI TEST PASS snapshot=${process.env.ARCHMELD_UI_SNAPSHOT}`);
  } finally {
    server?.kill();
  }
}

module.exports = { run };
