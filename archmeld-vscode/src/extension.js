// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

'use strict';

const vscode = require('vscode');
const fs = require('node:fs');
const crypto = require('node:crypto');
const { ArchmeldClient } = require('./api');
const { GraphModel } = require('./model');
const { renderHtml } = require('./webview');
const { renderReviewHtml } = require('./review');
const { renderChangesetHtml } = require('./changeset');
const { renderTargetEditorHtml } = require('./target');

const SOURCE_SCHEME = 'archmeld-snapshot';
const TRACE_SOURCE_SCHEME = 'archmeld-snapshot-file';

/** @type {import('vscode').WebviewPanel | undefined} */
let panel;
/** @type {import('vscode').WebviewPanel | undefined} */
let reviewPanel;
/** @type {import('vscode').WebviewPanel | undefined} */
let changesetPanel;
let targetPanel;
const model = new GraphModel();
/** @type {Map<string, string>} */
const sourceCache = new Map();
/** @type {Map<string, string>} */
const traceSourceCache = new Map();
let status;
let client;
const webviewAssets = { cytoscapeUri: undefined, cspSource: '' };
// Paint/render observations reported by the graph webview itself. They are
// only exposed through the ARCHMELD_PAINT_PROBE diagnostics commands used by
// the extension-host test harness; the product surface does not read them.
const paintDiagnostics = { paintReports: [], renderReports: [], waiters: [], sequence: 0 };

function readSettings() {
  const config = vscode.workspace.getConfiguration('archmeld');
  const endpoint = process.env.ARCHMELD_ENDPOINT || config.get('endpoint');
  const token = process.env.ARCHMELD_TOKEN || config.get('token') || '';
  const projectId = process.env.ARCHMELD_PROJECT || config.get('projectId');
  const snapshotId = process.env.ARCHMELD_SNAPSHOT || config.get('snapshotId');
  const workspaceHandle = process.env.ARCHMELD_WORKSPACE || config.get('workspaceHandle') || '';
  if (!projectId || !snapshotId) {
    throw new Error('Set archmeld.projectId and archmeld.snapshotId (or ARCHMELD_PROJECT/ARCHMELD_SNAPSHOT).');
  }
  return { endpoint, token, projectId, snapshotId, workspaceHandle };
}

function getClient(settings) {
  if (!client) {
    client = new ArchmeldClient({ endpoint: settings.endpoint, token: settings.token });
  }
  return client;
}

function refresh(stale = false) {
  if (!panel) {
    return;
  }
  panel.webview.html = renderHtml({
    graph: model,
    title: `ARCHMELD · ${model.view}`,
    stale,
    cytoscapeUri: webviewAssets.cytoscapeUri,
    cspSource: webviewAssets.cspSource,
    paintProbe: process.env.ARCHMELD_PAINT_PROBE === '1',
  });
}

