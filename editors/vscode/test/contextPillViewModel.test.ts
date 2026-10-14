import { test } from 'node:test';
import assert from 'node:assert';
import {
  assembleViewModel,
  groupOccurrencesByFile,
  mergeLiteralGroups,
  resolveSymbol,
  type PillGateway,
} from '../src/contextPill/viewModel';
import type { FindResponse, HealthResponse, RankedBody } from '../src/gateway';
import { buildAgentContextMarkdown } from '../src/contextPill/agentContext';

// These fakes return the REAL LSP wire shapes (verified against a live server):
//  - slice → { core: [{path,...}], deps: { data: [{path,...}] }, consumers, ... }
//  - impact → { total, direct: [{path, depth}], transitive: [...] }
//  - health → { health_score, status, dead_exports, cycles, twins, hotspots, ... }
//  - bodyRanked → { found, file, preview, truncated, candidates }
const fakeGateway: PillGateway = {
  lspState() {
    return { phase: 'running', message: 'Initialize handshake completed.' };
  },
  async slice() {
    return {
      core: [{ path: 'tb_core/dispatch.py', loc: 120, lang: 'python', depth: 0 }],
      deps: {
        data: [
          { path: 'tb_core/parse_claude.py', loc: 80, lang: 'python', depth: 1 },
          { path: 'tb_core/parse_codex.py', loc: 90, lang: 'python', depth: 1 },
          { path: 'tb_core/parse_gemini.py', loc: 70, lang: 'python', depth: 1 },
        ],
        chunk: 0,
        total_chunks: 1,
        next_cursor: null,
      },
      consumers: { data: [], chunk: 0, total_chunks: 1, next_cursor: null },
      total_files: 4,
      total_loc: 360,
    };
  },
  async impact() {
    return {
      total: 7,
      direct: [
        { path: 'cli.py', depth: 1 },
        { path: 'batch.py', depth: 1 },
        { path: 'main.py', depth: 1 },
      ],
      transitive: [{ path: 'app.py', depth: 2 }],
      blast_severity: 'medium',
    };
  },
  async health(): Promise<HealthResponse> {
    return {
      health_score: 92,
      status: 'green',
      cycles: 0,
      dead_exports: 0,
      twins: 0,
      hotspots: 1,
      snapshot_stale: false,
      snapshot_age_seconds: 5,
      top_risks: [
        { kind: 'hotspot', file: 'tb_core/dispatch.py', severity: 'high', message: 'high churn × fan-in' },
        { kind: 'dead_export', file: 'tb_core/parse_codex.py', severity: 'low', message: 'unused export foo' },
      ],
      recommended_actions: [],
    };
  },
  async bodyRanked(): Promise<RankedBody> {
    return {
      found: true,
      file: 'tb_core/dispatch.py',
      preview: 'def parse_jsonl_to_session_record(path):\n    ...',
      truncated: true,
      candidates: 1,
    };
  },
  async literal(): Promise<FindResponse> {
    return {
      query: 'TODO',
      total_matches: 4,
      literal_matches: {
        query: 'TODO',
        total: 4,
        files_matched: 2,
        source: 'literal',
        occurrences: [
          { file: 'a.py', line: 10, column: 1, matched_text: 'TODO', context: '', source: 'literal', occurrence_kind: 'comment' },
          { file: 'a.py', line: 3, column: 1, matched_text: 'TODO', context: '', source: 'literal', occurrence_kind: 'comment' },
          { file: 'b.py', line: 7, column: 1, matched_text: 'TODO', context: '', source: 'literal', occurrence_kind: 'comment' },
          { file: 'a.py', line: 10, column: 5, matched_text: 'TODO', context: '', source: 'literal', occurrence_kind: 'comment' },
        ],
        page: { offset: 0, limit: 50, returned: 4, has_more: true, next_offset: 50 },
      },
    };
  },
};

test('empty-snapshot health remains unknown through model and copied agent context', async () => {
  for (const score of [null, 0]) {
    const gateway: PillGateway = {
      ...fakeGateway,
      async health() {
        return { ...await fakeGateway.health(true), health_score: score, status: score === null ? 'unknown' : 'red' };
      },
    };
    const model = await assembleViewModel(gateway, { kind: 'file', value: 'tb_core/dispatch.py' });
    assert.strictEqual(model.health.score, score);
    assert.strictEqual(model.repoHealth, score);
    const markdown = buildAgentContextMarkdown(model, '');
    assert.ok(markdown.includes(`Repo health: ${score === null ? 'unknown' : '0/100'}`));
    assert.ok(!markdown.includes('null/100'));
  }
});

