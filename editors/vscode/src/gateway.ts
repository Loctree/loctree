/**
 * Loctree LSP Gateway
 *
 * Typed facade over the loctree-lsp custom requests. This is the plugin's
 * single source of truth for analysis data — it replaces the legacy habit of
 * reading `.loctree/*.json` off disk, which loctree 0.12 no longer writes to
 * the workspace (artifacts live in the per-project cache dir; the LSP loads
 * the snapshot from there regardless). Mirrors the JetBrains `LoctreeLspGateway`.
 *
 * Custom methods registered in loctree-lsp/src/lib.rs:
 *   loctree/health, loctree/find, loctree/follow, loctree/impact,
 *   loctree/slice, loctree/body, loctree/contextAtlas
 *
 * 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Vetcoders
 */

import { LanguageClient, Range } from 'vscode-languageclient/node';

export type LspRuntimePhase = 'stopped' | 'starting' | 'running' | 'error';

export interface LspRuntimeState {
    phase: LspRuntimePhase;
    message: string;
    serverCommand?: string;
    serverVersion?: string;
    resolutionSource?: string;
    runtimeWarning?: string;
    detail?: string;
    exitCode?: number | null;
}

export const DEFAULT_LSP_RUNTIME_STATE: LspRuntimeState = {
    phase: 'stopped',
    message: 'Loctree LSP has not started.',
};

export function lspStateLabel(state: LspRuntimeState): string {
    switch (state.phase) {
        case 'starting':
            return 'Starting';
        case 'running':
            return 'Running';
        case 'error':
            return 'Error';
        case 'stopped':
            return 'Stopped';
    }
}

function stateFromClient(client: LanguageClient | undefined): LspRuntimeState {
    if (client?.isRunning()) {
        return {
            phase: 'running',
            message: 'Initialize handshake completed.',
        };
    }
    return DEFAULT_LSP_RUNTIME_STATE;
}

function notRunningMessage(state: LspRuntimeState): string {
    const suffix = state.detail ? ` ${state.detail}` : '';
    if (state.phase === 'error') {
        return `Loctree LSP failed: ${state.message}${suffix}`;
    }
    return `Loctree LSP is ${lspStateLabel(state).toLowerCase()}: ${state.message}${suffix}`;
}

/** Pagination wrapper used by find/follow/slice match buckets. */
export interface Paginated<T> {
    chunk: number;
    total_chunks: number;
    next_cursor?: string | null;
    data: T;
    advisory?: string;
}

/** `loctree/health` response. The status bar + tree source counts from here. */
export interface HealthResponse {
    health_score: number | null;
    status: string; // green | yellow | red | unknown
    cycles: number;
    dead_exports: number;
    twins: number;
    hotspots: number;
    snapshot_stale: boolean;
    snapshot_age_seconds: number;
    top_risks: RiskItem[];
    recommended_actions: string[];
}

export function formatHealthScore(score: number | null): string {
    return score == null ? 'unknown' : `${score}/100`;
}

export interface RiskItem {
    kind: string; // cycle | dead_export | twin | hotspot
    file: string;
    severity: string; // high | medium | low
    message: string;
}

/** One exact-identifier occurrence (literal mode / `loct occurrences`). */
export interface Occurrence {
    file: string;
    line: number;
    column: number;
    matched_text: string;
    context: string;
    source: string; // "literal" | "fuzzy"
    occurrence_kind: string; // identifier | string | comment | ...
}

/** Per-file occurrence rollup attached when `group_by_file: true`. */
export interface ByFileCount {
    file: string;
    count: number;
}

/**
 * Pagination metadata for a literal occurrence page. Present when the LSP
 * applies `offset`/`limit` to the occurrence list (`apply_report`).
 */
export interface OccurrencePage {
    offset: number;
    limit?: number | null;
    returned: number;
    has_more: boolean;
    next_offset?: number | null;
}

export interface OccurrenceResults {
    query: string;
    total: number;
    files_matched: number;
    source: string;
    occurrences: Occurrence[];
    /** Present when `group_by_file: true` was requested. */
    by_file?: ByFileCount[];
    /** Present when the occurrence list was paged (`offset`/`limit`). */
    page?: OccurrencePage;
}

export interface FuzzySuggestion {
    file: string;
    /** Optional: the Rust producer skips this field when absent (None). */
    line?: number;
    score: number;
    source: string;
    [key: string]: unknown;
}

/** Subset of `loctree/find` response the plugin consumes. */
export interface FindResponse {
    query: string;
    total_matches: number;
    dead_status?: {
        is_exported: boolean;
        is_dead: boolean;
        dead_in_files: string[];
    };
    /** Present only for mode: "literal". */
    literal_matches?: OccurrenceResults;
    literal_fuzzy?: FuzzySuggestion[];
    [key: string]: unknown;
}