async function ensurePanel(context) {
  if (panel) {
    return panel;
  }
  const media = vscode.Uri.joinPath(context.extensionUri, 'media');
  panel = vscode.window.createWebviewPanel(
    'archmeld.graph',
    'ARCHMELD Graph',
    vscode.ViewColumn.Beside,
    { enableScripts: true, retainContextWhenHidden: true, localResourceRoots: [media] },
  );
  panel.onDidDispose(() => {
    panel = undefined;
    webviewAssets.cytoscapeUri = undefined;
    webviewAssets.cspSource = '';
  });
  const cytoscape = vscode.Uri.joinPath(media, 'cytoscape.min.js');
  webviewAssets.cytoscapeUri = fs.existsSync(cytoscape.fsPath)
    ? panel.webview.asWebviewUri(cytoscape).toString()
    : undefined;
  webviewAssets.cspSource = panel.webview.cspSource;
  panel.webview.onDidReceiveMessage(async (message) => {
    if (message.type === 'paintReport') {
      if (process.env.ARCHMELD_PAINT_PROBE === '1') {
        paintDiagnostics.paintReports.push({ received_epoch_ms: Date.now(), ...message.report });
        if (paintDiagnostics.paintReports.length > 100) {
          paintDiagnostics.paintReports.splice(0, paintDiagnostics.paintReports.length - 100);
        }
      }
      return;
    }
    if (message.type === 'renderReport') {
      if (process.env.ARCHMELD_PAINT_PROBE === '1') {
        paintDiagnostics.renderReports.push({ received_epoch_ms: Date.now(), ...message.report });
        if (paintDiagnostics.renderReports.length > 100) {
          paintDiagnostics.renderReports.splice(0, paintDiagnostics.renderReports.length - 100);
        }
      }
      return;
    }
    if (message.type === 'paintProbeResult') {
      const index = paintDiagnostics.waiters.findIndex((waiter) => waiter.probeId === message.probeId);
      if (index >= 0) {
        const [waiter] = paintDiagnostics.waiters.splice(index, 1);
        waiter.resolve(message.report);
      }
      return;
    }
    try {
      const settings = readSettings();
      if (message.type === 'setView') {
        model.setView(message.view);
        refresh();
      } else if (message.type === 'selectEdge') {
        const evidence = model.evidenceFor(message.edgeId);
        const relation = model.edges.get(message.edgeId);
        if (!relation) {
          return;
        }
        const style = model
          .visibleEdges([relation.source_id, relation.target_id])
          .find((edge) => edge.id === relation.id)?.style;
        const items = evidence.length
          ? evidence.map((item) => ({
              label: item.id,
              description: `${item.kind} · ${item.producer?.name || 'unknown'} ${item.producer?.version || ''}`,
              detail: item.locator,
            }))
          : [{ label: 'No evidence attached', description: 'unknown edge — not a resolved fact' }];
        await vscode.window.showQuickPick(items, {
          title: `${relation.kind} · ${style ? style.label : relation.basis}`,
          placeHolder: 'Evidence for this relation (basis and resolution are shown, not inferred certainty)',
        });
      } else if (message.type === 'selectEntity') {
        const changed = model.pushSelection({
          projectId: settings.projectId,
          snapshotId: settings.snapshotId,
          entityId: message.entityId,
          originView: model.view,
        });
        if (changed) {
          await openHistoricalSource(settings, message.entityId, 0);
        }
      }
    } catch (error) {
      vscode.window.showErrorMessage(`ARCHMELD: ${error.message}`);
    }
  });
  context.subscriptions.push(panel);
  return panel;
}

async function exportReport(settings) {
  if (!settings.projectId || !settings.snapshotId) {
    throw new Error('Set archmeld.projectId and archmeld.snapshotId before exporting a report.');
  }
  const outputName = await vscode.window.showInputBox({
    title: 'ARCHMELD report output name',
    prompt: 'Safe directory name under the server-owned exports directory',
    value: `report-${Date.now()}`,
    validateInput: (value) =>
      /^[A-Za-z0-9._-]+$/.test(value) ? null : 'Use letters, digits, dot, dash or underscore',
  });
  if (!outputName) {
    return null;
  }
  const sourceLabel = 'Include source bytes (sensitive)';
  const traceLabel = 'Include raw traces (sensitive)';
  const picks = await vscode.window.showQuickPick(
    [
      {
        label: sourceLabel,
        description: 'Source may contain credentials, personal data or proprietary code; no secret scanner runs',
      },
      {
        label: traceLabel,
        description: 'Traces may contain process names, paths and identifiers; no redaction runs',
      },
    ],
    {
      canPickMany: true,
      title: 'Optional bundle contents (default excludes both)',
      placeHolder: 'Leave both unselected for a metadata-only report',
    },
  );
  const includeSource = !!picks && picks.some((item) => item.label === sourceLabel);
  const includeRawTraces = !!picks && picks.some((item) => item.label === traceLabel);
  if (includeSource || includeRawTraces) {
    const confirmed = await vscode.window.showWarningMessage(
      'This report bundle will contain opted-in source or trace bytes and must be treated as sensitive.',
      { modal: true },
      'Export sensitive bundle',
    );
    if (confirmed !== 'Export sensitive bundle') {
      return null;
    }
  }
  const result = await getClient(settings).report(
    settings.projectId,
    settings.snapshotId,
    outputName,
    includeSource,
    includeRawTraces,
  );
  const action = await vscode.window.showInformationMessage(
    `ARCHMELD report ${result.report_id} (risk: ${result.risk_level})`,
    'Open report',
    'Copy manifest digest',
  );
  if (action === 'Open report') {
    await vscode.env.openExternal(
      vscode.Uri.joinPath(vscode.Uri.file(result.output_dir), 'report.html'),
    );
  } else if (action === 'Copy manifest digest' && result.bundle_digest) {
    await vscode.env.clipboard.writeText(result.bundle_digest);
  }
  return result;
}