test('file scope maps real slice/impact/health shapes to the view model', async () => {
  const vm = await assembleViewModel(fakeGateway, { kind: 'file', value: 'tb_core/dispatch.py' });
  assert.strictEqual(vm.scope.kind, 'file');
  // file comes from slice.core[0].path
  assert.strictEqual(vm.file, 'tb_core/dispatch.py');
  // health.score from health_score
  assert.strictEqual(vm.health.score, 92);
  assert.strictEqual(vm.health.status, 'green');
  // blastRadius.count from impact.total
  assert.strictEqual(vm.blastRadius.count, 7);
  // blastRadius.direct is a string[] of impact.direct[].path
  assert.deepStrictEqual(vm.blastRadius.direct, ['cli.py', 'batch.py', 'main.py']);
  assert.ok(vm.blastRadius.direct.every((d) => typeof d === 'string'));
  // deps is a string[] of slice.deps.data[].path
  assert.deepStrictEqual(vm.deps, [
    'tb_core/parse_claude.py',
    'tb_core/parse_codex.py',
    'tb_core/parse_gemini.py',
  ]);
  assert.ok(vm.deps.every((d) => typeof d === 'string'));
  // findings.dead from dead_exports
  assert.strictEqual(vm.findings.dead, 0);
  assert.strictEqual(vm.findings.hotspots, 1);
  // fileLoc comes from slice.core[0].loc (file scope) — gates the LOC badge.
  assert.strictEqual(vm.fileLoc, 120);
  // repoHealth carries the demoted repo health score (no longer a header badge).
  assert.strictEqual(vm.repoHealth, 92);
  // repoFindings carries repo totals (the muted background line).
  assert.deepStrictEqual(vm.repoFindings, { dead: 0, cycles: 0, twins: 0, hotspots: 1 });
  // fileRisks filters top_risks down to the active file only.
  assert.deepStrictEqual(vm.fileRisks, [
    { kind: 'hotspot', severity: 'high', message: 'high churn × fan-in' },
  ]);
  // No per-file LSP source for exports/summary: documented empty degrade.
  assert.deepStrictEqual(vm.exports, []);
  assert.strictEqual(vm.summary, '');
  // file scope has no body preview
  assert.strictEqual(vm.bodyPreview, null);
  assert.strictEqual(vm.agentPackStatus, 'ready');
});

test('symbol scope produces a body preview from bodyRanked', async () => {
  const vm = await assembleViewModel(fakeGateway, { kind: 'symbol', value: 'parse_jsonl_to_session_record' });
  assert.ok(vm.bodyPreview);
  assert.strictEqual(vm.bodyPreview?.found, true);
  assert.strictEqual(vm.bodyPreview?.file, 'tb_core/dispatch.py');
  assert.strictEqual(vm.bodyPreview?.truncated, true);
  // shared sections still mapped
  assert.strictEqual(vm.blastRadius.count, 7);
  assert.strictEqual(vm.health.score, 92);
  // exports/summary degrade empty for symbol scope too
  assert.deepStrictEqual(vm.exports, []);
  assert.strictEqual(vm.summary, '');
  // symbol scope has no single file LOC — gates OFF the LOC badge.
  assert.strictEqual(vm.fileLoc, null);
  // fileRisks filter on the resolved body file (bodyPreview.file).
  assert.deepStrictEqual(vm.fileRisks, [
    { kind: 'hotspot', severity: 'high', message: 'high churn × fan-in' },
  ]);
  // repoFindings still carries repo totals on symbol scope.
  assert.deepStrictEqual(vm.repoFindings, { dead: 0, cycles: 0, twins: 0, hotspots: 1 });
});

test('symbol scope threads scope.file into bodyRanked (disambiguation hint)', async () => {
  // A gateway whose bodyRanked honours the fileHint (like the real rankBody):
  // prefer the body whose file matches the hint, else fall back to the first.
  const bodies = [
    { file: 'a.ts', preview: 'A', truncated: false },
    { file: 'b.ts', preview: 'B', truncated: false },
  ];
  let seenHint: string | undefined = 'UNSET';
  const hintGateway: PillGateway = {
    ...fakeGateway,
    async bodyRanked(_symbol: string, fileHint?: string): Promise<RankedBody> {
      seenHint = fileHint;
      const hit = (fileHint && bodies.find((b) => b.file === fileHint)) || bodies[0];
      return { found: true, file: hit.file, preview: hit.preview, truncated: false, candidates: bodies.length };
    },
  };
  // Without a hint → first body (bodies[0] = a.ts).
  const noHint = await assembleViewModel(hintGateway, { kind: 'symbol', value: 'foo' });
  assert.strictEqual(seenHint, undefined);
  assert.strictEqual(noHint.bodyPreview?.file, 'a.ts');
  // With scope.file = b.ts → the hint is forwarded and the matching body wins.
  const withHint = await assembleViewModel(hintGateway, { kind: 'symbol', value: 'foo', file: 'b.ts' });
  assert.strictEqual(seenHint, 'b.ts');
  assert.strictEqual(withHint.bodyPreview?.file, 'b.ts');
});

