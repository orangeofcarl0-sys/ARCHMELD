// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

'use strict';

/**
 * Offline webview. When a local Cytoscape.js asset is supplied the graph is
 * rendered with Cytoscape (preset layout); otherwise a deterministic SVG
 * fallback keeps the same semantics for tests and minimal hosts. No remote
 * scripts and no inline source execution.
 */

const crypto = require('node:crypto');

function escapeHtml(text) {
  return String(text)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

function buildElements(graph) {
  const projection = graph ? graph.visibleNodes() : { nodes: [], warning: null };
  const ids = projection.nodes.map((node) => node.entity.id);
  const edges = graph ? graph.visibleEdges(ids) : [];
  const nodeById = new Map(projection.nodes.map((node) => [node.entity.id, node]));
  const nodes = projection.nodes.map((node) => ({
    data: {
      id: node.entity.id,
      label: node.entity.qualified_name || node.entity.id,
      kind: node.entity.kind,
    },
    position: { x: node.position.x, y: node.position.y },
  }));
  const edgeElements = edges
    .filter((edge) => nodeById.has(edge.source_id) && nodeById.has(edge.target_id))
    .map((edge) => ({
      data: {
        id: edge.id,
        source: edge.source_id,
        target: edge.target_id,
        label: edge.style.label,
        basis: edge.style.basis,
        resolution: edge.style.resolution,
      },
      classes: [
        edge.style.certainty ? 'certain' : 'uncertain',
        edge.style.lineStyle,
      ].join(' '),
    }));
  return { nodes, edges: edgeElements, warning: projection.warning, visible: projection.nodes, nodeById };
}

function paintProbeScript(nonce) {
  return `<script nonce="${nonce}">
(function () {
  if (window.__archmeldPaintProbeInstalled) { return; }
  window.__archmeldPaintProbeInstalled = true;
  const vscode = window.__archmeldVsCodeApi || (window.__archmeldVsCodeApi = acquireVsCodeApi());
  const timeOrigin = performance.timeOrigin;
  function paintEntries() {
    return performance.getEntriesByType('paint').map(function (entry) {
      return { name: entry.name, start_ms: entry.startTime };
    });
  }
  let lcpMs = null;
  try {
    new PerformanceObserver(function (list) {
      const entries = list.getEntries();
      if (entries.length) { lcpMs = entries[entries.length - 1].startTime; }
    }).observe({ type: 'largest-contentful-paint', buffered: true });
  } catch (error) { /* renderer without LCP support */ }
  function snapshot(phase) {
    const navigation = performance.getEntriesByType('navigation')[0];
    const paints = paintEntries();
    const fcp = paints.find(function (entry) { return entry.name === 'first-contentful-paint'; });
    return {
      probe_version: 1,
      phase: phase,
      time_origin_ms: timeOrigin,
      captured_ms: performance.now(),
      ready_state: document.readyState,
      paint_entries: paints,
      first_contentful_paint_ms: fcp ? fcp.start_ms : null,
      largest_contentful_paint_ms: lcpMs,
      navigation: navigation ? {
        response_end_ms: navigation.responseEnd,
        dom_content_loaded_ms: navigation.domContentLoadedEventEnd,
        load_event_ms: navigation.loadEventEnd
      } : null
    };
  }
  function postReport(phase) {
    vscode.postMessage({ type: 'paintReport', report: snapshot(phase) });
  }
  let settled = false;
  function settle() {
    if (settled) { return; }
    settled = true;
    requestAnimationFrame(function (first) {
      requestAnimationFrame(function (second) {
        const report = snapshot('load');
        report.first_raf_ms = first;
        report.second_raf_ms = second;
        report.double_raf_ms = second - first;
        vscode.postMessage({ type: 'paintReport', report: report });
      });
    });
    setTimeout(function () { postReport('settled'); }, 1000);
  }
  if (document.readyState === 'complete') { settle(); }
  else { window.addEventListener('load', settle, { once: true }); }
  window.addEventListener('message', function (event) {
    const data = event.data;
    if (!data || data.type !== 'archmeld.paintProbe') { return; }
    const started = performance.now();
    // Invalidate style/layout so the two animation frames follow real work.
    document.body.style.setProperty('--archmeld-paint-probe', String(started));
    void document.body.offsetHeight;
    requestAnimationFrame(function (first) {
      requestAnimationFrame(function (second) {
        const report = snapshot('probe');
        report.probe_id = data.probeId;
        report.start_ms = started;
        report.first_raf_ms = first;
        report.second_raf_ms = second;
        report.double_raf_ms = second - first;
        report.protocol_elapsed_ms = performance.now() - started;
        vscode.postMessage({ type: 'paintProbeResult', probeId: data.probeId, report: report });
      });
    });
  });
  postReport('interactive');
})();
</script>`;
}

function embedJson(value) {
  return JSON.stringify(value)
    .replace(/</g, '\\u003c')
    .replace(/>/g, '\\u003e')
    .replace(/&/g, '\\u0026');
}

function renderHtml({ graph, title = 'ARCHMELD graph', error = null, stale = false, cytoscapeUri, cspSource = '', nonce, paintProbe = false }) {
  const projection = buildElements(graph);
  const warnings = [...(graph ? graph.warnings : [])];
  if (projection.warning) {
    warnings.push(projection.warning);
  }
  if (stale) {
    warnings.push('showing a historical snapshot; this is not the current file');
  }
  const warningMarkup = warnings.map((warning) => `<li>${escapeHtml(warning)}</li>`).join('');
  const useCytoscape = Boolean(cytoscapeUri) && !error && projection.visible.length > 0;
  const scriptNonce = nonce || crypto.randomBytes(16).toString('hex');
  let body;
  if (error) {
    body = `<div class="state error"><strong>Graph unavailable</strong><p>${escapeHtml(error)}</p></div>`;
  } else if (projection.visible.length === 0) {
    body =
      '<div class="state empty"><strong>No entities</strong><p>The query returned an empty graph; this is not an indexing failure.</p></div>';
  } else if (useCytoscape) {
    body = `<div id="cy" role="img" aria-label="${escapeHtml(title)}"></div>
<script type="application/json" id="archmeld-graph">${embedJson({ nodes: projection.nodes, edges: projection.edges })}</script>
<script nonce="${scriptNonce}" src="${escapeHtml(cytoscapeUri)}"></script>
<script nonce="${scriptNonce}">
  const payload = JSON.parse(document.getElementById('archmeld-graph').textContent);
  const vscode = window.__archmeldVsCodeApi || (window.__archmeldVsCodeApi = acquireVsCodeApi());
  const cy = window.cytoscape({
    container: document.getElementById('cy'),
    elements: [...payload.nodes, ...payload.edges],
    layout: { name: 'preset', fit: true, padding: 30 },
    wheelSensitivity: 0.2,
    style: [
      { selector: 'node', style: {
        'background-color': '#e0f2fe', 'border-color': '#0369a1', 'border-width': 1,
        label: 'data(label)', 'font-size': 10, 'text-valign': 'bottom', 'text-margin-y': 4,
        color: '#111', 'text-wrap': 'wrap', 'text-max-width': 120
      }},
      { selector: 'edge', style: {
        width: 2, 'line-color': '#2563eb', 'target-arrow-color': '#2563eb',
        'target-arrow-shape': 'triangle', 'curve-style': 'bezier', label: 'data(label)',
        'font-size': 8, color: '#555', 'text-rotation': 'autorotate',
        'text-background-color': '#fff', 'text-background-opacity': 0.8
      }},
      { selector: 'edge.uncertain', style: {
        'line-color': '#b45309', 'target-arrow-color': '#b45309'
      }},
      { selector: 'edge.dashed', style: { 'line-style': 'dashed' }},
      { selector: 'edge.dotted', style: { 'line-style': 'dotted' }}
    ]
  });
  cy.ready(() => {
    vscode.postMessage({ type: 'renderReport', report: {
      renderer: 'cytoscape',
      render_ms: performance.now(),
      node_count: payload.nodes.length,
      edge_count: payload.edges.length
    }});
  });
  cy.on('tap', 'node', (event) => vscode.postMessage({ type: 'selectEntity', entityId: event.target.id() }));
  cy.on('tap', 'edge', (event) => vscode.postMessage({ type: 'selectEdge', edgeId: event.target.id() }));
  document.addEventListener('click', (event) => {
    const view = event.target.closest('[data-view]');
    if (view) { vscode.postMessage({ type: 'setView', view: view.getAttribute('data-view') }); }
  });
</script>`;
  } else {
    body = svgFallback(projection, title, scriptNonce);
  }
  const csp = useCytoscape
    ? `default-src 'none'; style-src 'unsafe-inline'; script-src 'nonce-${scriptNonce}' ${cspSource};`
    : `default-src 'none'; style-src 'unsafe-inline'; script-src 'nonce-${scriptNonce}';`;
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta http-equiv="Content-Security-Policy" content="${csp}" />
<style>
  body { font-family: var(--vscode-font-family); color: var(--vscode-foreground); background: var(--vscode-editor-background); margin: 0; padding: 12px; }
  header { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
  button { background: var(--vscode-button-background); color: var(--vscode-button-foreground); border: 0; padding: 4px 10px; cursor: pointer; }
  .legend { margin-left: auto; font-size: 11px; display: flex; gap: 12px; align-items: center; }
  .swatch { display: inline-block; width: 22px; height: 0; border-top: 2px solid #2563eb; vertical-align: middle; }
  .swatch.dashed { border-top-style: dashed; }
  .swatch.dotted { border-top-style: dotted; border-top-color: #b45309; }
  #cy { width: 100%; height: 460px; background: var(--vscode-editor-background); }
  svg { width: 100%; height: auto; background: var(--vscode-editor-background); }
  .node { cursor: pointer; }
  .node-label, .edge-label { font-size: 10px; fill: var(--vscode-foreground); }
  .edge-label { fill: var(--vscode-descriptionForeground); }
  .warnings { margin-top: 8px; font-size: 12px; color: var(--vscode-descriptionForeground); }
  .state { padding: 24px; border: 1px solid var(--vscode-panel-border); }
  .state.error { border-color: var(--vscode-errorForeground); }
</style>
</head>
<body>
<header>
  <strong>${escapeHtml(title)}</strong>
  <button data-view="file">Files</button>
  <button data-view="responsibility">Responsibilities</button>
  <button data-view="runtime">Runtime</button>
  <span class="legend"><span><span class="swatch"></span> certain</span><span><span class="swatch dashed"></span> candidate</span><span><span class="swatch dotted"></span> unknown</span></span>
</header>
${body}
<ul class="warnings">${warningMarkup}</ul>
${paintProbe ? paintProbeScript(scriptNonce) : ''}
</body>
</html>`;
}

function svgFallback(projection, title, nonce) {
  const ids = projection.visible.map((node) => node.entity.id);
  const nodeById = new Map(projection.visible.map((node) => [node.entity.id, node]));
  const edgeMarkup = projection.edges
    .map((edge) => {
      const source = nodeById.get(edge.data.source);
      const target = nodeById.get(edge.data.target);
      if (!source || !target) {
        return '';
      }
      const dash = edge.classes.includes('dotted') ? '2 4' : edge.classes.includes('dashed') ? '6 5' : '';
      const midX = Math.round((source.position.x + target.position.x) / 2);
      const midY = Math.round((source.position.y + target.position.y) / 2);
      return `<g class="edge ${edge.classes.includes('certain') ? 'certain' : 'uncertain'}" data-edge="${escapeHtml(edge.data.id)}">
  <line x1="${source.position.x}" y1="${source.position.y}" x2="${target.position.x}" y2="${target.position.y}"
        stroke="${edge.classes.includes('certain') ? '#2563eb' : '#b45309'}" stroke-width="2"
        ${dash ? `stroke-dasharray="${dash}"` : ''} />
  <text x="${midX}" y="${midY - 4}" class="edge-label">${escapeHtml(edge.data.label)}</text>
</g>`;
    })
    .join('\n');
  const nodeMarkup = projection.visible
    .map((node) => `<g class="node" data-entity="${escapeHtml(node.entity.id)}" transform="translate(${node.position.x},${node.position.y})">
  <circle r="14" fill="#e0f2fe" stroke="#0369a1" />
  <text y="28" text-anchor="middle" class="node-label">${escapeHtml(node.entity.qualified_name || node.entity.id)}</text>
</g>`)
    .join('\n');
  return `<svg viewBox="0 0 600 460" role="img" aria-label="${escapeHtml(title)}">${edgeMarkup}${nodeMarkup}</svg>
<script nonce="${nonce}">
  const vscode = window.__archmeldVsCodeApi || (window.__archmeldVsCodeApi = acquireVsCodeApi());
  requestAnimationFrame(() => {
    vscode.postMessage({ type: 'renderReport', report: {
      renderer: 'svg',
      render_ms: performance.now(),
      node_count: document.querySelectorAll('[data-entity]').length,
      edge_count: document.querySelectorAll('[data-edge]').length
    }});
  });
  document.addEventListener('click', (event) => {
    const node = event.target.closest('[data-entity]');
    if (node) { vscode.postMessage({ type: 'selectEntity', entityId: node.getAttribute('data-entity') }); return; }
    const edge = event.target.closest('[data-edge]');
    if (edge) { vscode.postMessage({ type: 'selectEdge', edgeId: edge.getAttribute('data-edge') }); return; }
    const view = event.target.closest('[data-view]');
    if (view) { vscode.postMessage({ type: 'setView', view: view.getAttribute('data-view') }); }
  });
</script>`;
}

module.exports = { renderHtml, escapeHtml, buildElements };
