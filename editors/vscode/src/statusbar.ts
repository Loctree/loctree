/**
 * Status Bar for Loctree
 *
 * Shows the Loctree Context-King entry point in the VS Code status bar.
 *
 * 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Vetcoders
 */

import * as vscode from 'vscode';
import { formatHealthScore, lspStateLabel, type HealthResponse, type LspRuntimeState } from './gateway';

export enum StatusBarState {
    Initializing = 'initializing',
    Ready = 'ready',
    Analyzing = 'analyzing',
    Healthy = 'healthy',
    Error = 'error',
    Stopped = 'stopped',
}

/**
 * Create the loctree status bar item
 */
export function createStatusBarItem(): vscode.StatusBarItem {
    const item = vscode.window.createStatusBarItem(
        vscode.StatusBarAlignment.Right,
        100
    );

    item.name = 'Loctree';
    item.command = 'loctree.contextQuery';
    item.tooltip = 'Loctree Context: one-shot full repository context for AI agents';

    return item;
}

/**
 * Update status bar with current state
 */
export function updateStatusBar(
    item: vscode.StatusBarItem,
    state: StatusBarState
): void {
    switch (state) {
        case StatusBarState.Initializing:
            item.text = '$(loading~spin) Loctree';
            item.tooltip = 'Loctree is preparing repository context...';
            item.backgroundColor = undefined;
            break;

        case StatusBarState.Ready:
            item.text = '$(type-hierarchy) Loctree';
            item.tooltip = 'Loctree Context is ready. Agents go BIG when they see.';
            item.backgroundColor = undefined;
            break;

        case StatusBarState.Analyzing:
            item.text = '$(sync~spin) Loctree';
            item.tooltip = 'Building repository context...';
            item.backgroundColor = undefined;
            break;

        case StatusBarState.Healthy:
            item.text = '$(type-hierarchy) Loctree Context';
            item.tooltip = 'One-shot full repository context for AI agents';
            item.backgroundColor = undefined;
            break;

        case StatusBarState.Error:
            item.text = '$(error) Loctree';
            item.tooltip = 'Loctree context is unavailable. Run Loctree: Initialize/Scan.';
            item.backgroundColor = new vscode.ThemeColor(
                'statusBarItem.errorBackground'
            );
            break;

        case StatusBarState.Stopped:
            item.text = '$(type-hierarchy) Loctree';
            item.tooltip = 'Loctree inactive. Run Loctree: Initialize/Scan.';
            item.backgroundColor = undefined;
            break;
    }

    item.show();
}

export function updateStatusBarFromLspState(
    item: vscode.StatusBarItem,
    state: LspRuntimeState
): void {
    const runtime = state.serverCommand
        ? `\nBinary: ${state.serverCommand}` +
        (state.serverVersion ? `\nIdentity: ${state.serverVersion}` : '') +
        (state.resolutionSource ? `\nSource: ${state.resolutionSource}` : '') +
        (state.runtimeWarning ? `\nWarning: ${state.runtimeWarning}` : '')
        : '';
    switch (state.phase) {
        case 'starting':
            item.text = '$(loading~spin) Loctree';
            item.tooltip = `Loctree LSP: ${lspStateLabel(state)}. ${state.message}${runtime}`;
            item.backgroundColor = undefined;
            break;

        case 'running':
            item.text = '$(type-hierarchy) Loctree';
            item.tooltip = `Loctree LSP: ${lspStateLabel(state)}. ${state.message}${runtime}`;
            item.backgroundColor = undefined;
            break;

        case 'error': {
            const detail = state.detail ? `\n${state.detail}` : '';
            item.text = '$(error) Loctree';
            item.tooltip = `Loctree LSP: Error. ${state.message}${runtime}${detail}`;
            item.backgroundColor = new vscode.ThemeColor('statusBarItem.errorBackground');
            break;
        }

        case 'stopped':
            item.text = '$(circle-slash) Loctree inactive';
            item.tooltip = `Loctree LSP: ${lspStateLabel(state)}. ${state.message}${runtime}`;
            item.backgroundColor = undefined;
            break;
    }

    item.show();
}

/**
 * Rich status bar update from a loctree/health response. Context remains the
 * primary click target; health/finding counts are secondary telemetry in the
 * tooltip so the status bar does not regress to a findings-first surface.
 */
export function updateStatusBarFromHealth(
    item: vscode.StatusBarItem,
    health: HealthResponse,
    runtimeState?: LspRuntimeState,
): void {
    const total = health.dead_exports + health.cycles + health.twins;
    const staleMark = health.snapshot_stale ? ' $(history)' : '';
    item.command = 'loctree.contextQuery';
    item.text = `$(type-hierarchy) Loctree Context${staleMark}`;

    const tooltip = new vscode.MarkdownString(undefined, true);
    tooltip.appendMarkdown('**Loctree Context**\n\n');
    tooltip.appendMarkdown('One-shot full repository context for AI agents.\n\n');
    tooltip.appendMarkdown(`Findings signal: ${formatHealthScore(health.health_score)} (${health.status})\n\n`);
    if (runtimeState?.serverCommand) {
        tooltip.appendMarkdown(`Runtime: \`${runtimeState.serverCommand}\`\n\n`);
        if (runtimeState.serverVersion) tooltip.appendMarkdown(`Identity: ${runtimeState.serverVersion}\n\n`);
        if (runtimeState.resolutionSource) tooltip.appendMarkdown(`Source: ${runtimeState.resolutionSource}\n\n`);
        if (runtimeState.runtimeWarning) tooltip.appendMarkdown(`Warning: ${runtimeState.runtimeWarning}\n\n`);
    }
    tooltip.appendMarkdown(
        `Dead: ${health.dead_exports} · Cycles: ${health.cycles} · ` +
        `Twins: ${health.twins} · Hotspots: ${health.hotspots}\n`
    );
    if (health.snapshot_stale) {
        tooltip.appendMarkdown(`\n$(history) Snapshot is stale relative to HEAD — refresh.\n`);
    }
    if (health.recommended_actions.length > 0) {
        tooltip.appendMarkdown(
            `\n**Recommended:**\n${health.recommended_actions.map((a) => `- ${a}`).join('\n')}\n`
        );
    }
    tooltip.appendMarkdown('\n_Click to open Context Query._');
    item.tooltip = tooltip;

    if (health.status === 'red') {
        item.backgroundColor = new vscode.ThemeColor('statusBarItem.errorBackground');
    } else if (health.status === 'yellow' || total > 0) {
        item.backgroundColor = new vscode.ThemeColor('statusBarItem.warningBackground');
    } else {
        item.backgroundColor = undefined;
    }

    item.show();
}
