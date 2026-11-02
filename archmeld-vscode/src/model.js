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

const VIEWS = ['file', 'module', 'responsibility', 'runtime'];

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
  const projection =
    basis === 'projection' ||
    basis === 'architecture_projection' ||
    resolution === 'aggregate';
  const spanParent = resolution === 'observed_span_parent';
  const certain =
    !projection &&
    !spanParent &&
    hasEvidence &&
    resolution !== 'unresolved' &&
    resolution !== 'candidate';
  let lineStyle = 'solid';
  if (projection || spanParent) {
    lineStyle = 'dotted';
  } else if (!hasEvidence) {
    lineStyle = 'dotted';
  } else if (resolution === 'candidate' || resolution === 'unresolved' || basis === 'model') {
    lineStyle = 'dashed';
  }
  return {
    basis,
    resolution,
    label:
      spanParent
        ? 'span parent'
        : projection
          ? basis === 'architecture_projection'
            ? 'component projection'
            : 'module projection'
          : basis === 'semantic' && resolution === 'resolved'
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

function moduleNameFor(entity) {
  const path = (entity.anchors || [])
    .map((anchor) => anchor.path)
    .find((value) => typeof value === 'string' && value.length > 0);
  if (!path) {
    return '(unassigned)';
  }
  const normalized = path.replace(/\\/g, '/').replace(/^\.\//, '');
  const slash = normalized.indexOf('/');
  return slash === -1 ? '(root)' : normalized.slice(0, slash);
}

function runtimeEntityId(span) {
  const value =
    span?.attributes?.['archmeld.entity_id'] ??
    span?.entity_id ??
    span?.attributes?.entity_id;
  return typeof value === 'string' && value.length > 0 ? value : null;
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
    // Module-view positions are assigned once and then kept; a later merge
    // must not reshuffle modules the user is already looking at.
    this.modulePositions = new Map();
    // User-dragged positions for the current view, keyed by the node id the
    // webview actually rendered (entity id or synthetic module id).
    this.positionOverrides = new Map();
    this.runtimeFilter = null;
    this.architecture = null;
    this.componentByEntity = new Map();
    this.componentPositions = new Map();
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

  setPositionOverrides(entries) {
    this.positionOverrides.clear();
    for (const [id, position] of Object.entries(entries || {})) {
      this.setPosition(id, position);
    }
  }

  clearPositionOverrides() {
    this.positionOverrides.clear();
  }

  exportPositionOverrides() {
    return Object.fromEntries(this.positionOverrides);
  }

  setPosition(entityId, position) {
    if (
      typeof entityId !== 'string' ||
      entityId.length === 0 ||
      !position ||
      !Number.isFinite(position.x) ||
      !Number.isFinite(position.y)
    ) {
      return false;
    }
    this.positionOverrides.set(entityId, {
      x: Math.round(position.x),
      y: Math.round(position.y),
    });
    return true;
  }

  positionForNode(entityId, fallback) {
    return this.positionOverrides.get(entityId) || fallback;
  }

  /** Attach a runtime run's mapped entities and observed span-parent edges. */
  setRuntimeRun(timeline, runId) {
    const spans = timeline?.spans || [];
    const ids = new Set();
    const byId = new Map();
    for (const span of spans) {
      if (span?.span_id) {
        byId.set(span.span_id, span);
      }
      const id = runtimeEntityId(span);
      if (id) {
        ids.add(id);
      }
    }
    const edges = [];
    for (const span of spans) {
      const parent = byId.get(span?.parent_span_id);
      if (!parent) {
        continue;
      }
      const source = runtimeEntityId(parent);
      const target = runtimeEntityId(span);
      if (!source || !target || source === target) {
        continue;
      }
      edges.push({
        id: `runtime-edge:${parent.span_id}->${span.span_id}`,
        source_id: source,
        target_id: target,
        kind: 'observed_span_parent',
        basis: 'runtime',
        resolution: 'observed_span_parent',
        evidence_ids: [],
        span_ids: [parent.span_id, span.span_id],
        parent_span_id: parent.span_id,
        child_span_id: span.span_id,
        child_name: span.name,
        child_start_ns: span.start_ns,
      });
    }
    this.runtimeFilter = {
      runId: String(runId ?? timeline?.run_id ?? 'unknown'),
      entityIds: [...ids].sort(),
      spans: spans.length,
      spanEdges: edges.length,
      edges,
    };
    return this.runtimeFilter;
  }

  clearRuntimeFilter() {
    this.runtimeFilter = null;
  }

  /** Bind a candidate architecture to a responsibility projection. */
  setArchitecture(architecture) {
    this.architecture =
      architecture && Array.isArray(architecture.components) ? architecture : null;
    this.componentByEntity.clear();
    this.componentPositions.clear();
    if (!this.architecture) {
      return null;
    }
    for (const component of this.architecture.components) {
      for (const entityId of component.member_entity_ids || []) {
        this.componentByEntity.set(entityId, component);
      }
    }
    return this.architecture;
  }

  membersOfModule(moduleId) {
    if (typeof moduleId !== 'string' || !moduleId.startsWith('module:')) {
      return [];
    }
    const name = moduleId.slice('module:'.length);
    return [...this.nodes.values()]
      .filter((node) => moduleNameFor(node.entity) === name)
      .map((node) => node.entity)
      .sort((left, right) => left.id.localeCompare(right.id));
  }

  membersOfComponent(componentId) {
    if (typeof componentId !== 'string' || !componentId.startsWith('component:')) {
      return [];
    }
    const id = componentId.slice('component:'.length);
    return [...this.nodes.values()]
      .filter((node) => this.componentByEntity.get(node.entity.id)?.id === id)
      .map((node) => node.entity)
      .sort((left, right) => left.id.localeCompare(right.id));
  }

  displayEdge(edgeId) {
    const relation = this.edges.get(edgeId);
    if (relation) {
      return { edge: relation, aggregate: false };
    }
    const projections = [];
    if (this.view === 'responsibility' && this.architecture) {
      projections.push(this.projectComponents());
    }
    projections.push(this.projectModules());
    for (const projection of projections) {
      const aggregate = projection.edges.find((edge) => edge.id === edgeId);
      if (aggregate) {
        return { edge: aggregate, aggregate: true };
      }
    }
    return null;
  }

  /** Same entity, different projection: no forced parent/child hierarchy. */
  visibleNodes() {
    if (this.view === 'module') {
      const projection = this.projectModules();
      return {
        nodes: projection.nodes,
        warning:
          'module view groups entities by the first path segment; it is a path-prefix projection, not an accepted architecture',
      };
    }
    if (this.view === 'responsibility' && this.architecture) {
      const projection = this.projectComponents();
      return {
        nodes: projection.nodes,
        warning: `responsibility view groups by candidate architecture ${this.architecture.id} (${this.architecture.status}); components are not an accepted architecture`,
      };
    }
    if (this.view === 'runtime' && this.runtimeFilter) {
      const allowed = new Set(this.runtimeFilter.entityIds);
      const nodes = [...this.nodes.values()]
        .filter((node) => allowed.has(node.entity.id))
        .map((node) => ({
          ...node,
          position: this.positionForNode(node.entity.id, node.position),
        }));
      if (nodes.length === 0) {
        return {
          nodes: [],
          warning: `runtime run ${this.runtimeFilter.runId} has no spans mapped to entities in this local graph`,
        };
      }
      return {
        nodes,
        warning: `runtime run ${this.runtimeFilter.runId}: ${nodes.length} mapped entities (${this.runtimeFilter.spans} spans, ${this.runtimeFilter.spanEdges} observed span-parent edges); span-parent edges are containment, not confirmed calls; other edges are static graph adjacency`,
      };
    }
    const wanted = {
      file: new Set(['file', 'symbol', 'type']),
      responsibility: new Set(['module', 'component', 'boundary', 'external']),
      runtime: new Set(['process', 'thread', 'task', 'queue', 'resource']),
    }[this.view];
    const nodes = [...this.nodes.values()]
      .filter((node) => wanted.has(node.entity.kind))
      .map((node) => ({
        ...node,
        position: this.positionForNode(node.entity.id, node.position),
      }));
    if (nodes.length === 0) {
      return {
        nodes: [...this.nodes.values()].map((node) => ({
          ...node,
          position: this.positionForNode(node.entity.id, node.position),
        })),
        warning: `view "${this.view}" has no matching entity kinds; showing the complete local graph`,
      };
    }
    return { nodes, warning: null };
  }

  visibleEdges(visibleIds) {
    const ids = new Set(visibleIds);
    const projection =
      this.view === 'module'
        ? this.projectModules()
        : this.view === 'responsibility' && this.architecture
          ? this.projectComponents()
          : null;
    if (projection) {
      return projection.edges
        .filter((edge) => ids.has(edge.source_id) && ids.has(edge.target_id))
        .map((edge) => ({ ...edge, style: relationStyle(edge, [...this.evidence.keys()]) }));
    }
    const staticEdges = [...this.edges.values()]
      .filter((edge) => ids.has(edge.source_id) && ids.has(edge.target_id))
      .map((edge) => ({ ...edge, style: relationStyle(edge, [...this.evidence.keys()]) }));
    if (this.view === 'runtime' && this.runtimeFilter) {
      const spanEdges = this.runtimeFilter.edges
        .filter((edge) => ids.has(edge.source_id) && ids.has(edge.target_id))
        .map((edge) => ({ ...edge, style: relationStyle(edge, [...this.evidence.keys()]) }));
      return [...staticEdges, ...spanEdges];
    }
    return staticEdges;
  }

  /**
   * Path-prefix module projection. Each aggregate edge keeps every underlying
   * relation id, kind and evidence id; it is rendered as a projection, never as
   * a confirmed dependency.
   */
  projectModules() {
    const groups = new Map();
    for (const node of this.nodes.values()) {
      const name = moduleNameFor(node.entity);
      const id = `module:${name}`;
      let group = groups.get(id);
      if (!group) {
        group = {
          id,
          name,
          members: [],
        };
        groups.set(id, group);
      }
      group.members.push(node.entity.id);
    }
    const aggregate = new Map();
    for (const edge of this.edges.values()) {
      const source = this.nodes.get(edge.source_id);
      const target = this.nodes.get(edge.target_id);
      if (!source || !target) {
        continue;
      }
      const sourceModule = `module:${moduleNameFor(source.entity)}`;
      const targetModule = `module:${moduleNameFor(target.entity)}`;
      if (sourceModule === targetModule) {
        continue;
      }
      const key = `${sourceModule}->${targetModule}`;
      let item = aggregate.get(key);
      if (!item) {
        item = {
          id: `module-edge:${key}`,
          source_id: sourceModule,
          target_id: targetModule,
          kind: 'aggregate',
          basis: 'projection',
          resolution: 'aggregate',
          evidence_ids: [],
          relation_ids: [],
          relation_kinds: [],
          relations: [],
          relations_truncated: false,
        };
        aggregate.set(key, item);
      }
      for (const evidenceId of edge.evidence_ids || []) {
        if (!item.evidence_ids.includes(evidenceId)) {
          item.evidence_ids.push(evidenceId);
        }
      }
      if (!item.relation_ids.includes(edge.id)) {
        item.relation_ids.push(edge.id);
      }
      if (!item.relation_kinds.includes(edge.kind)) {
        item.relation_kinds.push(edge.kind);
      }
      if (item.relations.length < 200) {
        item.relations.push({
          relation_id: edge.id,
          kind: edge.kind,
          basis: edge.basis,
          resolution: edge.resolution,
          source_id: edge.source_id,
          target_id: edge.target_id,
          evidence_ids: edge.evidence_ids || [],
        });
      } else {
        item.relations_truncated = true;
      }
    }
    const nodeEntities = [...groups.values()]
      .sort((left, right) => left.id.localeCompare(right.id))
      .map((group) => ({
        id: group.id,
        kind: 'module',
        qualified_name: group.name,
        projection: 'path_prefix',
        member_ids: group.members.slice().sort(),
        member_count: group.members.length,
      }));
    const edges = [...aggregate.values()].sort((left, right) => left.id.localeCompare(right.id));
    const positions = this.groupPositions_(this.modulePositions, nodeEntities, edges);
    return {
      nodes: nodeEntities.map((entity) => ({
        entity,
        position: this.positionForNode(entity.id, positions.get(entity.id)),
      })),
      edges,
    };
  }

  /**
   * Responsibility projection from the candidate architecture's real member
   * lists. Underlying graph relations and declared architecture dependencies
   * are both retained; neither is promoted to an accepted architecture.
   */
  projectComponents() {
    if (!this.architecture) {
      return { nodes: [], edges: [] };
    }
    const componentId = new Map();
    const entities = [];
    for (const component of this.architecture.components) {
      const members = (component.member_entity_ids || []).filter((id) => this.nodes.has(id));
      if (members.length === 0) {
        continue;
      }
      const id = `component:${component.id}`;
      componentId.set(component.id, id);
      entities.push({
        id,
        kind: 'component',
        qualified_name: component.name || component.id,
        projection: 'architecture_members',
        member_ids: members.slice().sort(),
        member_count: members.length,
        architecture_id: this.architecture.id,
        layer: this.architecture.layer,
        status: this.architecture.status,
      });
    }
    const aggregate = new Map();
    const add = (sourceId, targetId, relation, dependency) => {
      const key = `${sourceId}->${targetId}`;
      let item = aggregate.get(key);
      if (!item) {
        item = {
          id: `component-edge:${key}`,
          source_id: sourceId,
          target_id: targetId,
          kind: 'aggregate',
          basis: 'architecture_projection',
          resolution: 'aggregate',
          evidence_ids: [],
          relation_ids: [],
          relation_kinds: [],
          relations: [],
          architecture_dependencies: [],
          relations_truncated: false,
        };
        aggregate.set(key, item);
      }
      if (dependency) {
        for (const evidenceId of dependency.evidence_ids || []) {
          if (!item.evidence_ids.includes(evidenceId)) {
            item.evidence_ids.push(evidenceId);
          }
        }
        if (!item.relation_kinds.includes('architecture_dependency')) {
          item.relation_kinds.push('architecture_dependency');
        }
        item.architecture_dependencies.push({
          source_id: dependency.source_id,
          target_id: dependency.target_id,
          reason: dependency.reason,
          status: dependency.status,
          evidence_ids: dependency.evidence_ids || [],
        });
      }
      if (relation) {
        for (const evidenceId of relation.evidence_ids || []) {
          if (!item.evidence_ids.includes(evidenceId)) {
            item.evidence_ids.push(evidenceId);
          }
        }
        if (!item.relation_ids.includes(relation.id)) {
          item.relation_ids.push(relation.id);
        }
        if (!item.relation_kinds.includes(relation.kind)) {
          item.relation_kinds.push(relation.kind);
        }
        if (item.relations.length < 200) {
          item.relations.push({
            relation_id: relation.id,
            kind: relation.kind,
            basis: relation.basis,
            resolution: relation.resolution,
            source_id: relation.source_id,
            target_id: relation.target_id,
            evidence_ids: relation.evidence_ids || [],
          });
        } else {
          item.relations_truncated = true;
        }
      }
    };
    for (const relation of this.edges.values()) {
      const source = this.componentByEntity.get(relation.source_id);
      const target = this.componentByEntity.get(relation.target_id);
      if (!source || !target || source.id === target.id) {
        continue;
      }
      add(`component:${source.id}`, `component:${target.id}`, relation, null);
    }
    for (const dependency of this.architecture.dependencies || []) {
      const source = componentId.get(dependency.source_id);
      const target = componentId.get(dependency.target_id);
      if (!source || !target || source === target) {
        continue;
      }
      add(source, target, null, dependency);
    }
    const edges = [...aggregate.values()].sort((left, right) => left.id.localeCompare(right.id));
    for (const edge of edges) {
      edge.evidence_ids.sort();
      edge.relation_ids.sort();
      edge.relation_kinds.sort();
      edge.relations.sort((left, right) =>
        String(left.relation_id).localeCompare(String(right.relation_id)),
      );
    }
    const positions = this.groupPositions_(this.componentPositions, entities, edges);
    return {
      nodes: entities.map((entity) => ({
        entity,
        position: this.positionForNode(entity.id, positions.get(entity.id)),
      })),
      edges,
    };
  }

  groupPositions_(cache, entities, edges) {
    const depth = new Map(entities.map((entity) => [entity.id, 0]));
    const outgoing = new Map(entities.map((entity) => [entity.id, []]));
    const indegree = new Map(entities.map((entity) => [entity.id, 0]));
    for (const edge of edges) {
      if (!outgoing.has(edge.source_id) || !indegree.has(edge.target_id)) {
        continue;
      }
      outgoing.get(edge.source_id).push(edge.target_id);
      indegree.set(edge.target_id, indegree.get(edge.target_id) + 1);
    }
    const queue = entities
      .filter((entity) => indegree.get(entity.id) === 0)
      .map((entity) => entity.id)
      .sort();
    const seen = new Set(queue);
    let index = 0;
    while (index < queue.length) {
      const current = queue[index];
      index += 1;
      for (const target of outgoing.get(current) || []) {
        if (seen.has(target)) {
          continue;
        }
        seen.add(target);
        depth.set(target, depth.get(current) + 1);
        queue.push(target);
      }
    }
    const maxDepth = Math.max(0, ...depth.values());
    for (const entity of entities) {
      if (!seen.has(entity.id)) {
        depth.set(entity.id, maxDepth + 1);
      }
    }
    const positions = new Map();
    for (const entity of entities) {
      if (cache.has(entity.id)) {
        positions.set(entity.id, cache.get(entity.id));
        continue;
      }
      const x = 80 + depth.get(entity.id) * 220;
      let row = 0;
      while ([...cache.values()].some((position) => position.x === x && position.y === 80 + row * 80)) {
        row += 1;
      }
      const position = { x, y: 80 + row * 80 };
      cache.set(entity.id, position);
      positions.set(entity.id, position);
    }
    return positions;
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