async function openHistoricalSource(settings, entityId, anchorIndex) {
  const response = await getClient(settings).source({
    project_id: settings.projectId,
    snapshot_id: settings.snapshotId,
    entity_id: entityId,
    anchor_index: anchorIndex,
    max_bytes: 262144,
  });
  const path = `/${settings.snapshotId}/${encodeURIComponent(entityId)}/${anchorIndex}`;
  sourceCache.set(path, response.text);
  const uri = vscode.Uri.from({ scheme: SOURCE_SCHEME, path });
  const document = await vscode.workspace.openTextDocument(uri);
  await vscode.window.showTextDocument(document, { preview: true, viewColumn: vscode.ViewColumn.One });
  vscode.window.setStatusBarMessage(
    `ARCHMELD: historical source from ${settings.snapshotId.slice(0, 19)}… (not the current file)`,
    5000,
  );
}

function traceSourceLocation(span) {
  const attributes = span && span.attributes ? span.attributes : {};
  const file = attributes['tracy.src_file'];
  const line = Number(attributes['tracy.src_line']);
  if (typeof file !== 'string' || !Number.isInteger(line) || line < 1) {
    return null;
  }
  return { file, line };
}

async function openTraceFrame(settings, argument) {
  const runId = typeof argument === 'string' ? argument : argument?.runId;
  if (!runId) {
    throw new Error('Pass a runtime run id: archmeld.openTraceFrame {runId}');
  }
  const client = getClient(settings);
  const timeline = await client.runtime(settings.projectId, runId, 1000);
  const located = (timeline.spans || [])
    .map((span) => ({ span, location: traceSourceLocation(span) }))
    .filter((item) => item.location);
  if (located.length === 0) {
    throw new Error('this run has no spans with retained Tracy source locations');
  }
  let selected = null;
  if (argument?.spanId) {
    selected = located.find((item) => item.span.span_id === argument.spanId) || null;
    if (!selected) {
      throw new Error(`span ${argument.spanId} has no retained source location`);
    }
  } else if (Number.isInteger(argument?.spanIndex)) {
    selected = located[argument.spanIndex];
    if (!selected) {
      throw new Error(`span index ${argument.spanIndex} is out of range`);
    }
  } else {
    const pick = await vscode.window.showQuickPick(
      located.map((item) => ({
        label: item.span.name,
        description: `${item.location.file}:${item.location.line}`,
        detail: `${item.span.kind} · ${item.span.start_ns}ns · ${
          item.span.attributes?.['archmeld.entity_id'] || 'unmapped entity'
        }`,
        spanId: item.span.span_id,
      })),
      {
        title: 'Trace zones with retained source locations',
        placeHolder: 'Open the historical snapshot line for this zone',
      },
    );
    if (!pick) {
      return undefined;
    }
    selected = located.find((item) => item.span.span_id === pick.spanId);
  }
  const { file, line } = selected.location;
  const excerpt = await client.sourceFile({
    project_id: settings.projectId,
    snapshot_id: settings.snapshotId,
    path: file,
    line,
    context_lines: 4,
    max_bytes: 262144,
  });
  const uri = vscode.Uri.from({
    scheme: TRACE_SOURCE_SCHEME,
    path: `/${settings.snapshotId}/${encodeURIComponent(file)}/${line}`,
  });
  traceSourceCache.set(uri.path, excerpt.text);
  const document = await vscode.workspace.openTextDocument(uri);
  const editor = await vscode.window.showTextDocument(document, {
    preview: false,
    viewColumn: vscode.ViewColumn.One,
  });
  const target = Math.max(0, line - excerpt.line_start);
  const position = new vscode.Position(target, 0);
  editor.selection = new vscode.Selection(position, position);
  editor.revealRange(new vscode.Range(position, position), vscode.TextEditorRevealType.InCenter);
  vscode.window.setStatusBarMessage(
    `ARCHMELD: ${file}:${line} from ${settings.snapshotId.slice(0, 19)}… (historical snapshot)`,
    5000,
  );
  return {
    run_id: runId,
    span_id: selected.span.span_id,
    path: file,
    line,
    line_start: excerpt.line_start,
    line_end: excerpt.line_end,
    snapshot_id: settings.snapshotId,
    document_text: excerpt.text,
    truncated: excerpt.truncated,
  };
}