/** `loctree/follow` response — items.data is a per-scope summary object. */
export interface FollowResponse {
    scope: string;
    items: Paginated<Record<string, unknown>>;
    summary?: {
        count: number;
        severity: string;
        message?: string | null;
    };
    [key: string]: unknown;
}

/** `loctree/body` response — mirrors `loct body --json`. */
export interface SymbolBody {
    symbol: string;
    file: string;
    start_line: number;
    end_line: number;
    language: string;
    source: string;
    truncated: boolean;
    total_lines: number;
    line_cap: number;
}

export interface BodyResponse {
    symbol: string;
    bodies: SymbolBody[];
}

export interface RankedBody {
    found: boolean;
    file: string;
    preview: string;
    truncated: boolean;
    candidates: number;
}

/** Pure ranking: file is a hint, not a filter. Exported for unit tests. */
export function rankBody(
    fileHint: string | undefined,
    bodies: Array<{ file: string; preview: string; truncated?: boolean }>,
): RankedBody {
    if (!bodies || bodies.length === 0) {
        return { found: false, file: '', preview: '', truncated: false, candidates: 0 };
    }
    const hit = (fileHint && bodies.find((b) => b.file === fileHint)) || bodies[0];
    return { found: true, file: hit.file, preview: hit.preview, truncated: !!hit.truncated, candidates: bodies.length };
}

/** Bounded body sub-object of `loctree/symbolContext`. */
export interface SymbolContextBody {
    source: string;
    /** 1-based start line of the body. */
    start_line: number;
    /** 1-based end line actually returned (inclusive). */
    end_line: number;
    /** True if the body exceeded `body_max_lines` and was truncated. */
    truncated: boolean;
    /** Total lines the full body spans (pre-cap). */
    total_lines: number;
}

/**
 * A single occurrence in a `loctree/symbolContext` response. This mirrors the
 * server's leaner `OccurrenceView` (file/line/column/context/kind) — it is NOT
 * the richer literal-search `Occurrence` (which carries matched_text/source and
 * names the field `occurrence_kind`). Typed honestly so consumers don't read
 * fields the wire never sends.
 */
export interface SymbolContextOccurrence {
    file: string;
    line: number;
    column: number;
    context: string;
    kind: string;
}

/** Occurrences sub-object of `loctree/symbolContext` — same-file totals + pagination. */
export interface SymbolContextOccurrences {
    /** Total literal occurrences across the searched scope. */
    total: number;
    /** Total literal occurrences in the requested `file` only. */
    same_file_total: number;
    /** Occurrences returned in this page. */
    returned: SymbolContextOccurrence[];
    /** Whether more occurrences exist after this page. */
    has_more: boolean;
    /** Offset to request for the next page. Omitted on the final page. */
    next_offset?: number;
}

/** Best-effort enclosing-symbol context of `loctree/symbolContext`. */
export interface SymbolContextParent {
    /** Enclosing function/class name, if resolvable from the live tree. */
    name?: string;
    /** Provenance: `"live_ast"` when resolved, `"unavailable"` otherwise. */
    source: string;
}

/**
 * `loctree/symbolContext` response — the Context-King surface. Mirrors the
 * Rust `SymbolContextResponse` in `loctree-lsp/src/symbol_context.rs` exactly.
 *
 * `exported`/`internal` are computed from the SYMBOL IDENTITY at `file +
 * position` (not from `find` literal's `dead_status` stub): `true` when the
 * symbol resolved with that status, `false` for the opposite, `null` when the
 * identity could not be resolved at all.
 */
export interface SymbolContextResponse {
    /** Resolved symbol name. */
    symbol: string;
    /** File the request anchored on (echoed from params). */
    file: string;
    /** Range of the symbol declaration, when a line was resolved. */
    range?: Range;
    /** Symbol kind (`function`, `class`, `const`, …) when known. */
    kind?: string;
    /** `true`/`false` from symbol identity; `null` when identity unresolved. */
    exported: boolean | null;
    /** `true` when the symbol resolved in `file` and is NOT exported; `null` when unresolved. */
    internal: boolean | null;
    /**
     * Workspace-relative DECLARING file, set ONLY when the symbol resolved
     * CROSS-FILE through the import graph (the cursor sat on a usage of an
     * imported symbol whose declaration lives elsewhere). Absent when the
     * symbol resolved locally in `file`, or stayed unresolved. The hover uses
     * this to label "defined in <D>" and to know `body` came from `D`.
     */
    defined_in?: string;
    /** Bounded body, when a definition was located IN the requested file. */
    body?: SymbolContextBody;
    /**
     * Set when `body` is absent because the symbol resolves only in OTHER
     * files (`"not_found_in_file"`). We never surface a cross-file body.
     */
    body_error?: 'not_found_in_file';
    /** Literal occurrences with same-file totals + pagination. */
    occurrences: SymbolContextOccurrences;
    /** Best-effort enclosing function/class context. */
    parent_context?: SymbolContextParent;
}