test('a failing section degrades gracefully, never throws', async () => {
  const sparse: PillGateway = {
    ...fakeGateway,
    async impact() { throw new Error('no impact'); },
  };
  const vm = await assembleViewModel(sparse, { kind: 'symbol', value: 'detect_agent' });
  assert.strictEqual(vm.blastRadius.count, 0);
  assert.deepStrictEqual(vm.blastRadius.direct, []);
  assert.strictEqual(vm.agentPackStatus, 'ready');
});

test('populated file scope is state:ready', async () => {
  const vm = await assembleViewModel(fakeGateway, { kind: 'file', value: 'tb_core/dispatch.py' });
  assert.strictEqual(vm.state, 'ready');
});

test('out-of-workspace scope short-circuits with ZERO gateway calls', async () => {
  // A spy gateway whose every method records a call AND throws — any touch is a
  // failure. The out-of-workspace VM must not leak repo health numbers.
  const calls: string[] = [];
  const spyGateway: PillGateway = {
    lspState() { return { phase: 'running', message: 'Initialize handshake completed.' }; },
    async slice() { calls.push('slice'); throw new Error('slice must not be called'); },
    async impact() { calls.push('impact'); throw new Error('impact must not be called'); },
    async health(): Promise<HealthResponse> { calls.push('health'); throw new Error('health must not be called'); },
    async bodyRanked(): Promise<RankedBody> { calls.push('bodyRanked'); throw new Error('bodyRanked must not be called'); },
    async literal(): Promise<FindResponse> { calls.push('literal'); throw new Error('literal must not be called'); },
  };
  const vm = await assembleViewModel(spyGateway, { kind: 'out-of-workspace', value: '/tmp/elsewhere/notes.md' });
  assert.deepStrictEqual(calls, []);
  assert.strictEqual(vm.state, 'out-of-workspace');
  assert.strictEqual(vm.file, '/tmp/elsewhere/notes.md');
  assert.strictEqual(vm.health.score, null);
  assert.strictEqual(vm.health.status, 'unknown');
  assert.strictEqual(vm.blastRadius.count, 0);
  assert.deepStrictEqual(vm.blastRadius.direct, []);
});

test('starting LSP returns an honest blocked pill without gateway calls', async () => {
  const calls: string[] = [];
  const startingGateway: PillGateway = {
    lspState() {
      return {
        phase: 'starting',
        message: 'Starting loctree-lsp and waiting for initialize handshake.',
        serverCommand: '/tmp/loctree-lsp',
      };
    },
    async slice() { calls.push('slice'); throw new Error('slice must not be called before handshake'); },
    async impact() { calls.push('impact'); throw new Error('impact must not be called before handshake'); },
    async health(): Promise<HealthResponse> { calls.push('health'); throw new Error('health must not be called before handshake'); },
    async bodyRanked(): Promise<RankedBody> { calls.push('bodyRanked'); throw new Error('bodyRanked must not be called before handshake'); },
    async literal(): Promise<FindResponse> { calls.push('literal'); throw new Error('literal must not be called before handshake'); },
  };

  const vm = await assembleViewModel(startingGateway, { kind: 'file', value: 'src/index.ts' });
  assert.deepStrictEqual(calls, []);
  assert.strictEqual(vm.state, 'lsp-starting');
  assert.strictEqual(vm.lsp.phase, 'starting');
  assert.strictEqual(vm.lsp.label, 'Starting');
  assert.match(vm.summary, /initialize handshake/);
  assert.strictEqual(vm.agentPackStatus, 'building');
});

test('startup failure returns an error pill with binary path and reason', async () => {
  const errorGateway: PillGateway = {
    lspState() {
      return {
        phase: 'error',
        message: 'Failed to start loctree-lsp at /missing/loctree-lsp: spawn ENOENT',
        serverCommand: '/missing/loctree-lsp',
        detail: 'exit code 127',
      };
    },
    async slice() { throw new Error('slice must not be called after startup failure'); },
    async impact() { throw new Error('impact must not be called after startup failure'); },
    async health(): Promise<HealthResponse> { throw new Error('health must not be called after startup failure'); },
    async bodyRanked(): Promise<RankedBody> { throw new Error('bodyRanked must not be called after startup failure'); },
    async literal(): Promise<FindResponse> { throw new Error('literal must not be called after startup failure'); },
  };

  const vm = await assembleViewModel(errorGateway, { kind: 'literal', value: 'TODO' });
  assert.strictEqual(vm.state, 'lsp-error');
  assert.strictEqual(vm.lsp.phase, 'error');
  assert.strictEqual(vm.lsp.serverCommand, '/missing/loctree-lsp');
  assert.match(vm.summary, /spawn ENOENT/);
  assert.strictEqual(vm.agentPackStatus, 'stale');
});

