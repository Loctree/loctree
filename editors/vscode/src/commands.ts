/**
 * Command Handlers for Loctree VSCode Extension
 *
 * Registers and implements all loctree commands.
 *
 * 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Vetcoders
 */

import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';
import { LoctreeGateway, LoctreeNotRunningError, Occurrence, formatHealthScore, lspStateLabel } from './gateway';

interface LocationTarget {
    filePath: string;
    line?: number;
    column?: number;
}

type CommandPayload = Record<string, unknown>;
// Intentional: detect control characters in payloads before they reach the shell/LSP.
// eslint-disable-next-line no-control-regex
const CONTROL_CHAR_REGEX = /[\x00-\x1F\x7F]/;

function asRecord(value: unknown): CommandPayload | undefined {
    if (typeof value === 'object' && value !== null && !Array.isArray(value)) {
        return value as CommandPayload;
    }
    return undefined;
}

function getString(payload: CommandPayload, key: string): string | undefined {
    const value = payload[key];
    if (typeof value === 'string') {
        const trimmed = value.trim();
        if (trimmed.length > 0) {
            return trimmed;
        }
    }
    return undefined;
}

function getNumber(payload: CommandPayload, key: string): number | undefined {
    const value = payload[key];
    if (typeof value === 'number' && Number.isFinite(value)) {
        return value;
    }
    if (typeof value === 'string') {
        const parsed = Number(value);
        if (Number.isFinite(parsed)) {
            return parsed;
        }
    }
    return undefined;
}

function normalizeCommandArgs(args: unknown[]): unknown[] {
    if (args.length === 1 && Array.isArray(args[0])) {
        return args[0] as unknown[];
    }
    return args;
}

function hasUnsafeControlChars(value: string): boolean {
    return CONTROL_CHAR_REGEX.test(value);
}

function safeRelativeParts(raw: string): string[] | null {
    if (!raw || path.isAbsolute(raw) || raw.includes('\0')) {
        return null;
    }
    const parts = raw.split(/[\\/]+/).filter((part) => part !== '' && part !== '.');
    if (parts.some((part) => part === '..')) {
        return null;
    }
    return parts;
}

function isWithinWorkspace(absolutePath: string, workspaceRoot: string): boolean {
    const root = path.normalize(workspaceRoot);
    const target = path.normalize(absolutePath);
    const relativePath = path.relative(root, target);

    return (
        relativePath === '' ||
        (!relativePath.startsWith('..') && !path.isAbsolute(relativePath))
    );
}

function safeChildPath(root: string, ...segments: string[]): string | undefined {
    const parts: string[] = [];
    for (const segment of segments) {
        const segmentParts = safeRelativeParts(segment);
        if (!segmentParts) {
            return undefined;
        }
        parts.push(...segmentParts);
    }

    const normalizedRoot = path.normalize(root);
    const child = path.normalize(vscode.Uri.joinPath(vscode.Uri.file(normalizedRoot), ...parts).fsPath);
    return isWithinWorkspace(child, normalizedRoot) ? child : undefined;
}

function trustedChildPath(root: string, ...segments: string[]): string {
    const child = safeChildPath(root, ...segments);
    if (!child) {
        throw new Error(`Refusing to resolve path outside trusted root: ${segments.join('/')}`);
    }
    return child;
}

function resolveFilePath(filePath: string, workspaceRoot: string): string | undefined {
    if (hasUnsafeControlChars(filePath)) {
        return undefined;
    }

    if (filePath.startsWith('file://')) {
        try {
            return vscode.Uri.parse(filePath).fsPath;
        } catch {
            return undefined;
        }
    }

    if (path.isAbsolute(filePath)) {
        return path.normalize(filePath);
    }

    return safeChildPath(workspaceRoot, filePath);
}

function toWorkspaceAbsolutePath(filePath: string, workspaceRoot: string): string | undefined {
    const absolutePath = resolveFilePath(filePath, workspaceRoot);
    if (!absolutePath) {
        return undefined;
    }

    if (!isWithinWorkspace(absolutePath, workspaceRoot)) {
        return undefined;
    }

    return absolutePath;
}

