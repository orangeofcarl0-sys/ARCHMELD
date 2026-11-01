// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

'use strict';

/**
 * Deterministic graph projection and selection model.
 *
 * Positions are derived from entity identity only, so adding neighbors never
 * re-randomizes the existing layout. Relations keep their basis/resolution and
 * an unknown edge is never rendered as a certain one.
 */

const VIEWS = ['file', 'responsibility', 'runtime'];

function hash32(text) {
  let hash = 2166136261;
  for (let index = 0; index < text.length; index += 1) {
    hash ^= text.charCodeAt(index);
    hash = Math.imul(hash, 16777619);
  }
  return hash >>> 0;
}

function positionFor(entityId) {
  const h = hash32(entityId);
  const angle = ((h % 3600) / 3600) * Math.PI * 2;
  const radius = 90 + ((h >>> 9) % 110);
  return {
    x: Math.round(300 + Math.cos(angle) * radius),
    y: Math.round(230 + Math.sin(angle) * radius),
  };
}

function relationStyle(relation, evidenceKinds) {
  const basis = relation.basis || 'unknown';
  const resolution = relation.resolution || 'unresolved';
  const ids = Array.isArray(relation.evidence_ids) ? relation.evidence_ids : [];
  const knownKinds = new Set(evidenceKinds || []);
  const hasEvidence = ids.length > 0 && ids.every((id) => knownKinds.size === 0 || knownKinds.has(id));
  const certain = hasEvidence && resolution !== 'unresolved' && resolution !== 'candidate';
  let lineStyle = 'solid';
  if (!hasEvidence) {
    lineStyle = 'dotted';
  } else if (resolution === 'candidate' || resolution === 'unresolved' || basis === 'model') {
    lineStyle = 'dashed';
  }
  return {
    basis,
    resolution,
    label:
      basis === 'semantic' && resolution === 'resolved'
        ? 'resolved'
        : basis === 'runtime' && resolution === 'observed'
          ? 'observed'
          : hasEvidence
            ? `${basis}/${resolution}`
            : 'unknown',
    certainty: certain,
    lineStyle,
  };
}

class GraphModel {
  constructor() {
    this.nodes = new Map();
    this.edges = new Map();
    this.evidence = new Map();
    this.view = 'file';
    this.history = [];
    this.selection = null;
    this.warnings = [];
    this.coverage = null;
  }

  mergeResult(result) {
    const evidenceKinds = [];
    for (const item of result.evidence || []) {
      this.evidence.set(item.id, item);
      evidenceKinds.push(item.id);
    }
    for (const entity of result.entities || []) {
      if (!this.nodes.has(entity.id)) {
        this.nodes.set(entity.id, {
          entity,
          position: positionFor(entity.id),
        });
      }
    }
    for (const relation of result.relations || []) {
      this.edges.set(relation.id, relation);
    }
    if (result.coverage) {
      this.coverage = result.coverage;
    }
    this.warnings = (result.warnings || []).slice();
    return this;
  }

  /**
   * Precise lookup: the cursor position must fall inside an anchor of the
   * entity, on the same snapshot-relative path. Line numbers are 0-based
   * (editor convention); anchor line ranges are 1-based from upstream parsers.
   */
  entityAt(entities, { path, line, character }) {
    let best = null;
    for (const entity of entities || []) {
      for (const anchor of entity.anchors || []) {
        if (anchor.path !== path) {
          continue;
        }
        const anchorLineStart = anchor.line_start;
        const anchorLineEnd = anchor.line_end ?? anchor.line_start;
        if (anchorLineStart === null || anchorLineStart === undefined) {
          continue;
        }
        const lineOk = line + 1 >= anchorLineStart && line + 1 <= anchorLineEnd;
        if (!lineOk) {
          continue;
        }
        const byteWidth =
          anchor.precision === 'bytes' && anchor.byte_start !== null && anchor.byte_end !== null
            ? anchor.byte_end - anchor.byte_start
            : Number.MAX_SAFE_INTEGER;
        const lineWidth = anchorLineEnd - anchorLineStart;
        const score = [anchor.precision === 'bytes' ? 0 : 1, lineWidth, byteWidth, entity.id];
        if (!best || compareScores(score, best.score) < 0) {
          best = { entity, score, character };
        }
      }
    }
    return best ? best.entity : null;
  }

  setView(view) {
    if (!VIEWS.includes(view)) {
      throw new Error(`unknown view ${view}`);
    }
    this.view = view;
  }

  /** Same entity, different projection: no forced parent/child hierarchy. */
  visibleNodes() {
    const wanted = {
      file: new Set(['file', 'symbol', 'type']),
      responsibility: new Set(['module', 'component', 'boundary', 'external']),
      runtime: new Set(['process', 'thread', 'task', 'queue', 'resource']),
    }[this.view];
    const nodes = [...this.nodes.values()].filter((node) => wanted.has(node.entity.kind));
    if (nodes.length === 0) {
      return {
        nodes: [...this.nodes.values()],
        warning: `view "${this.view}" has no matching entity kinds; showing the complete local graph`,
      };
    }
    return { nodes, warning: null };
  }

  visibleEdges(visibleIds) {
    const ids = new Set(visibleIds);
    return [...this.edges.values()]
      .filter((edge) => ids.has(edge.source_id) && ids.has(edge.target_id))
      .map((edge) => ({ ...edge, style: relationStyle(edge, [...this.evidence.keys()]) }));
  }

  pushSelection(selection) {
    const signature = JSON.stringify(selection);
    if (this.selection && JSON.stringify(this.selection) === signature) {
      return false;
    }
    if (this.selection) {
      this.history.push(this.selection);
    }
    this.selection = selection;
    return true;
  }

  back() {
    const previous = this.history.pop();
    if (!previous) {
      return null;
    }
    this.selection = previous;
    return previous;
  }

  evidenceFor(edgeId) {
    const edge = this.edges.get(edgeId);
    if (!edge) {
      return [];
    }
    return (edge.evidence_ids || []).map((id) => this.evidence.get(id)).filter(Boolean);
  }
}

function compareScores(a, b) {
  for (let index = 0; index < a.length - 1; index += 1) {
    if (a[index] !== b[index]) {
      return a[index] < b[index] ? -1 : 1;
    }
  }
  return String(a[a.length - 1]).localeCompare(String(b[b.length - 1]));
}

module.exports = { GraphModel, relationStyle, positionFor, VIEWS };