/** Options for `loctree/symbolContext`, mapped to the LSP snake_case params. */
export interface SymbolContextOptions {
    /** Optional symbol-name hint; identity is still computed from file+position. */
    symbol?: string;
    /** Maximum body source lines (LSP `body_max_lines`). */
    bodyMaxLines?: number;
    /** Occurrence page size (LSP `occurrence_limit`). */
    occurrenceLimit?: number;
    /** Return only same-file occurrences — cheap hover path (LSP `same_file_only`). */
    sameFileOnly?: boolean;
    /** Zero-based occurrence offset for paged output. */
    offset?: number;
    /** Tighter literal identifier boundaries (LSP `whole_token`). */
    wholeToken?: boolean;
}

export interface FindOptions {
    mode?: 'single' | 'split' | 'and' | 'literal';
    lang?: string;
    dead_only?: boolean;
    exported_only?: boolean;
    limit?: number;
    cursor?: string;
    /** Page size for paginated (non-literal) match buckets. */
    chunk_size?: number;
    /** Literal mode: require tighter whole-token identifier boundaries. */
    whole_token?: boolean;
    /** Literal mode: attach a per-file occurrence rollup (`by_file`). */
    group_by_file?: boolean;
    /** Literal mode: zero-based occurrence offset for paged output. */
    offset?: number;
    /** Literal mode: suppress the occurrence list, keep counters only. */
    count_only?: boolean;
    /** Literal mode: alias for `count_only` (MCP/CLI wording). */
    slim?: boolean;
    /** Stable `<file_path>::<symbol_name>` anchor echoed back in the response. */
    symbol_id?: string;
}

export interface FollowOptions {
    handler?: string;
    limit?: number;
    cursor?: string;
}

/** `loctree/contextPack` response — one bounded agent-context section. */
export interface ContextPackResponse {
    /** Zero-based section index. */
    section: number;
    /** Card id this section belongs to (e.g. `core`, `risk`). */
    card: string;
    /** Human-readable section title. */
    title: string;
    /** Rendered markdown content of this section. */
    content: string;
    /** Total sections across the materialized pack. */
    total_sections: number;
    /** Opaque cursor for the next section, when more remain. */
    next_cursor?: string | null;
}

/** Options for `loctree/contextPack`, mapped to the LSP snake_case params. */
export interface ContextPackOptions {
    /** Opaque cursor from a previous response to page forward. */
    cursor?: string;
    /** Ordered card ids, e.g. `["core", "risk"]`. */
    cards?: string[];
    /** Natural-language task hint for context narrowing. */
    task?: string;
}

export class LoctreeNotRunningError extends Error {
    public readonly state: LspRuntimeState;

    constructor(state: LspRuntimeState = DEFAULT_LSP_RUNTIME_STATE) {
        super(notRunningMessage(state));
        this.name = 'LoctreeNotRunningError';
        this.state = state;
    }
}

/**
 * True when an LSP error means the snapshot is not loaded *yet* (transient,
 * during the initial scan), rather than a hard failure. The server returns
 * ServerError(-32001) "loctree snapshot not loaded yet" in this window.
 */
export function isSnapshotNotLoaded(error: unknown): boolean {
    const e = error as { code?: number; message?: unknown };
    if (e && e.code === -32001) {
        return true;
    }
    return (
        !!e &&
        typeof e.message === 'string' &&
        e.message.toLowerCase().includes('not loaded')
    );
}

/**
 * Typed gateway over the loctree-lsp custom requests.
 *
 * Holds a getter (not the client itself) so it always talks to the live
 * client instance even across restarts.
 */
export class LoctreeGateway {
    private readonly getRuntimeState: () => LspRuntimeState;

    constructor(
        private readonly getClient: () => LanguageClient | undefined,
        getRuntimeState?: () => LspRuntimeState,
    ) {
        this.getRuntimeState = getRuntimeState ?? (() => stateFromClient(this.getClient()));
    }

    /** Single source of truth for every editor surface's LSP lifecycle display. */
    public lspState(): LspRuntimeState {
        return this.getRuntimeState();
    }

