import type { Scope } from './scope';
import {
  DEFAULT_LSP_RUNTIME_STATE,
  isSnapshotNotLoaded,
  lspStateLabel,
  type FindResponse,
  type HealthResponse,
  type LspRuntimePhase,
  type LspRuntimeState,
  type RankedBody,
} from '../gateway';

export interface ExportSym { name: string; kind: string }
export interface BodyPreview { found: boolean; file: string; preview: string; truncated: boolean }
export interface BlastRadius { count: number; direct: string[] }
export interface PillHealth { score: number | null; status: string }
export interface PillFindings { dead: number; cycles: number; twins: number; hotspots: number }
/** One finding scoped to the active file (filtered out of repo `top_risks`). */
export interface FileRisk { kind: string; severity: string; message: string }
/** Literal occurrences for one file (grouped, sorted ascending lines). */
export interface LiteralGroup { file: string; lines: number[] }

/**
 * Render state for the pill. `ready` is the normal full surface. The empty
 * states replace the misleading all-zeros pill for files the snapshot can't
 * speak to: `out-of-workspace` (file outside any open Loctree project) and
 * `not-in-snapshot` (in-workspace, but the current snapshot has no record yet).
 */
export type PillState =
  | 'ready'
  | 'out-of-workspace'
  | 'not-in-snapshot'
  | 'no-editor'
  | 'lsp-starting'
  | 'lsp-stopped'
  | 'lsp-error';

export interface PillLspState {
  phase: LspRuntimePhase;
  label: string;
  message: string;
  serverCommand?: string;
  detail?: string;
}

export interface ContextPillViewModel {
  scope: Scope;
  file: string;
  state: PillState;
  lsp: PillLspState;
  summary: string;
  health: PillHealth;
  /** Repo-level health score (`health.health_score`), 0–100. Demoted to a
   *  muted background line in the header — no longer a header badge. */
  repoHealth: number | null;
  /** Lines of code of the ACTIVE file. `slice.core[0].loc` for file scope;
   *  `null` for symbol/literal scope and out-of-workspace (no single file). */
  fileLoc: number | null;
  blastRadius: BlastRadius;
  exports: ExportSym[];
  deps: string[];
  bodyPreview: BodyPreview | null;
  /** Repo totals (`findings` kept as an alias so agentContext stays valid). */
  findings: PillFindings;
  /** Repo totals, surfaced as muted background under per-file findings. */
  repoFindings: PillFindings;
  /** Findings whose `file` matches the active file (the honest, scoped set). */
  fileRisks: FileRisk[];
  agentPackStatus: 'ready' | 'building' | 'stale';
  /** Literal occurrences grouped by file. Populated ONLY for literal scope;
   *  empty array otherwise (gated so non-literal pills never render it). */
  literalGroups: LiteralGroup[];
  /** Total literal occurrences across the searched scope (literal scope only). */
  literalTotal: number;
  /** True when more literal occurrence pages exist (literal scope only). */
  literalHasMore: boolean;
  /** Offset to request for the next literal page; null when no more / N/A. */
  literalNextOffset: number | null;
}

/**
 * Real wire shapes of the LSP responses the pill consumes (verified against a
 * live server). Typed inline here so `tsc` enforces the adapter contract — the
 * old `Promise<any>` gateway hid the fact that the adapter was reading fields
 * the server never sends.
 */

/** One node in a `loctree/slice` bucket (core / deps.data / consumers.data). */
export interface SliceNode {
  path: string;
  loc?: number;
  lang?: string;
  depth?: number;
}

/** A paginated bucket of slice nodes (deps / consumers). */
export interface SliceBucket {
  data?: SliceNode[];
  chunk?: number;
  total_chunks?: number;
  next_cursor?: string | null;
}

/** `loctree/slice` response. */
export interface SliceResult {
  core?: SliceNode[];
  deps?: SliceBucket;
  consumers?: SliceBucket;
  identity?: unknown;
  total_files?: number;
  total_loc?: number;
}