function wordAtCursor() {
  const editor = vscode.window.activeTextEditor;
  if (!editor) {
    return null;
  }
  const word = editor.document.getText(editor.document.getWordRangeAtPosition(editor.selection.active));
  return word || null;
}

function relativePath() {
  const editor = vscode.window.activeTextEditor;
  if (!editor) {
    throw new Error('Open a source file first.');
  }
  if (!vscode.workspace.workspaceFolders || vscode.workspace.workspaceFolders.length === 0) {
    throw new Error('Open the repository folder in VS Code so the cursor path can be resolved.');
  }
  return vscode.workspace.asRelativePath(editor.document.uri, false).replace(/\\/g, '/');
}

async function findEntityAtCursor(settings) {
  const editor = vscode.window.activeTextEditor;
  if (!editor) {
    throw new Error('Open a source file first.');
  }
  const query = wordAtCursor();
  if (!query) {
    throw new Error('Place the cursor on a symbol name first.');
  }
  const response = await getClient(settings).query({
    project_id: settings.projectId,
    snapshot_id: settings.snapshotId,
    kind: 'symbols',
    query,
    entity_ids: [],
    relation_kinds: [],
    max_nodes: 50,
    max_depth: 0,
    cursor: null,
  });
  const path = relativePath();
  const entity = model.entityAt(response.entities, {
    path,
    line: editor.selection.active.line,
    character: editor.selection.active.character,
  });
  if (!entity) {
    throw new Error(
      `No indexed entity is anchored at ${path}:${editor.selection.active.line + 1}. ` +
        'This is an explicit miss, not a fallback to a similarly named symbol.',
    );
  }
  return entity;
}

async function findEntity(settings, query) {
  const response = await getClient(settings).query({
    project_id: settings.projectId,
    snapshot_id: settings.snapshotId,
    kind: 'symbols',
    query,
    entity_ids: [],
    relation_kinds: [],
    max_nodes: 20,
    max_depth: 0,
    cursor: null,
  });
  return response.entities[0] || null;
}

async function openGraph(context) {
  const settings = readSettings();
  const entity = await findEntityAtCursor(settings);
  const result = await getClient(settings).query({
    project_id: settings.projectId,
    snapshot_id: settings.snapshotId,
    kind: 'neighbors',
    query: '',
    entity_ids: [entity.id],
    relation_kinds: [],
    max_nodes: 60,
    max_depth: 2,
    cursor: null,
  });
  model.mergeResult({ ...result, entities: [entity, ...result.entities] });
  model.pushSelection({
    projectId: settings.projectId,
    snapshotId: settings.snapshotId,
    entityId: entity.id,
    originView: model.view,
  });
  await ensurePanel(context);
  refresh(true);
  status.text = `ARCHMELD · ${settings.snapshotId.slice(7, 19)}… · coverage ${result.coverage ? result.coverage.state : 'unknown'}`;
  status.show();
  return entity;
}

async function openArchitectureReview(context) {
  const settings = readSettings();
  const architectureId = await vscode.window.showInputBox({
    prompt: 'Architecture candidate id',
    placeHolder: 'arch.codeboarding.…',
    ignoreFocusOut: true,
  });
  if (!architectureId) {
    return;
  }
  const api = getClient(settings);
  const [architecture, review, checks] = await Promise.all([
    api.architecture(settings.projectId, architectureId),
    api.architectureReview(settings.projectId, architectureId),
    api.architectureCheck(settings.projectId, architectureId),
  ]);
  if (!reviewPanel) {
    reviewPanel = vscode.window.createWebviewPanel(
      'archmeld.review',
      'ARCHMELD Candidate Review',
      vscode.ViewColumn.Beside,
      { enableScripts: false, retainContextWhenHidden: true },
    );
    reviewPanel.onDidDispose(() => {
      reviewPanel = undefined;
    });
  }
  reviewPanel.title = `ARCHMELD Candidate · ${architectureId}`;
  reviewPanel.webview.html = renderReviewHtml({
    architecture,
    review,
    checks,
    cspSource: reviewPanel.webview.cspSource,
  });
  reviewPanel.reveal(vscode.ViewColumn.Beside, true);
}

