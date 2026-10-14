/**
 * Loctree VSCode Extension
 *
 * Provides IDE integration for dead code detection, circular import analysis,
 * and codebase navigation powered by the loctree-lsp server.
 *
 * 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Vetcoders
 */

import * as vscode from 'vscode';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import * as crypto from 'crypto';
import { ExecuteCommandRequest, LanguageClient } from 'vscode-languageclient/node';
import { createLanguageClient, resolveServerBinary, startClient, stopClient } from './client';
import {
    createStatusBarItem,
    updateStatusBar,
    StatusBarState,
    updateStatusBarFromHealth,
    updateStatusBarFromLspState,
} from './statusbar';
import { registerCommands } from './commands';
import { LoctreeFindingsTreeProvider } from './treeview';
import { ContextPillViewProvider } from './contextPill/panel';
import { getChangedFiles } from './contextPill/taskSignal';
import { LoctreeGateway, formatHealthScore, isSnapshotNotLoaded, lspStateLabel, type LspRuntimeState } from './gateway';

let client: LanguageClient | undefined;
let statusBarItem: vscode.StatusBarItem | undefined;
let autoRefreshTimer: ReturnType<typeof setTimeout> | undefined;
let findingsTreeProvider: LoctreeFindingsTreeProvider | undefined;
let lspRuntimeState: LspRuntimeState = {
    phase: 'stopped',
    message: 'Loctree LSP has not started.',
};
const gateway = new LoctreeGateway(() => client, () => lspRuntimeState);

const INITIALIZE_COMMAND = 'loctree.initialize';
const SNAPSHOT_FILE = 'snapshot.json';

const AUTO_REFRESH_DEBOUNCE_MS = 1500;
// `loctree/refresh` is a fire-and-forget notification with no completion
// signal, so give the server a moment to reload the snapshot before we
// re-read health. Watcher-driven rescans use loctree/scanProgress instead.
const REFRESH_SETTLE_MS = 600;
const OPEN_ATLAS_CARD_COMMAND = 'loctree.openAtlasCard';

interface OpenAtlasCardResponse {
    ok?: boolean;
    card_path?: string;
}

function normalizeOpenAtlasCardArgs(args: unknown[]): unknown[] {
    if (args.length === 1 && Array.isArray(args[0])) {
        return args[0] as unknown[];
    }
    return args;
}

function cardPathFromResponse(response: unknown): string | undefined {
    if (typeof response !== 'object' || response === null || Array.isArray(response)) {
        return undefined;
    }

    const payload = response as OpenAtlasCardResponse;
    if (payload.ok === true && typeof payload.card_path === 'string' && payload.card_path.length > 0) {
        return payload.card_path;
    }

    return undefined;
}

function registerOpenAtlasCardCommand(
    context: vscode.ExtensionContext,
    outputChannel: vscode.OutputChannel,
    gateway: LoctreeGateway,
    getClient: () => LanguageClient | undefined
): void {
    context.subscriptions.push(
        vscode.commands.registerCommand(OPEN_ATLAS_CARD_COMMAND, async (...rawArgs: unknown[]) => {
            if (!gateway.isReady()) {
                const state = gateway.lspState();
                const message =
                    state.phase === 'error'
                        ? `Loctree LSP failed: ${state.message}`
                        : `Loctree LSP is ${lspStateLabel(state).toLowerCase()}: ${state.message}`;
                vscode.window.showWarningMessage(message);
                outputChannel.appendLine(`Skipped opening atlas card: ${message}`);
                return;
            }
            const activeClient = getClient();
            if (!activeClient) {
                const message = 'Loctree LSP client missing after ready state';
                vscode.window.showWarningMessage(message);
                outputChannel.appendLine(`Skipped opening atlas card: ${message}`);
                return;
            }

            try {
                const response = await activeClient.sendRequest(ExecuteCommandRequest.type, {
                    command: OPEN_ATLAS_CARD_COMMAND,
                    arguments: normalizeOpenAtlasCardArgs(rawArgs),
                });
                const cardPath = cardPathFromResponse(response);

                if (!cardPath) {
                    vscode.window.showWarningMessage('Loctree: atlas card command returned no card path');
                    outputChannel.appendLine(
                        `Atlas card command returned unexpected response: ${JSON.stringify(response)}`
                    );
                    return;
                }

                const document = await vscode.workspace.openTextDocument(vscode.Uri.file(cardPath));
                await vscode.window.showTextDocument(document, { preview: false });
                outputChannel.appendLine(`Opened atlas card: ${cardPath}`);
            } catch (error) {
                const message = error instanceof Error ? error.message : String(error);
                outputChannel.appendLine(`Failed to open atlas card: ${message}`);
                vscode.window.showErrorMessage(`Loctree: failed to open atlas card (${message})`);
            }
        })
    );
}