/** One node in an `loctree/impact` bucket (direct / transitive). */
export interface ImpactNode {
  path: string;
  depth?: number;
}

/** `loctree/impact` response. */
export interface ImpactResult {
  total?: number;
  direct?: ImpactNode[];
  transitive?: ImpactNode[];
  blast_severity?: string;
  warnings?: unknown;
}

/**
 * The slice of the real `LoctreeGateway` the pill needs. `LoctreeGateway`
 * structurally satisfies this (slice/impact return `Promise<unknown>`, which we
 * narrow via `as` against the verified shapes above). Health and bodyRanked are
 * already typed by the gateway.
 */
export interface PillGateway {
  lspState(): LspRuntimeState;
  slice(target: string, consumers?: boolean): Promise<unknown>;
  impact(target: string, transitive?: boolean): Promise<unknown>;
  health(includeTopRisks?: boolean): Promise<HealthResponse>;
  bodyRanked(symbol: string, fileHint?: string): Promise<RankedBody>;
  /** Exact-identifier occurrence scan (`loctree/find` mode:literal). The pill
   *  uses it for literal-scope assembly + Load-more paging via `offset`. */
  literal(
    query: string,
    options?: { group_by_file?: boolean; offset?: number; limit?: number; count_only?: boolean },
  ): Promise<FindResponse>;
}

/** One occurrence row the grouping helpers consume (file + 1-based line). */
export interface OccurrenceLike { file: string; line: number }

/**
 * Group flat occurrences by file into `{file, lines}` rows. Lines within a file
 * are de-duplicated and sorted ascending; file order follows first appearance.
 * Pure — exported for unit tests.
 */
export function groupOccurrencesByFile(occurrences: OccurrenceLike[]): LiteralGroup[] {
  const order: string[] = [];
  const byFile = new Map<string, Set<number>>();
  for (const o of occurrences ?? []) {
    if (!o || typeof o.file !== 'string') continue;
    let set = byFile.get(o.file);
    if (!set) {
      set = new Set<number>();
      byFile.set(o.file, set);
      order.push(o.file);
    }
    if (typeof o.line === 'number') set.add(o.line);
  }
  return order.map((file) => ({
    file,
    lines: Array.from(byFile.get(file) ?? []).sort((a, b) => a - b),
  }));
}

/**
 * Merge an incoming page of literal groups into the existing groups, de-duping
 * on (file, line). Existing file order is preserved; new files append. Pure —
 * exported for unit tests (the Load-more append path).
 */
export function mergeLiteralGroups(existing: LiteralGroup[], incoming: LiteralGroup[]): LiteralGroup[] {
  const order: string[] = [];
  const byFile = new Map<string, Set<number>>();
  const absorb = (groups: LiteralGroup[]): void => {
    for (const g of groups ?? []) {
      if (!g || typeof g.file !== 'string') continue;
      let set = byFile.get(g.file);
      if (!set) {
        set = new Set<number>();
        byFile.set(g.file, set);
        order.push(g.file);
      }
      for (const line of g.lines ?? []) {
        if (typeof line === 'number') set.add(line);
      }
    }
  };
  absorb(existing);
  absorb(incoming);
  return order.map((file) => ({
    file,
    lines: Array.from(byFile.get(file) ?? []).sort((a, b) => a - b),
  }));
}

/** Accumulated literal occurrences across every loaded page (panel-side harvest
 *  state for a single literal scope). */
export interface LiteralHarvest {
  query: string;
  groups: LiteralGroup[];
  total: number;
  hasMore: boolean;
}

/**
 * Overlay an accumulated literal harvest onto a freshly-assembled VM so a Copy
 * Agent Context exports EVERY page the user loaded via Load-more, not just the
 * first page `assembleViewModel` re-fetches. No-op (returns the VM unchanged)
 * unless the VM is a literal scope whose value matches the harvest query. Pure —
 * exported for tests.
 */