async function openTargetEditor(context) {
  const settings = readSettings();
  const baseId = await vscode.window.showInputBox({
    prompt: 'As-Is architecture id to edit from',
    placeHolder: 'arch.…',
    ignoreFocusOut: true,
  });
  if (!baseId) {
    return;
  }
  const api = getClient(settings);
  const base = await api.architecture(settings.projectId, baseId);
  if (base.layer !== 'as_is') {
    throw new Error('The base architecture for a To-Be edit must have layer=as_is.');
  }
  if (!targetPanel) {
    targetPanel = vscode.window.createWebviewPanel(
      'archmeld.targetEditor',
      'ARCHMELD To-Be Editor',
      vscode.ViewColumn.Beside,
      { enableScripts: true, retainContextWhenHidden: true },
    );
    targetPanel.onDidDispose(() => {
      targetPanel = undefined;
    });
    targetPanel.webview.onDidReceiveMessage(async (message) => {
      if (!message || message.type !== 'target.preview') {
        return;
      }
      try {
        const current = readSettings();
        const diff = await getClient(current).architectureDiff(
          current.projectId,
          baseId,
          message.target,
        );
        targetPanel?.webview.postMessage({ type: 'target.result', diff });
      } catch (error) {
        targetPanel?.webview.postMessage({ type: 'target.result', error: error.message });
      }
    });
  }
  targetPanel.title = `ARCHMELD To-Be · ${baseId}`;
  targetPanel.webview.html = renderTargetEditorHtml({
    base,
    cspSource: targetPanel.webview.cspSource,
  });
  targetPanel.reveal(vscode.ViewColumn.Beside, true);
}

function idempotencyKey(prefix) {
  return `${prefix}-${crypto.randomBytes(16).toString('hex')}`;
}

async function loadChangeset(settings, changesetId) {
  const api = getClient(settings);
  const [changeset, diff, list] = await Promise.all([
    api.changeset(settings.projectId, changesetId),
    api.changesetDiff(settings.projectId, changesetId),
    api.verifications(settings.projectId, changesetId),
  ]);
  const summary = (list.verifications ?? [])[0] ?? null;
  const verification = summary
    ? await api.verification(settings.projectId, summary.verification_id)
    : null;
  return { changeset, diff, verification };
}

async function refreshChangesetPanel(settings, changesetId) {
  if (!changesetPanel) {
    return;
  }
  const { changeset, diff, verification } = await loadChangeset(settings, changesetId);
  changesetPanel.title = `ARCHMELD ChangeSet · ${changesetId}`;
  changesetPanel.webview.html = renderChangesetHtml({
    changeset,
    diff,
    verification,
    cspSource: changesetPanel.webview.cspSource,
  });
}

