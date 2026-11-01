// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

'use strict';

/** Loopback-only client for the ARCHMELD local API. */
class ApiError extends Error {
  constructor(status, code, message, body) {
    super(`${code}: ${message}`);
    this.status = status;
    this.code = code;
    this.body = body;
  }
}

class ArchmeldClient {
  constructor({ endpoint, token, fetchImpl }) {
    if (!/^http:\/\/(127\.0\.0\.1|localhost|\[::1\]):\d+$/.test(endpoint || '')) {
      throw new Error(`endpoint must be loopback http, got ${endpoint}`);
    }
    this.endpoint = endpoint.replace(/\/$/, '');
    this.token = token || '';
    this.fetch = fetchImpl || globalThis.fetch;
  }

  async request(method, path, body) {
    const headers = { 'content-type': 'application/json' };
    if (this.token) {
      headers.authorization = `Bearer ${this.token}`;
    }
    const response = await this.fetch(`${this.endpoint}${path}`, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = await response.text();
    let parsed;
    try {
      parsed = text ? JSON.parse(text) : {};
    } catch {
      parsed = { raw: text };
    }
    if (!response.ok) {
      throw new ApiError(
        response.status,
        parsed.code || 'internal_error',
        parsed.message || `HTTP ${response.status}`,
        parsed,
      );
    }
    return parsed;
  }

  health() {
    return this.request('GET', '/v1/health');
  }

  query(request) {
    return this.request('POST', '/v1/query', request);
  }

  source(request) {
    return this.request('POST', '/v1/source', request);
  }

  sourceFile(request) {
    return this.request('POST', '/v1/source/file', request);
  }

  report(projectId, snapshotId, outputName, includeSource = false, includeRawTraces = false) {
    return this.request('POST', '/v1/reports', {
      project_id: projectId,
      snapshot_id: snapshotId,
      output_name: outputName,
      include_source: includeSource,
      include_raw_traces: includeRawTraces,
    });
  }

  runtime(projectId, runId, limit = 1000) {
    return this.request(
      'GET',
      `/v1/runtime/${encodeURIComponent(runId)}?project_id=${encodeURIComponent(projectId)}&limit=${encodeURIComponent(limit)}`,
    );
  }

  events(projectId, afterSequence) {
    const suffix =
      afterSequence === undefined ? '' : `&after_sequence=${encodeURIComponent(afterSequence)}`;
    return this.request('GET', `/v1/events?project_id=${encodeURIComponent(projectId)}${suffix}`);
  }

  claim(projectId, claimId) {
    return this.request(
      'GET',
      `/v1/claims/${encodeURIComponent(claimId)}?project_id=${encodeURIComponent(projectId)}`,
    );
  }

  architecture(projectId, architectureId) {
    return this.request(
      'GET',
      `/v1/architectures/${encodeURIComponent(architectureId)}?project_id=${encodeURIComponent(projectId)}`,
    );
  }

  architectureReview(projectId, architectureId) {
    return this.request(
      'GET',
      `/v1/architectures/${encodeURIComponent(architectureId)}/review?project_id=${encodeURIComponent(projectId)}`,
    );
  }

  architectureCheck(projectId, architectureId, ruleIds = null) {
    return this.request(
      'POST',
      `/v1/architectures/${encodeURIComponent(architectureId)}/check`,
      { project_id: projectId, ...(ruleIds ? { rule_ids: ruleIds } : {}) },
    );
  }

  architectureDiff(projectId, architectureId, target) {
    return this.request(
      'POST',
      `/v1/architectures/${encodeURIComponent(architectureId)}/diff`,
      { project_id: projectId, target },
    );
  }

  changeset(projectId, changesetId) {
    return this.request(
      'GET',
      `/v1/changesets/${encodeURIComponent(changesetId)}?project_id=${encodeURIComponent(projectId)}`,
    );
  }

  changesetDiff(projectId, changesetId, maxBytes = 65536) {
    return this.request(
      'GET',
      `/v1/changesets/${encodeURIComponent(changesetId)}/diff?project_id=${encodeURIComponent(projectId)}&max_bytes=${encodeURIComponent(maxBytes)}`,
    );
  }

  approveChangeset(projectId, changesetId, { expectedRevision, preparedDigest, actor, idempotencyKey }) {
    return this.request('POST', `/v1/changesets/${encodeURIComponent(changesetId)}/approve`, {
      project_id: projectId,
      expected_revision: expectedRevision,
      prepared_digest: preparedDigest,
      actor,
      idempotency_key: idempotencyKey,
    });
  }

  applyChangeset(
    projectId,
    changesetId,
    { expectedRevision, preparedDigest, approvalId, actor, workspaceHandle, idempotencyKey },
  ) {
    return this.request('POST', `/v1/changesets/${encodeURIComponent(changesetId)}/apply`, {
      project_id: projectId,
      expected_revision: expectedRevision,
      prepared_digest: preparedDigest,
      approval_id: approvalId,
      actor,
      workspace_handle: workspaceHandle,
      idempotency_key: idempotencyKey,
    });
  }

  recoverChangeset(projectId, changesetId, { expectedRevision, actor, workspaceHandle, idempotencyKey }) {
    return this.request('POST', `/v1/changesets/${encodeURIComponent(changesetId)}/recover`, {
      project_id: projectId,
      expected_revision: expectedRevision,
      actor,
      workspace_handle: workspaceHandle,
      idempotency_key: idempotencyKey,
    });
  }

  verifications(projectId, changesetId) {
    return this.request(
      'GET',
      `/v1/verifications?project_id=${encodeURIComponent(projectId)}&changeset_id=${encodeURIComponent(changesetId)}`,
    );
  }

  verification(projectId, verificationId) {
    return this.request(
      'GET',
      `/v1/verifications/${encodeURIComponent(verificationId)}?project_id=${encodeURIComponent(projectId)}`,
    );
  }

  verifyChangeset(
    projectId,
    changesetId,
    { expectedRevision, workspaceHandle, grantId, reason, idempotencyKey },
  ) {
    return this.request('POST', `/v1/changesets/${encodeURIComponent(changesetId)}/verify`, {
      project_id: projectId,
      expected_revision: expectedRevision,
      workspace_handle: workspaceHandle,
      execution_grant: { grant_id: grantId, reason },
      idempotency_key: idempotencyKey,
    });
  }

  acceptChangeset(
    projectId,
    changesetId,
    { expectedRevision, verificationId, workspaceHandle, actor, idempotencyKey },
  ) {
    return this.request('POST', `/v1/changesets/${encodeURIComponent(changesetId)}/accept`, {
      project_id: projectId,
      expected_revision: expectedRevision,
      verification_id: verificationId,
      workspace_handle: workspaceHandle,
      actor,
      idempotency_key: idempotencyKey,
    });
  }

  capabilities() {
    return this.request('GET', '/v1/capabilities');
  }

  cancelJob(projectId, jobId, { expectedRevision, idempotencyKey, approvalId = null }) {
    return this.request('POST', `/v1/jobs/${encodeURIComponent(jobId)}/cancel`, {
      project_id: projectId,
      expected_revision: expectedRevision,
      approval_id: approvalId,
      idempotency_key: idempotencyKey,
    });
  }
}

module.exports = { ArchmeldClient, ApiError };