function formatError(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
}

function setLspRuntimeState(state: LspRuntimeState, outputChannel?: vscode.OutputChannel): void {
    lspRuntimeState = state;
    if (statusBarItem) {
        updateStatusBarFromLspState(statusBarItem, state);
    }
    findingsTreeProvider?.refresh();
    outputChannel?.appendLine(
        `Loctree LSP state: ${lspStateLabel(state)} - ${state.message}` +
        (state.serverCommand ? ` (${state.serverCommand})` : '') +
        (state.detail ? ` ${state.detail}` : '')
    );
}

function startupErrorState(serverCommand: string | undefined, error: unknown): LspRuntimeState {
    return {
        phase: 'error',
        message: `Failed to start loctree-lsp${serverCommand ? ` at ${serverCommand}` : ''}: ${formatError(error)}`,
        serverCommand,
        detail: error instanceof Error && error.stack ? error.stack.split('\n').slice(1, 3).join(' ') : undefined,
    };
}

async function refreshStatusBarFromHealth(
    outputChannel: vscode.OutputChannel
): Promise<void> {
    if (!statusBarItem) {
        return;
    }
    // Don't override an existing state (Initializing/Error) when the LSP is not
    // ready — that would mask a startup failure as a normal "Ready" bar.
    if (!gateway.isReady()) {
        return;
    }
    try {
        const health = await gateway.health(false);
        updateStatusBarFromHealth(statusBarItem, health, lspRuntimeState);
        outputChannel.appendLine(
            `Status updated (health ${formatHealthScore(health.health_score)} ${health.status}): ` +
            `${health.dead_exports} dead, ${health.cycles} cycles, ${health.twins} twins, ${health.hotspots} hotspots`
        );
    } catch (error) {
        if (isSnapshotNotLoaded(error)) {
            // Transient: initial scan still running. Show analyzing, don't error.
            updateStatusBar(statusBarItem, StatusBarState.Analyzing);
            return;
        }
        const message = error instanceof Error ? error.message : String(error);
        outputChannel.appendLine(`Failed to refresh status from LSP health: ${message}`);
        // Leave the current state as-is rather than masking with "Ready".
    }
}

/** Re-read health and refresh both the status bar and the findings tree. */
async function applyHealthToUi(outputChannel: vscode.OutputChannel): Promise<void> {
    await refreshStatusBarFromHealth(outputChannel);
    findingsTreeProvider?.refresh();
}

/**
 * Ask the server to reload its snapshot, then refresh the UI. Keeps the
 * Analyzing state visible until the snapshot settles (loctree/refresh gives no
 * completion signal), so the user sees that work is in progress.
 */
function triggerRefresh(outputChannel: vscode.OutputChannel): void {
    if (statusBarItem && gateway.isReady()) {
        updateStatusBar(statusBarItem, StatusBarState.Analyzing);
    }
    gateway.refresh();
    setTimeout(() => {
        void applyHealthToUi(outputChannel);
    }, REFRESH_SETTLE_MS);
}