async function openChangesetReview(context) {
  const settings = readSettings();
  const changesetId = await vscode.window.showInputBox({
    prompt: 'ChangeSet id',
    placeHolder: 'changeset.…',
    ignoreFocusOut: true,
  });
  if (!changesetId) {
    return;
  }
  if (!changesetPanel) {
    changesetPanel = vscode.window.createWebviewPanel(
      'archmeld.changeset',
      'ARCHMELD ChangeSet',
      vscode.ViewColumn.Beside,
      { enableScripts: true, retainContextWhenHidden: true },
    );
    changesetPanel.onDidDispose(() => {
      changesetPanel = undefined;
    });
    changesetPanel.webview.onDidReceiveMessage(async (message) => {
      if (!message || typeof message.type !== 'string' || !message.type.startsWith('changeset.')) {
        return;
      }
      try {
        const current = readSettings();
        const api = getClient(current);
        if (message.type === 'changeset.approve') {
          await api.approveChangeset(current.projectId, message.changesetId, {
            expectedRevision: message.revision,
            preparedDigest: message.preparedDigest,
            actor: 'vscode.local',
            idempotencyKey: idempotencyKey('vscode-approve'),
          });
          vscode.window.showInformationMessage('ARCHMELD: prepared change approved.');
        } else if (message.type === 'changeset.apply') {
          if (!current.workspaceHandle) {
            throw new Error('Set archmeld.workspaceHandle before applying a ChangeSet.');
          }
          const confirm = await vscode.window.showWarningMessage(
            `Apply ${message.changesetId} exactly as prepared? Files will be replaced after per-file checks.`,
            { modal: true },
            'Apply',
          );
          if (confirm !== 'Apply') {
            return;
          }
          await api.applyChangeset(current.projectId, message.changesetId, {
            expectedRevision: message.revision,
            preparedDigest: message.preparedDigest,
            approvalId: message.approvalId,
            actor: 'vscode.local',
            workspaceHandle: current.workspaceHandle,
            idempotencyKey: idempotencyKey('vscode-apply'),
          });
          vscode.window.showInformationMessage('ARCHMELD: change applied; verification is still pending.');
        } else if (message.type === 'changeset.verify') {
          if (!current.workspaceHandle) {
            throw new Error('Set archmeld.workspaceHandle before verifying a ChangeSet.');
          }
          const reason = await vscode.window.showInputBox({
            prompt: 'Verification execution grant reason',
            placeHolder: 'Why may the frozen checks run now?',
            ignoreFocusOut: true,
          });
          if (!reason) {
            return;
          }
          const confirm = await vscode.window.showWarningMessage(
            `Run the frozen contract checks for ${message.changesetId}? The commands execute locally.`,
            { modal: true },
            'Run checks',
          );
          if (confirm !== 'Run checks') {
            return;
          }
          await api.verifyChangeset(current.projectId, message.changesetId, {
            expectedRevision: message.revision,
            workspaceHandle: current.workspaceHandle,
            grantId: idempotencyKey('vscode-verify'),
            reason,
            idempotencyKey: idempotencyKey('vscode-verify-request'),
          });
          vscode.window.showInformationMessage('ARCHMELD: verification finished; inspect the check states.');
        } else if (message.type === 'changeset.accept') {
          if (!current.workspaceHandle) {
            throw new Error('Set archmeld.workspaceHandle before accepting a ChangeSet.');
          }
          if (!message.verificationId) {
            throw new Error('No verification id is available for acceptance.');
          }
          const confirm = await vscode.window.showWarningMessage(
            `Accept ${message.changesetId} using verification ${message.verificationId}? Acceptance rechecks for drift.`,
            { modal: true },
            'Accept',
          );
          if (confirm !== 'Accept') {
            return;
          }
          await api.acceptChangeset(current.projectId, message.changesetId, {
            expectedRevision: message.revision,
            verificationId: message.verificationId,
            workspaceHandle: current.workspaceHandle,
            actor: 'vscode.local',
            idempotencyKey: idempotencyKey('vscode-accept'),
          });
          vscode.window.showInformationMessage('ARCHMELD: change accepted.');
        } else if (message.type === 'changeset.recover') {
          if (!current.workspaceHandle) {
            throw new Error('Set archmeld.workspaceHandle before recovering a ChangeSet.');
          }
          const confirm = await vscode.window.showWarningMessage(
            `Recover the owned writes of ${message.changesetId}? Later user edits are preserved as conflicts.`,
            { modal: true },
            'Recover',
          );
          if (confirm !== 'Recover') {
            return;
          }
          await api.recoverChangeset(current.projectId, message.changesetId, {
            expectedRevision: message.revision,
            actor: 'vscode.local',
            workspaceHandle: current.workspaceHandle,
            idempotencyKey: idempotencyKey('vscode-recover'),
          });
          vscode.window.showInformationMessage('ARCHMELD: recovery finished; inspect conflicts before continuing.');
        }
        await refreshChangesetPanel(current, message.changesetId);
      } catch (error) {
        vscode.window.showErrorMessage(`ARCHMELD: ${error.message}`);
      }
    });
  }
  await refreshChangesetPanel(settings, changesetId);
  changesetPanel.reveal(vscode.ViewColumn.Beside, true);
}