export function applyLiteralHarvest(
  vm: ContextPillViewModel,
  harvest: LiteralHarvest | null,
): ContextPillViewModel {
  if (!harvest || vm.scope.kind !== 'literal' || vm.scope.value !== harvest.query) {
    return vm;
  }
  return {
    ...vm,
    literalGroups: harvest.groups,
    literalTotal: harvest.total,
    literalHasMore: harvest.hasMore,
  };
}

async function safe<T>(p: Promise<T>, fallback: T): Promise<T> {
  try { return await p; } catch { return fallback; }
}

/**
 * Like {@link safe}, but distinguishes a -32001 "snapshot not loaded yet"
 * rejection (warming) from any other failure or a clean empty result. The
 * adapter needs this so a warming snapshot is never mistaken for
 * `not-in-snapshot`: on `notLoaded` we keep `state:'ready'` with empty sections.
 */
async function tryCall<T>(p: Promise<T>, fallback: T): Promise<{ value: T; notLoaded: boolean }> {
  try {
    return { value: await p, notLoaded: false };
  } catch (err) {
    return { value: fallback, notLoaded: isSnapshotNotLoaded(err) };
  }
}

/** One scope-switcher candidate row (symbol defined in `file` at `line`). */
export interface SymbolCandidate { symbol: string; file: string; line: number }

/**
 * Classify a `body()` result for the scope switcher. Pure — exported for tests.
 * `'one'` → rescope directly; `'many'` → render inline candidate list;
 * `'zero'` → probe literal occurrences and offer "show as a pill?".
 */
export function resolveSymbol(
  bodies: Array<{ symbol: string; file: string; start_line: number }> | undefined,
): { kind: 'one' | 'many' | 'zero'; candidates: SymbolCandidate[] } {
  const list = (bodies ?? []).map((b) => ({ symbol: b.symbol, file: b.file, line: b.start_line }));
  if (list.length === 0) return { kind: 'zero', candidates: [] };
  if (list.length === 1) return { kind: 'one', candidates: list };
  return { kind: 'many', candidates: list };
}

/** Module-private page-size value (see TDZ note in `assembleViewModel`). */
const LITERAL_PAGE_LIMIT_VALUE = 50;

/** First-page occurrence count for the literal pill (panel paginates further). */
export const LITERAL_FIRST_PAGE_LIMIT = LITERAL_PAGE_LIMIT_VALUE;

/** Default literal fields — non-literal scopes never render the occurrence list. */
const NO_LITERAL = {
  literalGroups: [] as LiteralGroup[],
  literalTotal: 0,
  literalHasMore: false,
  literalNextOffset: null as number | null,
};

function pillLspState(state: LspRuntimeState = DEFAULT_LSP_RUNTIME_STATE): PillLspState {
  return {
    phase: state.phase,
    label: lspStateLabel(state),
    message: state.message,
    serverCommand: state.serverCommand,
    detail: state.detail,
  };
}

function emptyMetrics() {
  return {
    health: { score: null, status: 'unknown' },
    repoHealth: null,
    blastRadius: { count: 0, direct: [] },
    exports: [] as ExportSym[],
    deps: [] as string[],
    bodyPreview: null,
    findings: { dead: 0, cycles: 0, twins: 0, hotspots: 0 },
    repoFindings: { dead: 0, cycles: 0, twins: 0, hotspots: 0 },
    fileRisks: [] as FileRisk[],
  };
}

function lspUnavailableState(phase: LspRuntimePhase): PillState {
  if (phase === 'error') return 'lsp-error';
  if (phase === 'starting') return 'lsp-starting';
  return 'lsp-stopped';
}