function extractLocationTarget(raw: unknown): LocationTarget | undefined {
    if (typeof raw === 'string') {
        const trimmed = raw.trim();
        if (trimmed.length > 0) {
            return { filePath: trimmed };
        }
        return undefined;
    }

    const payload = asRecord(raw);
    if (!payload) {
        return undefined;
    }

    const filePath =
        getString(payload, 'filePath') ??
        getString(payload, 'file') ??
        getString(payload, 'path') ??
        getString(payload, 'uri');

    if (!filePath) {
        return undefined;
    }

    return {
        filePath,
        line: getNumber(payload, 'line'),
        column: getNumber(payload, 'column'),
    };
}

function extractCycleChain(raw: unknown): string[] {
    if (typeof raw === 'string') {
        return raw
            .split('->')
            .map((part) => part.trim())
            .filter((part) => part.length > 0);
    }

    if (Array.isArray(raw)) {
        const chain: string[] = [];
        for (const item of raw) {
            if (typeof item === 'string') {
                const trimmed = item.trim();
                if (trimmed.length > 0) {
                    chain.push(trimmed);
                    continue;
                }
            }
            const nestedPayload = asRecord(item);
            const nestedFile = nestedPayload ? getString(nestedPayload, 'file') : undefined;
            if (nestedFile) {
                chain.push(nestedFile);
            }
        }
        return chain;
    }

    const payload = asRecord(raw);
    if (!payload) {
        return [];
    }

    const nested =
        payload.cycle ??
        payload.chain ??
        payload.files ??
        payload.fileAndCycle;

    if (nested !== undefined) {
        const chain = extractCycleChain(nested);
        if (chain.length > 0) {
            return chain;
        }
    }

    const file = getString(payload, 'file');
    return file ? [file] : [];
}

function pickFileForCommand(args: unknown[], workspaceRoot: string): string | undefined {
    for (const arg of args) {
        const location = extractLocationTarget(arg);
        if (location) {
            const workspacePath = toWorkspaceAbsolutePath(location.filePath, workspaceRoot);
            if (workspacePath) {
                return workspacePath;
            }
        }

        const cycleChain = extractCycleChain(arg);
        if (cycleChain.length > 0) {
            const workspacePath = toWorkspaceAbsolutePath(cycleChain[0], workspaceRoot);
            if (workspacePath) {
                return workspacePath;
            }
        }
    }

    const editor = vscode.window.activeTextEditor;
    if (!editor) {
        return undefined;
    }

    return toWorkspaceAbsolutePath(editor.document.uri.fsPath, workspaceRoot);
}

/**
 * Run a gateway call and present its result, with uniform handling of the
 * "LSP not running" case. Returns the result, or undefined on failure.
 */
async function runGatewayCall<T>(
    outputChannel: vscode.OutputChannel,
    label: string,
    call: () => Promise<T>
): Promise<T | undefined> {
    try {
        return await call();
    } catch (error) {
        if (error instanceof LoctreeNotRunningError) {
            vscode.window.showWarningMessage(error.message);
            outputChannel.appendLine(`Skipped ${label}: ${error.message}`);
            return undefined;
        }
        const message = error instanceof Error ? error.message : String(error);
        outputChannel.appendLine(`${label} failed: ${message}`);
        vscode.window.showErrorMessage(`Loctree: ${label} failed (${message})`);
        return undefined;
    }
}

function describeLspUnavailable(gateway: LoctreeGateway): string {
    const state = gateway.lspState();
    if (state.phase === 'error') {
        return `Loctree LSP failed: ${state.message}`;
    }
    return `Loctree LSP is ${lspStateLabel(state).toLowerCase()}: ${state.message}`;
}

/** Open rendered Markdown in a read-only preview tab. */
async function showMarkdown(title: string, markdown: string): Promise<void> {
    const doc = await vscode.workspace.openTextDocument({
        content: `<!-- ${title} -->\n\n${markdown}`,
        language: 'markdown',
    });
    await vscode.window.showTextDocument(doc, { preview: true });
}