function activate(context) {
  status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 50);
  status.name = 'ARCHMELD snapshot';
  context.subscriptions.push(status);

  context.subscriptions.push(
    vscode.workspace.registerTextDocumentContentProvider(SOURCE_SCHEME, {
      provideTextDocumentContent(uri) {
        return sourceCache.get(uri.path) ?? 'Source is no longer cached. Reopen it from the graph.';
      },
    }),
    vscode.workspace.registerTextDocumentContentProvider(TRACE_SOURCE_SCHEME, {
      provideTextDocumentContent(uri) {
        return (
          traceSourceCache.get(uri.path) ??
          'Source is no longer cached. Reopen it from the runtime trace view.'
        );
      },
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('archmeld.openGraph', async () => {
      try {
        await openGraph(context);
      } catch (error) {
        vscode.window.showErrorMessage(`ARCHMELD: ${error.message}`);
      }
    }),
    vscode.commands.registerCommand('archmeld.openSource', async () => {
      try {
        const settings = readSettings();
        const selection = model.selection;
        const query = selection?.entityId ?? null;
        const entityId = query || (await findEntity(settings, wordAtCursor() || ''))?.id;
        if (!entityId) {
          throw new Error('Select an entity in the graph or place the cursor on an indexed symbol.');
        }
        await openHistoricalSource(settings, entityId, 0);
      } catch (error) {
        vscode.window.showErrorMessage(`ARCHMELD: ${error.message}`);
      }
    }),
    vscode.commands.registerCommand('archmeld.openTraceFrame', async (argument) => {
      try {
        return await openTraceFrame(readSettings(), argument);
      } catch (error) {
        vscode.window.showErrorMessage(`ARCHMELD: ${error.message}`);
        throw error;
      }
    }),
    vscode.commands.registerCommand('archmeld.reviewArchitecture', async () => {
      try {
        await openArchitectureReview(context);
      } catch (error) {
        vscode.window.showErrorMessage(`ARCHMELD: ${error.message}`);
      }
    }),
    vscode.commands.registerCommand('archmeld.editTargetGraph', async () => {
      try {
        await openTargetEditor(context);
      } catch (error) {
        vscode.window.showErrorMessage(`ARCHMELD: ${error.message}`);
      }
    }),
    vscode.commands.registerCommand('archmeld.reviewChangeSet', async () => {
      try {
        await openChangesetReview(context);
      } catch (error) {
        vscode.window.showErrorMessage(`ARCHMELD: ${error.message}`);
      }
    }),
    vscode.commands.registerCommand('archmeld.exportReport', async () => {
      try {
        return await exportReport(readSettings());
      } catch (error) {
        vscode.window.showErrorMessage(`ARCHMELD: ${error.message}`);
        throw error;
      }
    }),
    vscode.commands.registerCommand('archmeld.back', () => {
      const previous = model.back();
      if (!previous) {
        vscode.window.showInformationMessage('ARCHMELD: no previous selection.');
        return;
      }
      refresh();
    }),
  );

  if (process.env.ARCHMELD_PAINT_PROBE === '1') {
    context.subscriptions.push(
      vscode.commands.registerCommand('archmeld.diagnostics.paintReport', () => ({
        paint_reports: paintDiagnostics.paintReports,
        render_reports: paintDiagnostics.renderReports,
        pending_probes: paintDiagnostics.waiters.length,
      })),
      vscode.commands.registerCommand('archmeld.diagnostics.paintProbe', async () => {
        if (!panel) {
          throw new Error('ARCHMELD graph panel is not open');
        }
        const probeId = `paint-${++paintDiagnostics.sequence}`;
        const started = process.hrtime.bigint();
        const report = await new Promise((resolve, reject) => {
          const timer = setTimeout(() => {
            const index = paintDiagnostics.waiters.findIndex((waiter) => waiter.probeId === probeId);
            if (index >= 0) {
              paintDiagnostics.waiters.splice(index, 1);
            }
            reject(new Error(`webview paint probe ${probeId} timed out`));
          }, 5000);
          paintDiagnostics.waiters.push({
            probeId,
            resolve: (value) => {
              clearTimeout(timer);
              resolve(value);
            },
          });
          panel.webview.postMessage({ type: 'archmeld.paintProbe', probeId });
        });
        const roundtripMs = Number(process.hrtime.bigint() - started) / 1e6;
        return { probe_id: probeId, roundtrip_ms: roundtripMs, report };
      }),
    );
  }
}

function deactivate() {
  panel?.dispose();
  panel = undefined;
  reviewPanel?.dispose();
  reviewPanel = undefined;
  changesetPanel?.dispose();
  changesetPanel = undefined;
  targetPanel?.dispose();
  targetPanel = undefined;
  client = undefined;
}

module.exports = { activate, deactivate };