function lspUnavailableViewModel(scope: Scope, lsp: PillLspState): ContextPillViewModel {
  return {
    scope,
    file: scope.value,
    state: lspUnavailableState(lsp.phase),
    lsp,
    summary: lsp.message,
    ...emptyMetrics(),
    fileLoc: null,
    agentPackStatus: lsp.phase === 'starting' ? 'building' : 'stale',
    ...NO_LITERAL,
  };
}

/**
 * Minimal VM for the "no active editor" surface — a synthetic empty-state card
 * (heading + body + scope switcher) so the pill is never a blank `#pill` with no
 * escape hatch. No gateway calls: the scope is a placeholder literal of '' (the
 * webview renders by `state`, not scope, for this card).
 */
export function noEditorViewModel(lspState: LspRuntimeState = DEFAULT_LSP_RUNTIME_STATE): ContextPillViewModel {
  return {
    scope: { kind: 'literal', value: '' },
    file: '',
    state: 'no-editor',
    lsp: pillLspState(lspState),
    summary: '',
    ...emptyMetrics(),
    fileLoc: null,
    agentPackStatus: 'ready',
    ...NO_LITERAL,
  };
}

export async function assembleViewModel(gw: PillGateway, scope: Scope): Promise<ContextPillViewModel> {
  const target = scope.value;
  const lsp = pillLspState(gw.lspState());

  if (lsp.phase !== 'running' && scope.kind !== 'out-of-workspace') {
    return lspUnavailableViewModel(scope, lsp);
  }

  // Out-of-workspace files get a dedicated empty-state VM with ZERO gateway
  // calls — critically, no health() call, so no repo-level numbers leak into a
  // surface that has nothing to do with the open project.
  if (scope.kind === 'out-of-workspace') {
    return {
      scope,
      file: scope.value,
      state: 'out-of-workspace',
      lsp,
      summary: '',
      ...emptyMetrics(),
      fileLoc: null,
      agentPackStatus: 'ready',
      ...NO_LITERAL,
    };
  }

  // Literal scope: occurrence-list pill (Task 6). It does NOT use the shared
  // impact/health sections (blast/findings/exports/body all stay empty) — a
  // literal is a string match, not a structural target. Assemble from the first
  // page of `literal_matches`, grouped by file.
  if (scope.kind === 'literal') {
    // NOTE: reference the module-private value, NOT the exported binding — the
    // exported `LITERAL_FIRST_PAGE_LIMIT` compiles to `exports.LITERAL_...`,
    // which would collide with the local `const exports` declared below (TDZ).
    const litCall = await tryCall(
      gw.literal(target, { group_by_file: true, offset: 0, limit: LITERAL_PAGE_LIMIT_VALUE }),
      {} as FindResponse,
    );
    const matches = litCall.value.literal_matches;
    const groups = groupOccurrencesByFile(
      (matches?.occurrences ?? []).map((o) => ({ file: o.file, line: o.line })),
    );
    const page = matches?.page;
    const hasMore = !!page?.has_more;
    return {
      scope,
      file: target,
      state: 'ready',
      lsp,
      summary: '',
      ...emptyMetrics(),
      fileLoc: null,
      agentPackStatus: 'ready',
      literalGroups: groups,
      literalTotal: matches?.total ?? 0,
      literalHasMore: hasMore,
      literalNextOffset: hasMore ? page?.next_offset ?? null : null,
    };
  }

  // Shared sections: impact (blast radius) + health (repo findings). Both apply
  // to file and symbol scopes; the target is the path or the symbol name.
  const impactCall = await tryCall(gw.impact(target, true) as Promise<ImpactResult>, {} as ImpactResult);
  const im = impactCall.value;
  const blastRadius: BlastRadius = {
    count: im.total ?? 0,
    direct: (im.direct ?? []).map((d) => d.path),
  };

  const h = await safe<Partial<HealthResponse>>(gw.health(true), {});
  const health: PillHealth = { score: h.health_score ?? null, status: h.status ?? 'unknown' };
  const repoHealth = h.health_score ?? null;
  // Repo totals. `findings` is kept as the same object (agentContext reads it);
  // `repoFindings` is the webview-facing alias for the muted background line.
  const repoFindings: PillFindings = {
    dead: h.dead_exports ?? 0,
    cycles: h.cycles ?? 0,
    twins: h.twins ?? 0,
    hotspots: h.hotspots ?? 0,
  };
  const findings = repoFindings;
  const allRisks = h.top_risks ?? [];

  // Per-file findings: filter the repo `top_risks` down to the active file.
  // activeFile = the file path (file scope) or the resolved body file / target
  // (symbol scope). Computed per branch below; `risksFor` does the filter.
  const risksFor = (activeFile: string): FileRisk[] =>
    allRisks
      .filter((r) => r.file === activeFile)
      .map((r) => ({ kind: r.kind, severity: r.severity, message: r.message }));

  // No per-file LSP source exists for a file's defined symbols (`exports`) or a
  // per-file `summary`: `find` is query-driven symbol search, not a file symbol
  // listing, and `contextPack` is repo-level markdown. So both degrade to empty
  // rather than being faked. Tracked as future work (needs a server surface).
  const exports: ExportSym[] = [];
  const summary = '';

  if (scope.kind === 'file') {
    const sliceCall = await tryCall(gw.slice(target, true) as Promise<SliceResult>, {} as SliceResult);
    const sl = sliceCall.value;
    const deps = (sl.deps?.data ?? []).map((d) => d.path);
    const file = sl.core?.[0]?.path ?? target;
    const fileLoc = sl.core?.[0]?.loc ?? null;

    // Decide state. A truly empty slice+impact means the snapshot has no record
    // of this file → 'not-in-snapshot'. But if EITHER call rejected with a
    // -32001 (snapshot still warming), we can't conclude that — keep 'ready'
    // with empty sections and let a later rescope fill them in.
    const sliceEmpty = (sl.core?.length ?? 0) === 0;
    const impactEmpty = !im.total && (im.direct?.length ?? 0) === 0;
    const warming = sliceCall.notLoaded || impactCall.notLoaded;
    const state: PillState =
      !warming && sliceEmpty && impactEmpty ? 'not-in-snapshot' : 'ready';

    return {
      scope,
      file,
      state,
      lsp,
      summary,
      health,
      repoHealth,
      fileLoc,
      blastRadius,
      exports,
      deps,
      bodyPreview: null,
      findings,
      repoFindings,
      fileRisks: risksFor(file),
      agentPackStatus: 'ready',
      ...NO_LITERAL,
    };
  }

  if (scope.kind === 'symbol') {
    // Pass the file hint (set when a disambiguation candidate was clicked) so
    // rankBody prefers the chosen definition instead of falling back to bodies[0].
    const body = await safe(gw.bodyRanked(target, scope.file), null);
    const bodyPreview: BodyPreview | null = body?.found
      ? { found: true, file: body.file, preview: body.preview, truncated: body.truncated }
      : null;
    const activeFile = bodyPreview?.file ?? target;
    return {
      scope,
      file: activeFile,
      state: 'ready',
      lsp,
      summary,
      health,
      repoHealth,
      fileLoc: null,
      blastRadius,
      exports,
      deps: [],
      bodyPreview,
      findings,
      repoFindings,
      fileRisks: risksFor(activeFile),
      agentPackStatus: 'ready',
      ...NO_LITERAL,
    };
  }

  // Every Scope kind is handled above (out-of-workspace, literal, file,
  // symbol). Guard exhaustiveness so a future scope kind must add its own
  // branch here instead of silently degrading.
  return assertNeverScope(scope);
}

/** Exhaustiveness guard for the {@link Scope} union (unreachable at runtime). */
function assertNeverScope(scope: never): never {
  throw new Error(`assembleViewModel: unhandled scope ${JSON.stringify(scope)}`);
}