function relPath(filePath: string, workspaceRoot: string): string {
    if (path.isAbsolute(filePath)) {
        const rel = path.relative(workspaceRoot, filePath);
        if (rel && !rel.startsWith('..') && !path.isAbsolute(rel)) {
            return rel;
        }
    }
    return filePath;
}

/** Navigate the active editor to a single occurrence site. */
async function navigateToOccurrence(
    workspaceRoot: string,
    occ: Occurrence
): Promise<void> {
    const absolute = toWorkspaceAbsolutePath(occ.file, workspaceRoot);
    if (!absolute) {
        vscode.window.showWarningMessage(
            'Loctree: blocked path outside workspace or invalid path payload'
        );
        return;
    }
    if (!fs.existsSync(absolute)) {
        vscode.window.showWarningMessage(`Loctree: file not found: ${occ.file}`);
        return;
    }
    const document = await vscode.workspace.openTextDocument(vscode.Uri.file(absolute));
    const editor = await vscode.window.showTextDocument(document, { preview: false });
    const line = Math.max(occ.line - 1, 0);
    const column = Math.max((occ.column ?? 1) - 1, 0);
    const position = new vscode.Position(line, column);
    editor.selection = new vscode.Selection(position, position);
    editor.revealRange(new vscode.Range(position, position), vscode.TextEditorRevealType.InCenter);
}

const LOAD_MORE_SENTINEL = Symbol('loctree.loadMore');

/**
 * Present literal occurrences as a navigable, paginated QuickPick.
 *
 * Pages are accumulated across "Load more…" selections: each subsequent page
 * is fetched via `loadMore(offset)` and appended, so the user never has the
 * whole occurrence set dumped at once (P0 #3). The title reflects
 * `returned / total` of what has been loaded so far.
 */
async function presentOccurrences(
    workspaceRoot: string,
    query: string,
    initial: { occurrences: Occurrence[]; total: number; nextOffset?: number | null; hasMore: boolean },
    loadMore: (
        offset: number
    ) => Promise<{ occurrences: Occurrence[]; total: number; nextOffset?: number | null; hasMore: boolean } | undefined>
): Promise<void> {
    if (initial.occurrences.length === 0) {
        vscode.window.showInformationMessage(`Loctree: no occurrences of "${query}" found.`);
        return;
    }

    interface OccurrenceQuickPick extends vscode.QuickPickItem {
        occurrence?: Occurrence;
        action?: typeof LOAD_MORE_SENTINEL;
    }

    const loaded: Occurrence[] = [...initial.occurrences];
    let total = initial.total;
    let hasMore = initial.hasMore;
    let nextOffset = initial.nextOffset ?? undefined;

    const occurrenceItem = (occ: Occurrence): OccurrenceQuickPick => ({
        label: `$(symbol-key) ${relPath(occ.file, workspaceRoot)}:${occ.line}`,
        description: occ.occurrence_kind,
        detail: occ.context?.trim(),
        occurrence: occ,
    });

    // Loop so "Load more…" re-opens the QuickPick with the appended page.
    for (; ;) {
        const items: OccurrenceQuickPick[] = loaded.map(occurrenceItem);
        if (hasMore && nextOffset !== undefined) {
            items.push({
                label: '$(ellipsis) Load more…',
                description: `${loaded.length} / ${total} loaded`,
                action: LOAD_MORE_SENTINEL,
            });
        }

        const picked = await vscode.window.showQuickPick(items, {
            title: `${loaded.length} / ${total} literal occurrences: "${query}"`,
            placeHolder: 'Select an occurrence to navigate to it',
            matchOnDescription: true,
            matchOnDetail: true,
        });

        if (!picked) {
            return;
        }

        if (picked.action === LOAD_MORE_SENTINEL) {
            if (nextOffset === undefined) {
                return;
            }
            const page = await loadMore(nextOffset);
            if (!page) {
                return;
            }
            loaded.push(...page.occurrences);
            total = page.total;
            hasMore = page.hasMore;
            nextOffset = page.nextOffset ?? undefined;
            continue;
        }

        if (picked.occurrence) {
            await navigateToOccurrence(workspaceRoot, picked.occurrence);
        }
        return;
    }
}