function isInsidePath(root: string, candidate: string): boolean {
    const normalizedRoot = path.normalize(root);
    const normalizedCandidate = path.normalize(candidate);
    const relativePath = path.relative(normalizedRoot, normalizedCandidate);
    return (
        relativePath === '' ||
        (!relativePath.startsWith('..') && !path.isAbsolute(relativePath))
    );
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

function safeChildPath(root: string, ...segments: string[]): string | null {
    const parts: string[] = [];
    for (const segment of segments) {
        const segmentParts = safeRelativeParts(segment);
        if (!segmentParts) {
            return null;
        }
        parts.push(...segmentParts);
    }

    const normalizedRoot = path.normalize(root);
    const child = path.normalize(vscode.Uri.joinPath(vscode.Uri.file(normalizedRoot), ...parts).fsPath);
    return isInsidePath(normalizedRoot, child) ? child : null;
}

/**
 * Resolve the loctree global cache base directory, mirroring
 * `loctree-rs::snapshot::cache_base_dir`:
 *   1. `LOCT_CACHE_DIR` env override
 *   2. platform cache dir + "loctree"
 *   3. (Rust falls back to CWD `.loctree`; we just return null so the caller
 *      relies on the workspace `.loctree` check instead.)
 */
function cacheBaseDir(): string | null {
    const custom = process.env.LOCT_CACHE_DIR?.trim();
    if (custom) {
        return custom;
    }
    const home = os.homedir();
    if (!home) {
        return null;
    }
    if (process.platform === 'darwin') {
        return path.join(home, 'Library', 'Caches', 'loctree');
    }
    if (process.platform === 'win32') {
        const localAppData = process.env.LOCALAPPDATA?.trim();
        const base = localAppData || path.join(home, 'AppData', 'Local');
        return path.join(base, 'loctree');
    }
    // Linux / other unix: $XDG_CACHE_HOME/loctree, else ~/.cache/loctree
    const xdg = process.env.XDG_CACHE_HOME?.trim();
    const base = xdg || path.join(home, '.cache');
    return path.join(base, 'loctree');
}

/**
 * Resolve the per-project cache dir, mirroring
 * `loctree-rs::snapshot::project_cache_dir`. project_id is the first 16 hex
 * chars of SHA-256(canonical_project_root). We approximate canonicalization
 * with `fs.realpathSync` (falls back to the raw path when it cannot resolve).
 */
function projectCacheDir(root: string): string | null {
    let canonical = root;
    try {
        canonical = fs.realpathSync(root);
    } catch {
        // keep raw root
    }

    const custom = process.env.LOCT_CACHE_DIR?.trim();
    const projectId = crypto
        .createHash('sha256')
        .update(canonical)
        .digest('hex')
        .slice(0, 16);
    if (!/^[0-9a-f]{16}$/.test(projectId)) {
        return null;
    }

    if (custom) {
        if (custom.includes('\0')) {
            return null;
        }
        if (!path.isAbsolute(custom)) {
            return safeChildPath(canonical, custom);
        }
        return safeChildPath(custom, 'projects', projectId);
    }

    const base = cacheBaseDir();
    if (!base) {
        return null;
    }
    return safeChildPath(base, 'projects', projectId);
}

/**
 * A "project signal" means loctree already considers this workspace a project:
 * either a workspace-local `.loctree/` folder, or a cached snapshot in the
 * per-project global cache. Absent both, an autoscan would scan a random large
 * workspace from scratch — which we avoid unless explicitly opted in.
 */
async function hasProjectSignal(
    workspaceFolder: vscode.Uri,
    workspaceRoot: string
): Promise<boolean> {
    const loctreeFolder = vscode.Uri.joinPath(workspaceFolder, '.loctree');
    try {
        await vscode.workspace.fs.stat(loctreeFolder);
        return true;
    } catch {
        // no workspace-local .loctree
    }

    const cacheDir = projectCacheDir(workspaceRoot);
    if (cacheDir) {
        try {
            const snapshotPath = safeChildPath(cacheDir, SNAPSHOT_FILE);
            if (snapshotPath && fs.existsSync(snapshotPath)) {
                return true;
            }
        } catch {
            // ignore probe failures; treat as no signal
        }
    }

    return false;
}

/**
 * Show the "inactive" status: the extension is loaded but the LSP is not
 * started (no project signal + autoscan disabled), so no heavy scan was
 * triggered. The user can opt in via Loctree: Initialize/Scan.
 */
function setInactiveStatus(): void {
    if (!statusBarItem) {
        return;
    }
    statusBarItem.text = '$(circle-slash) Loctree inactive';
    statusBarItem.tooltip = 'Loctree inactive — run Loctree: Initialize/Scan';
    statusBarItem.command = INITIALIZE_COMMAND;
    statusBarItem.backgroundColor = undefined;
    statusBarItem.show();
}

/**
 * Resolve the binary, create the language client, start it, and wire status +
 * notifications. Idempotent: if a client is already running, this is a no-op.
 * Starting the client is what triggers the server-side `initialized` →
 * `load_snapshot` → auto-scan, so this is the single gate for the heavy scan.
 */
async function startLspClient(
    context: vscode.ExtensionContext,
    outputChannel: vscode.OutputChannel,
    workspaceRoot: string
): Promise<void> {
    if (gateway.isReady()) {
        return;
    }
    if (gateway.lspState().phase === 'starting') {
        return;
    }

    if (statusBarItem) {
        statusBarItem.command = 'loctree.showHealth';
    }
    setLspRuntimeState(
        {
            phase: 'starting',
            message: 'Resolving loctree-lsp binary.',
        },
        outputChannel
    );

    let resolvedServerCommand: string | undefined;
    let resolvedServerVersion: string | undefined;
    let resolutionSource: string | undefined;
    let runtimeWarning: string | undefined;
    try {
        const runtime = await resolveServerBinary(context, outputChannel);
        resolvedServerCommand = runtime.command;
        resolvedServerVersion = runtime.version;
        resolutionSource = runtime.source;
        runtimeWarning = runtime.warning;
        outputChannel.appendLine(
            `Resolved loctree-lsp: ${runtime.command} | ${runtime.version} | source=${runtime.source}`
        );
        if (runtime.warning) {
            outputChannel.appendLine(runtime.warning);
            void vscode.window.showWarningMessage(runtime.warning);
        }
        setLspRuntimeState(
            {
                phase: 'starting',
                message: 'Starting loctree-lsp and waiting for initialize handshake.',
                serverCommand: resolvedServerCommand,
                serverVersion: resolvedServerVersion,
                resolutionSource,
                runtimeWarning,
            },
            outputChannel
        );

        const nextClient = createLanguageClient(context, outputChannel, resolvedServerCommand, workspaceRoot);
        let handshakeComplete = false;

        nextClient.onDidChangeState((event: { newState: number }) => {
            if (event.newState === 1) { // Stopped
                if (client === nextClient) {
                    client = undefined;
                }
                setLspRuntimeState(
                    {
                        phase: 'stopped',
                        message: 'Loctree LSP stopped.',
                        serverCommand: resolvedServerCommand,
                        serverVersion: resolvedServerVersion,
                        resolutionSource,
                        runtimeWarning,
                    },
                    outputChannel
                );
            } else if (event.newState === 2 && handshakeComplete && client === nextClient) { // Running
                setLspRuntimeState(
                    {
                        phase: 'running',
                        message: 'Initialize handshake completed.',
                        serverCommand: resolvedServerCommand,
                        serverVersion: resolvedServerVersion,
                        resolutionSource,
                        runtimeWarning,
                    },
                    outputChannel
                );
                void applyHealthToUi(outputChannel);
            }
        });

        await startClient(nextClient);
        handshakeComplete = true;
        client = nextClient;
        outputChannel.appendLine('Loctree LSP client started successfully');

        setLspRuntimeState(
            {
                phase: 'running',
                message: 'Initialize handshake completed.',
                serverCommand: resolvedServerCommand,
                serverVersion: resolvedServerVersion,
                resolutionSource,
                runtimeWarning,
            },
            outputChannel
        );

        // React to server-side rescans (file watcher) instead of polling a
        // disk file. The server emits scanning → done / failed.
        nextClient.onNotification('loctree/scanProgress', (params: { phase?: string; message?: string }) => {
            const phase = params?.phase;
            if (phase === 'scanning' || phase === 'composing') {
                if (statusBarItem) {
                    updateStatusBar(statusBarItem, StatusBarState.Analyzing);
                }
            } else if (phase === 'done') {
                void applyHealthToUi(outputChannel);
            } else if (phase === 'failed') {
                outputChannel.appendLine(`Loctree scan failed: ${params?.message ?? 'unknown'}`);
                if (statusBarItem) {
                    updateStatusBar(statusBarItem, StatusBarState.Error);
                }
            }
        });

        // Initial UI load — only on a successful start, so a startup failure
        // (handled in catch) keeps the Error state instead of being masked.
        void applyHealthToUi(outputChannel);
    } catch (error) {
        outputChannel.appendLine(`Failed to start LSP client: ${formatError(error)}`);
        if (client) {
            void stopClient(client).catch((stopError) => {
                outputChannel.appendLine(`Failed to stop partially started LSP client: ${formatError(stopError)}`);
            });
        }
        client = undefined;
        setLspRuntimeState(startupErrorState(resolvedServerCommand, error), outputChannel);
    }
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
    const outputChannel = vscode.window.createOutputChannel('Loctree');
    outputChannel.appendLine('Loctree extension activating...');

    // Check if .loctree folder exists
    const workspaceFolders = vscode.workspace.workspaceFolders;
    if (!workspaceFolders || workspaceFolders.length === 0) {
        outputChannel.appendLine('No workspace folder found');
        return;
    }

    const workspaceRoot = workspaceFolders[0].uri.fsPath;

    // Create status bar item
    const config = vscode.workspace.getConfiguration('loctree');
    if (config.get<boolean>('showStatusBar', true)) {
        statusBarItem = createStatusBarItem();
        context.subscriptions.push(statusBarItem);
        updateStatusBar(statusBarItem, StatusBarState.Initializing);
    }

    // Register commands
    registerCommands(context, outputChannel, workspaceRoot, gateway, () =>
        triggerRefresh(outputChannel)
    );
    registerOpenAtlasCardCommand(context, outputChannel, gateway, () => client);

    // Initialize/Scan: start the LSP on demand (used when the workspace had no
    // project signal at activation and autoscan-on-startup was disabled).
    context.subscriptions.push(
        vscode.commands.registerCommand(INITIALIZE_COMMAND, async () => {
            if (gateway.isReady()) {
                triggerRefresh(outputChannel);
                return;
            }
            outputChannel.appendLine('Loctree: Initialize/Scan requested — starting LSP client');
            await startLspClient(context, outputChannel, workspaceRoot);
        })
    );

    findingsTreeProvider = new LoctreeFindingsTreeProvider(workspaceRoot, outputChannel, gateway);
    const findingsTreeView = vscode.window.createTreeView('loctree.findings', {
        treeDataProvider: findingsTreeProvider,
        showCollapseAll: true,
    });
    context.subscriptions.push(findingsTreeView);

    // Context Pill — the "loctree.context" view is now a WEBVIEW (replacing the
    // old query-driven tree router from contextPanel.ts). It binds ambiently to
    // the active editor and assembles a Context Pill view model over the real
    // gateway. Findings stays the demoted legacy tree summary above.
    const pill = new ContextPillViewProvider(context, gateway, () => ({
        changedFiles: getChangedFiles(),
        selection:
            vscode.window.activeTextEditor?.document.getText(
                vscode.window.activeTextEditor.selection
            ) ?? '',
    }));
    context.subscriptions.push(
        vscode.window.registerWebviewViewProvider(ContextPillViewProvider.viewId, pill),
        vscode.window.onDidChangeActiveTextEditor(() => void pill.rescopeToActiveEditor()),
        vscode.commands.registerCommand('loctree.copyAgentContext', () =>
            pill.copyAgentContextForCurrent()
        ),
        // `loctree.contextQuery` is the status bar item's command (statusbar.ts).
        // The old tree router that registered it is gone; alias it to focusing
        // the Context Pill view + rescoping so the status bar click stays live.
        vscode.commands.registerCommand('loctree.contextQuery', async () => {
            await vscode.commands.executeCommand('loctree.context.focus');
            await pill.rescopeToActiveEditor();
        })
    );

    // Decide whether to start the LSP (and thus the server-side auto-scan) now.
    // A heavy scan of a random large workspace is only acceptable when there is
    // already a project signal (.loctree / cached snapshot) OR the user opted in
    // via loctree.autoScanOnStartup. Otherwise stay inactive until the user runs
    // Loctree: Initialize/Scan.
    const autoScanOnStartup = config.get<boolean>('autoScanOnStartup', false);
    const projectSignal = await hasProjectSignal(workspaceFolders[0].uri, workspaceRoot);

    if (projectSignal || autoScanOnStartup) {
        outputChannel.appendLine(
            projectSignal
                ? 'Loctree project signal found (.loctree or cached snapshot); starting LSP client'
                : 'loctree.autoScanOnStartup enabled; starting LSP client'
        );
        await startLspClient(context, outputChannel, workspaceRoot);
    } else {
        outputChannel.appendLine(
            'No .loctree / cached snapshot and autoScanOnStartup is false; staying inactive. ' +
            'Run Loctree: Initialize/Scan to activate.'
        );
        setLspRuntimeState(
            {
                phase: 'stopped',
                message: 'No project signal and autoScanOnStartup is false. Run Loctree: Initialize/Scan.',
            },
            outputChannel
        );
        setInactiveStatus();
    }

    // Auto-refresh on save if enabled. Server-driven rescans arrive via
    // loctree/scanProgress; this nudges a reload for the saved document.
    context.subscriptions.push(
        vscode.workspace.onDidSaveTextDocument(() => {
            const config = vscode.workspace.getConfiguration('loctree');
            if (!config.get<boolean>('autoRefresh', false)) {
                return;
            }
            if (autoRefreshTimer) {
                clearTimeout(autoRefreshTimer);
            }
            autoRefreshTimer = setTimeout(() => {
                triggerRefresh(outputChannel);
            }, AUTO_REFRESH_DEBOUNCE_MS);
        })
    );

    context.subscriptions.push(outputChannel);

    outputChannel.appendLine('Loctree extension activated');
}

export async function deactivate(): Promise<void> {
    if (client) {
        await stopClient(client);
    }
    client = undefined;
    lspRuntimeState = {
        phase: 'stopped',
        message: 'Loctree LSP stopped.',
    };
}