    /** True when the LSP client is connected and ready to take requests. */
    public isReady(): boolean {
        const client = this.getClient();
        return this.lspState().phase === 'running' && client !== undefined && client.isRunning();
    }

    private require(): LanguageClient {
        const client = this.getClient();
        if (!client || !client.isRunning() || this.lspState().phase !== 'running') {
            throw new LoctreeNotRunningError(this.lspState());
        }
        return client;
    }

    public health(includeTopRisks = false): Promise<HealthResponse> {
        return this.require().sendRequest<HealthResponse>('loctree/health', {
            include_top_risks: includeTopRisks,
        });
    }

    public find(query: string, options: FindOptions = {}): Promise<FindResponse> {
        return this.require().sendRequest<FindResponse>('loctree/find', {
            query,
            mode: options.mode ?? 'single',
            lang: options.lang,
            dead_only: options.dead_only ?? false,
            exported_only: options.exported_only ?? false,
            limit: options.limit,
            cursor: options.cursor,
            chunk_size: options.chunk_size,
            whole_token: options.whole_token,
            group_by_file: options.group_by_file,
            offset: options.offset,
            count_only: options.count_only,
            slim: options.slim,
            symbol_id: options.symbol_id,
        });
    }

    /** Convenience: exact-identifier occurrence scan (= `loct occurrences`). */
    public literal(query: string, options: Omit<FindOptions, 'mode'> = {}): Promise<FindResponse> {
        return this.find(query, { ...options, mode: 'literal' });
    }

    public follow(scope: string, options: FollowOptions = {}): Promise<FollowResponse> {
        return this.require().sendRequest<FollowResponse>('loctree/follow', {
            scope,
            handler: options.handler,
            limit: options.limit,
            cursor: options.cursor,
        });
    }

    public impact(target: string, transitive = false): Promise<unknown> {
        return this.require().sendRequest('loctree/impact', { target, transitive });
    }

    public slice(
        target: string,
        consumers = true,
        depth?: number,
        cursor?: string
    ): Promise<unknown> {
        return this.require().sendRequest('loctree/slice', { target, consumers, depth, cursor });
    }

    public body(symbol: string, options: { file?: string; max_lines?: number } = {}): Promise<BodyResponse> {
        return this.require().sendRequest<BodyResponse>('loctree/body', {
            symbol,
            file: options.file,
            max_lines: options.max_lines,
        });
    }

    /**
     * Fix for the 'no body found' bug: resolve cross-file (no `file` filter),
     * then rank by hint client-side. Never returns not-found if any body exists.
     */
    public async bodyRanked(symbol: string, fileHint?: string): Promise<RankedBody> {
        const resp = await this.body(symbol);
        const bodies = (resp.bodies ?? []).map((b: SymbolBody) => ({
            file: b.file,
            preview: b.source,
            truncated: b.truncated,
        }));
        return rankBody(fileHint, bodies);
    }

    /**
     * `loctree/symbolContext` — the Context-King surface. Resolves one bounded,
     * literal context pack for the symbol at `file + position`: export/internal
     * badge (from symbol identity, not the literal `dead_status` stub), bounded
     * body disambiguated to `file`, paginated literal occurrences with same-file
     * totals, and a best-effort parent context.
     *
     * Maps the camelCase `opts` to the LSP snake_case params. Like the other
     * custom requests, a -32001 "snapshot not loaded yet" rejection surfaces via
     * the shared {@link isSnapshotNotLoaded} guard the callers apply.
     */
    public symbolContext(
        file: string,
        position: { line: number; character: number },
        opts: SymbolContextOptions = {},
    ): Promise<SymbolContextResponse> {
        return this.require().sendRequest<SymbolContextResponse>('loctree/symbolContext', {
            file,
            position,
            symbol: opts.symbol,
            body_max_lines: opts.bodyMaxLines,
            occurrence_limit: opts.occurrenceLimit,
            same_file_only: opts.sameFileOnly,
            offset: opts.offset,
            whole_token: opts.wholeToken,
        });
    }

    /**
     * `loctree/contextPack` — one bounded agent-context section. The Context
     * panel uses this for its "Context Pack" mode; pass `cursor` to page through
     * subsequent sections.
     */
    public contextPack(options: ContextPackOptions = {}): Promise<ContextPackResponse> {
        return this.require().sendRequest<ContextPackResponse>('loctree/contextPack', {
            cursor: options.cursor,
            cards: options.cards,
            task: options.task,
        });
    }

    public refresh(): void {
        if (!this.isReady()) {
            return;
        }
        this.require().sendNotification('loctree/refresh');
    }
}