function tomlString(value: string): string {
    const sanitized = value
        .replace(/\\/g, '\\\\')
        .replace(/"/g, '\\"')
        .replace(/\n/g, '\\n')
        .replace(/\r/g, '\\r')
        .replace(/\t/g, '\\t')
        // Intentional: strip residual control characters from TOML string values.
        // eslint-disable-next-line no-control-regex
        .replace(/[\x00-\x08\x0B\x0C\x0E-\x1F\x7F]/g, '');
    return `"${sanitized}"`;
}

async function appendCycleSuppression(
    workspaceRoot: string,
    cycleChain: string[]
): Promise<string> {
    const suppressionsDir = trustedChildPath(workspaceRoot, '.loctree');
    const suppressionsPath = trustedChildPath(suppressionsDir, 'suppressions.toml');
    const cycleSymbol = cycleChain.join(' -> ');
    const filePath = cycleChain[0];
    const date = new Date().toISOString().slice(0, 10);

    await fs.promises.mkdir(suppressionsDir, { recursive: true });

    if (!fs.existsSync(suppressionsPath)) {
        await fs.promises.writeFile(
            suppressionsPath,
            '# Loctree suppressions - findings marked as reviewed/OK\n\n',
            'utf-8'
        );
    }

    const existing = await fs.promises.readFile(suppressionsPath, 'utf-8');
    const symbolLine = `symbol = ${tomlString(cycleSymbol)}`;
    if (existing.includes('type = "circular"') && existing.includes(symbolLine)) {
        return suppressionsPath;
    }

    const entry = [
        '',
        '[[suppress]]',
        'type = "circular"',
        `symbol = ${tomlString(cycleSymbol)}`,
        `file = ${tomlString(filePath)}`,
        'reason = "Ignored via VSCode quick action"',
        `added = ${tomlString(date)}`,
        '',
    ].join('\n');

    await fs.promises.appendFile(suppressionsPath, entry, 'utf-8');
    return suppressionsPath;
}

async function navigateToFile(
    workspaceRoot: string,
    outputChannel: vscode.OutputChannel,
    raw: unknown
): Promise<void> {
    const location = extractLocationTarget(raw);
    if (!location) {
        vscode.window.showWarningMessage('Loctree: no file path provided');
        return;
    }

    const absolutePath = toWorkspaceAbsolutePath(location.filePath, workspaceRoot);
    if (!absolutePath) {
        vscode.window.showWarningMessage(
            'Loctree: blocked path outside workspace or invalid path payload'
        );
        return;
    }

    if (!fs.existsSync(absolutePath)) {
        vscode.window.showWarningMessage(`Loctree: file not found: ${location.filePath}`);
        return;
    }

    const document = await vscode.workspace.openTextDocument(vscode.Uri.file(absolutePath));
    const editor = await vscode.window.showTextDocument(document, { preview: false });

    if (location.line !== undefined) {
        const line = Math.max(location.line - 1, 0);
        const column = Math.max((location.column ?? 1) - 1, 0);
        const position = new vscode.Position(line, column);
        editor.selection = new vscode.Selection(position, position);
        editor.revealRange(new vscode.Range(position, position), vscode.TextEditorRevealType.InCenter);
    }

    outputChannel.appendLine(`Opened file: ${absolutePath}`);
}

/**
 * Register all loctree commands
 */
export function registerCommands(
    context: vscode.ExtensionContext,
    outputChannel: vscode.OutputChannel,
    workspaceRoot: string,
    gateway: LoctreeGateway,
    onManualRefresh: () => void
): void {
    const reg = (name: string, handler: (...args: unknown[]) => unknown): void => {
        context.subscriptions.push(vscode.commands.registerCommand(name, handler));
    };

    // Refresh analysis (reload the snapshot, then update status bar + tree)
    reg('loctree.refresh', () => {
        if (!gateway.isReady()) {
            vscode.window.showWarningMessage(describeLspUnavailable(gateway));
            return;
        }
        onManualRefresh();
        outputChannel.appendLine('Requested loctree refresh via LSP');
        vscode.window.showInformationMessage('Loctree refresh requested.');
    });

    // Open HTML report (resolved by loct in the per-project cache dir)
    reg('loctree.openReport', async () => {
        const localReport = trustedChildPath(workspaceRoot, '.loctree', 'report.html');
        if (fs.existsSync(localReport)) {
            await vscode.env.openExternal(vscode.Uri.file(localReport));
            outputChannel.appendLine(`Opened report: ${localReport}`);
            return;
        }
        vscode.window.showInformationMessage(
            'Loctree HTML report lives in the per-project cache. Run `loct context` to (re)generate it, ' +
            'or use the Loctree views which read live data from the LSP.'
        );
    });

    // Show health summary (real loctree/health, rendered)
    reg('loctree.showHealth', async () => {
        const health = await runGatewayCall(outputChannel, 'health', () => gateway.health(true));
        if (!health) {
            return;
        }
        const risks = health.top_risks
            .map((r) => `- **${r.severity}** \`${r.kind}\` ${relPath(r.file, workspaceRoot)} — ${r.message}`)
            .join('\n');
        const actions = health.recommended_actions.map((a) => `- ${a}`).join('\n');
        const md = [
            `# Loctree Health: ${formatHealthScore(health.health_score)} (${health.status})`,
            '',
            `| Metric | Count |`,
            `|---|---|`,
            `| Dead exports | ${health.dead_exports} |`,
            `| Circular imports | ${health.cycles} |`,
            `| Twins | ${health.twins} |`,
            `| Hotspots | ${health.hotspots} |`,
            '',
            health.snapshot_stale ? '> ⚠ Snapshot is stale relative to HEAD — run a refresh.\n' : '',
            risks ? `## Top risks\n${risks}` : '',
            actions ? `\n## Recommended actions\n${actions}` : '',
        ].join('\n');
        await showMarkdown('Loctree Health', md);
    });

    // Analyze impact (blast radius) — real loctree/impact
    reg('loctree.analyzeImpact', async (...rawArgs: unknown[]) => {
        const filePath = pickFileForCommand(normalizeCommandArgs(rawArgs), workspaceRoot);
        if (!filePath) {
            vscode.window.showWarningMessage('No file selected for impact analysis');
            return;
        }
        const rel = relPath(filePath, workspaceRoot);
        const result = await runGatewayCall(outputChannel, `impact ${rel}`, () =>
            gateway.impact(rel, true)
        );
        if (result === undefined) {
            return;
        }
        await showMarkdown(
            `Loctree Impact: ${rel}`,
            `# Impact — \`${rel}\`\n\n\`\`\`json\n${JSON.stringify(result, null, 2)}\n\`\`\``
        );
    });

    // Find consumers / importers — real loctree/impact (direct consumers)
    const consumersHandler = async (...rawArgs: unknown[]): Promise<void> => {
        const filePath = pickFileForCommand(normalizeCommandArgs(rawArgs), workspaceRoot);
        if (!filePath) {
            vscode.window.showWarningMessage('No file selected');
            return;
        }
        const rel = relPath(filePath, workspaceRoot);
        const result = await runGatewayCall(outputChannel, `consumers ${rel}`, () =>
            gateway.impact(rel, false)
        );
        if (result === undefined) {
            return;
        }
        await showMarkdown(
            `Loctree Consumers: ${rel}`,
            `# Consumers — \`${rel}\`\n\n\`\`\`json\n${JSON.stringify(result, null, 2)}\n\`\`\``
        );
    };
    reg('loctree.findConsumers', consumersHandler);
    reg('loctree.findImporters', consumersHandler);

    // Show slice (file context) — real loctree/slice
    reg('loctree.showSlice', async (...rawArgs: unknown[]) => {
        const filePath = pickFileForCommand(normalizeCommandArgs(rawArgs), workspaceRoot);
        if (!filePath) {
            vscode.window.showWarningMessage('No file selected');
            return;
        }
        const rel = relPath(filePath, workspaceRoot);
        const result = await runGatewayCall(outputChannel, `slice ${rel}`, () =>
            gateway.slice(rel, true)
        );
        if (result === undefined) {
            return;
        }
        await showMarkdown(
            `Loctree Slice: ${rel}`,
            `# Slice — \`${rel}\`\n\n\`\`\`json\n${JSON.stringify(result, null, 2)}\n\`\`\``
        );
    });

    // Show cycle details from a diagnostic argument (no LSP call needed)
    reg('loctree.showCycleDetails', (...rawArgs: unknown[]) => {
        const args = normalizeCommandArgs(rawArgs);
        const uri = typeof args[0] === 'string' ? args[0] : undefined;
        const cycleChain = extractCycleChain(args[1] ?? args[0]);
        const chainText = cycleChain.join(' -> ');
        outputChannel.appendLine('=== Loctree Cycle Details ===');
        if (uri) {
            outputChannel.appendLine(`Diagnostic URI: ${uri}`);
        }
        outputChannel.appendLine(
            chainText.length > 0 ? `Cycle chain: ${chainText}` : 'Cycle chain not provided.'
        );
        outputChannel.show(true);
    });

    // Open target file in editor
    reg('loctree.navigateToFile', async (...rawArgs: unknown[]) => {
        await navigateToFile(workspaceRoot, outputChannel, normalizeCommandArgs(rawArgs)[0]);
    });

    // Suppress a cycle in .loctree/suppressions.toml
    reg('loctree.ignoreCycle', async (...rawArgs: unknown[]) => {
        const cycleChain = extractCycleChain(normalizeCommandArgs(rawArgs)[0]);
        if (cycleChain.length === 0) {
            vscode.window.showWarningMessage('Loctree: no cycle chain provided');
            return;
        }
        try {
            const suppressionsPath = await appendCycleSuppression(workspaceRoot, cycleChain);
            outputChannel.appendLine(`Ignored cycle in ${suppressionsPath}: ${cycleChain.join(' -> ')}`);
            vscode.window.showInformationMessage('Loctree: cycle added to suppressions');
        } catch (error) {
            const message = error instanceof Error ? error.message : String(error);
            outputChannel.appendLine(`Failed to write cycle suppression: ${message}`);
            vscode.window.showErrorMessage(`Loctree: failed to ignore cycle (${message})`);
        }
    });

    // Show all cycles — real loctree/follow cycles
    const cyclesHandler = async (): Promise<void> => {
        const result = await runGatewayCall(outputChannel, 'cycles', () => gateway.follow('cycles'));
        if (result === undefined) {
            return;
        }
        await showMarkdown(
            'Loctree Cycles',
            `# Circular imports\n\n\`\`\`json\n${JSON.stringify(result.items.data, null, 2)}\n\`\`\``
        );
    };
    reg('loctree.showCycles', cyclesHandler);
    reg('loctree.analyzeCycle', cyclesHandler);

    // Check dead exports — real loctree/follow dead
    reg('loctree.checkDeadExports', async () => {
        const result = await runGatewayCall(outputChannel, 'dead', () => gateway.follow('dead'));
        if (result === undefined) {
            return;
        }
        await showMarkdown(
            'Loctree Dead Exports',
            `# Dead exports\n\n\`\`\`json\n${JSON.stringify(result.items.data, null, 2)}\n\`\`\``
        );
    });

    // NEW: literal occurrences (= loct occurrences / find --literal)
    reg('loctree.searchLiteral', async (...rawArgs: unknown[]) => {
        const args = normalizeCommandArgs(rawArgs);
        const seed =
            (typeof args[0] === 'string' ? args[0] : undefined) ?? symbolUnderCursor() ?? '';
        const query = await vscode.window.showInputBox({
            title: 'Loctree: literal occurrence search',
            prompt:
                'Exact identifier-boundary occurrences (no fuzzy). Multi-OR: NameA|NameB — same engine as loct find / MCP find literal',
            value: seed,
        });
        if (!query) {
            return;
        }

        const LITERAL_PAGE_SIZE = 50;

        // Sharpest literal mode: identifier-boundary whole-token scan, per-file
        // rollup, first page of 50 starting at offset 0 (P0 #2).
        const queryLiteralPage = async (
            offset: number
        ): Promise<
            | { occurrences: Occurrence[]; total: number; nextOffset?: number | null; hasMore: boolean }
            | undefined
        > => {
            const response = await runGatewayCall(outputChannel, `occurrences "${query}"`, () =>
                gateway.literal(query, {
                    whole_token: true,
                    group_by_file: true,
                    limit: LITERAL_PAGE_SIZE,
                    offset,
                })
            );
            if (response === undefined) {
                return undefined;
            }
            const literal = response.literal_matches;
            const occurrences = literal?.occurrences ?? [];
            const total = literal?.total ?? occurrences.length;
            const page = literal?.page;
            return {
                occurrences,
                total,
                nextOffset: page?.next_offset ?? undefined,
                hasMore: page?.has_more ?? false,
            };
        };

        const first = await queryLiteralPage(0);
        if (first === undefined) {
            return;
        }
        await presentOccurrences(workspaceRoot, query, first, queryLiteralPage);
    });

    // NEW: bounded symbol body (= loct body)
    reg('loctree.showBody', async (...rawArgs: unknown[]) => {
        const args = normalizeCommandArgs(rawArgs);
        const seed =
            (typeof args[0] === 'string' ? args[0] : undefined) ?? symbolUnderCursor() ?? '';
        const symbol = await vscode.window.showInputBox({
            title: 'Loctree: show symbol body',
            prompt: 'Bounded source body of a function/method/symbol',
            value: seed,
        });
        if (!symbol) {
            return;
        }

        // Disambiguate by the active file (P0 #4): common names like
        // parse/run/init/load/render resolve to the symbol in the file the
        // user is editing, not the first match in the snapshot. The file is
        // passed relative to the workspace root, and a hover-grade line cap.
        const BODY_MAX_LINES = 120;
        const activeEditor = vscode.window.activeTextEditor;
        const activeFile = activeEditor
            ? (() => {
                const abs = toWorkspaceAbsolutePath(activeEditor.document.uri.fsPath, workspaceRoot);
                return abs ? relPath(abs, workspaceRoot) : undefined;
            })()
            : undefined;

        const response = await runGatewayCall(outputChannel, `body ${symbol}`, () =>
            gateway.body(symbol, { file: activeFile, max_lines: BODY_MAX_LINES })
        );
        if (response === undefined) {
            return;
        }
        if (response.bodies.length === 0) {
            vscode.window.showInformationMessage(`Loctree: no body found for "${symbol}".`);
            return;
        }
        const body = response.bodies[0];
        const doc = await vscode.workspace.openTextDocument({
            content: body.source,
            language: languageIdFor(body.language),
        });
        await vscode.window.showTextDocument(doc, { preview: true });
        outputChannel.appendLine(
            `body ${symbol}: ${body.file}:${body.start_line}-${body.end_line}` +
            (body.truncated ? ` (truncated, ${body.total_lines} total, cap ${body.line_cap})` : '')
        );
    });
}

/** Best-effort: the identifier under the active editor's cursor. */
function symbolUnderCursor(): string | undefined {
    const editor = vscode.window.activeTextEditor;
    if (!editor) {
        return undefined;
    }
    const range = editor.document.getWordRangeAtPosition(editor.selection.active);
    return range ? editor.document.getText(range) : undefined;
}

/** Map loctree's language tag onto a VS Code language id for syntax coloring. */
function languageIdFor(lang: string): string {
    switch (lang) {
        case 'ts':
            return 'typescript';
        case 'tsx':
            return 'typescriptreact';
        case 'js':
            return 'javascript';
        case 'jsx':
            return 'javascriptreact';
        case 'rs':
            return 'rust';
        case 'py':
            return 'python';
        case 'kt':
            return 'kotlin';
        default:
            return lang;
    }
}