test('file scope with empty slice + empty impact (clean) is not-in-snapshot', async () => {
  const emptyGateway: PillGateway = {
    ...fakeGateway,
    async slice() { return { core: [], deps: { data: [], chunk: 0, total_chunks: 1, next_cursor: null } }; },
    async impact() { return { total: 0, direct: [], transitive: [] }; },
  };
  const vm = await assembleViewModel(emptyGateway, { kind: 'file', value: 'untracked/new_file.ts' });
  assert.strictEqual(vm.state, 'not-in-snapshot');
});

test('a -32001 snapshot-warming rejection on slice keeps state:ready', async () => {
  const warmingGateway: PillGateway = {
    ...fakeGateway,
    async slice() { throw { code: -32001, message: 'loctree snapshot not loaded yet' }; },
    async impact() { return { total: 0, direct: [], transitive: [] }; },
  };
  const vm = await assembleViewModel(warmingGateway, { kind: 'file', value: 'src/warming.ts' });
  assert.strictEqual(vm.state, 'ready');
});

// ── Task 6 — literal scope ─────────────────────────────────────────────────

test('literal scope assembles groups by file + total + hasMore', async () => {
  const vm = await assembleViewModel(fakeGateway, { kind: 'literal', value: 'TODO' });
  assert.strictEqual(vm.state, 'ready');
  assert.strictEqual(vm.literalTotal, 4);
  assert.strictEqual(vm.literalHasMore, true);
  assert.strictEqual(vm.literalNextOffset, 50);
  // grouped by file, lines deduped + sorted, file order = first appearance.
  assert.deepStrictEqual(vm.literalGroups, [
    { file: 'a.py', lines: [3, 10] },
    { file: 'b.py', lines: [7] },
  ]);
  // structural sections stay empty for a literal scope.
  assert.strictEqual(vm.blastRadius.count, 0);
  assert.deepStrictEqual(vm.exports, []);
  assert.strictEqual(vm.bodyPreview, null);
  assert.deepStrictEqual(vm.fileRisks, []);
});

test('non-literal scopes carry empty literal fields', async () => {
  const vm = await assembleViewModel(fakeGateway, { kind: 'file', value: 'tb_core/dispatch.py' });
  assert.deepStrictEqual(vm.literalGroups, []);
  assert.strictEqual(vm.literalTotal, 0);
  assert.strictEqual(vm.literalHasMore, false);
  assert.strictEqual(vm.literalNextOffset, null);
});

test('groupOccurrencesByFile dedups + sorts + preserves first-seen order', () => {
  const groups = groupOccurrencesByFile([
    { file: 'z.ts', line: 5 },
    { file: 'a.ts', line: 9 },
    { file: 'z.ts', line: 2 },
    { file: 'z.ts', line: 5 },
  ]);
  assert.deepStrictEqual(groups, [
    { file: 'z.ts', lines: [2, 5] },
    { file: 'a.ts', lines: [9] },
  ]);
});

test('mergeLiteralGroups appends new pages without duplicating (file, line)', () => {
  const existing = [{ file: 'a.ts', lines: [1, 4] }];
  const incoming = [
    { file: 'a.ts', lines: [4, 8] },
    { file: 'b.ts', lines: [2] },
  ];
  assert.deepStrictEqual(mergeLiteralGroups(existing, incoming), [
    { file: 'a.ts', lines: [1, 4, 8] },
    { file: 'b.ts', lines: [2] },
  ]);
});

// ── Task 5 — symbol resolution ─────────────────────────────────────────────

test('resolveSymbol classifies one / many / zero bodies', () => {
  assert.deepStrictEqual(resolveSymbol([]), { kind: 'zero', candidates: [] });
  assert.deepStrictEqual(
    resolveSymbol([{ symbol: 'foo', file: 'a.ts', start_line: 12 }]),
    { kind: 'one', candidates: [{ symbol: 'foo', file: 'a.ts', line: 12 }] },
  );
  const many = resolveSymbol([
    { symbol: 'foo', file: 'a.ts', start_line: 12 },
    { symbol: 'foo', file: 'b.ts', start_line: 30 },
  ]);
  assert.strictEqual(many.kind, 'many');
  assert.deepStrictEqual(many.candidates, [
    { symbol: 'foo', file: 'a.ts', line: 12 },
    { symbol: 'foo', file: 'b.ts', line: 30 },
  ]);
});

test('resolveSymbol tolerates undefined bodies', () => {
  assert.deepStrictEqual(resolveSymbol(undefined), { kind: 'zero', candidates: [] });
});
