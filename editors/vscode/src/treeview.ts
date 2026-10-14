/**
 * Loctree Findings Tree View (secondary signal)
 *
 * Sidebar summary sourced from the loctree LSP (loctree/health), NOT from
 * disk. loctree 0.12 writes artifacts to the per-project cache dir, not the
 * workspace `.loctree/`, so reading `.loctree/findings.json` showed nothing.
 * The LSP loads the snapshot from the cache and answers loctree/health with
 * live counts + top_risks drill-down.
 *
 * 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Vetcoders
 */

import * as path from 'path';
import * as vscode from 'vscode';
import { HealthResponse, LoctreeGateway, RiskItem, formatHealthScore, isSnapshotNotLoaded, lspStateLabel } from './gateway';

type GroupKind = 'dead' | 'cycles' | 'twins' | 'hotspots';

const GROUP_ORDER: GroupKind[] = ['dead', 'cycles', 'twins', 'hotspots'];

const GROUP_TITLE: Record<GroupKind, string> = {
    dead: 'Dead Exports',
    cycles: 'Circular Imports',
    twins: 'Twins',
    hotspots: 'Hotspots',
};

const GROUP_ICON: Record<GroupKind, string> = {
    dead: 'warning',
    cycles: 'sync-ignored',
    twins: 'copy',
    hotspots: 'flame',
};

const GROUP_COLOR: Record<GroupKind, string> = {
    dead: 'loctree.danger',
    cycles: 'loctree.amber',
    twins: 'loctree.teal',
    hotspots: 'loctree.amber',
};

/** Map a health top_risk `kind` onto a tree group. */
function riskKindToGroup(kind: string): GroupKind | undefined {
    switch (kind) {
        case 'dead_export':
            return 'dead';
        case 'cycle':
            return 'cycles';
        case 'twin':
            return 'twins';
        case 'hotspot':
            return 'hotspots';
        default:
            return undefined;
    }
}

interface HealthNode {
    type: 'health';
}
interface GroupNode {
    type: 'group';
    kind: GroupKind;
}
interface RiskNode {
    type: 'risk';
    kind: GroupKind;
    risk: RiskItem;
}
interface InfoNode {
    type: 'info';
    kind: GroupKind;
    label: string;
    description?: string;
}

type TreeNode = HealthNode | GroupNode | RiskNode | InfoNode;

function toDisplayPath(filePath: string, workspaceRoot: string): string {
    let candidate = filePath;
    if (candidate.startsWith('file://')) {
        candidate = vscode.Uri.parse(candidate).fsPath;
    }
    if (path.isAbsolute(candidate)) {
        const relative = path.relative(workspaceRoot, candidate);
        if (relative.length > 0 && !relative.startsWith('..') && !path.isAbsolute(relative)) {
            return relative;
        }
        return candidate;
    }
    return candidate;
}

function countForGroup(health: HealthResponse, kind: GroupKind): number {
    switch (kind) {
        case 'dead':
            return health.dead_exports;
        case 'cycles':
            return health.cycles;
        case 'twins':
            return health.twins;
        case 'hotspots':
            return health.hotspots;
    }
}

export class LoctreeFindingsTreeProvider implements vscode.TreeDataProvider<TreeNode> {
    private readonly onDidChangeEmitter = new vscode.EventEmitter<TreeNode | undefined | void>();
    private health: HealthResponse | undefined;
    private error: string | undefined;
    private loaded = false;
    private retriedNotLoaded = false;
    private static readonly NOT_LOADED_RETRY_MS = 800;

    public readonly onDidChangeTreeData = this.onDidChangeEmitter.event;

    constructor(
        private readonly workspaceRoot: string,
        private readonly outputChannel: vscode.OutputChannel,
        private readonly gateway: LoctreeGateway
    ) { }

    public refresh(): void {
        this.loaded = false;
        this.onDidChangeEmitter.fire();
    }

    private risksForGroup(kind: GroupKind): RiskItem[] {
        if (!this.health) {
            return [];
        }
        return this.health.top_risks.filter((risk) => riskKindToGroup(risk.kind) === kind);
    }

    public getTreeItem(element: TreeNode): vscode.TreeItem {
        if (element.type === 'health') {
            const health = this.health;
            const label = `Findings Signal: ${formatHealthScore(health?.health_score ?? null)}`;
            const item = new vscode.TreeItem(label, vscode.TreeItemCollapsibleState.None);
            const status = health?.status ?? 'unknown';
            item.description = health?.snapshot_stale ? `${status} · snapshot stale` : status;
            const colorId =
                status === 'green' ? 'loctree.teal' : status === 'yellow' ? 'loctree.amber' : status === 'red' ? 'loctree.danger' : 'disabledForeground';
            item.iconPath = new vscode.ThemeIcon('pulse', new vscode.ThemeColor(colorId));
            item.contextValue = 'loctreeHealth';
            item.tooltip =
                health?.recommended_actions?.length
                    ? `Recommended:\n${health.recommended_actions.join('\n')}`
                    : 'Legacy findings summary; use Loctree Context for agent-ready repository context';
            item.command = { command: 'loctree.showHealth', title: 'Show Findings Health' };
            return item;
        }

        if (element.type === 'group') {
            const count = this.health ? countForGroup(this.health, element.kind) : 0;
            const item = new vscode.TreeItem(
                `${GROUP_TITLE[element.kind]} (${count})`,
                count > 0 ? vscode.TreeItemCollapsibleState.Collapsed : vscode.TreeItemCollapsibleState.None
            );
            item.contextValue = `loctreeGroup:${element.kind}`;
            item.iconPath = new vscode.ThemeIcon(
                GROUP_ICON[element.kind],
                new vscode.ThemeColor(GROUP_COLOR[element.kind])
            );
            return item;
        }

        if (element.type === 'info') {
            const item = new vscode.TreeItem(element.label, vscode.TreeItemCollapsibleState.None);
            item.description = element.description;
            item.contextValue = `loctreeInfo:${element.kind}`;
            item.iconPath = new vscode.ThemeIcon(element.label === 'No findings' ? 'pass' : 'info');
            return item;
        }

        // risk item
        const risk = element.risk;
        const display = risk.file ? toDisplayPath(risk.file, this.workspaceRoot) : risk.message;
        const item = new vscode.TreeItem(display, vscode.TreeItemCollapsibleState.None);
        item.description = risk.message;
        item.tooltip = `${risk.severity.toUpperCase()} · ${risk.message}\n${risk.file}`;
        item.contextValue = 'loctreeFinding';
        item.iconPath =
            risk.severity === 'high'
                ? new vscode.ThemeIcon('error')
                : risk.severity === 'medium'
                    ? new vscode.ThemeIcon('warning')
                    : new vscode.ThemeIcon('circle-filled');
        if (risk.file) {
            item.command = {
                command: 'loctree.navigateToFile',
                title: 'Open Finding',
                arguments: [{ filePath: risk.file }],
            };
        }
        return item;
    }

    public async getChildren(element?: TreeNode): Promise<TreeNode[]> {
        await this.ensureLoaded();

        if (!element) {
            if (this.error) {
                return [{ type: 'info', kind: 'dead', label: this.error }];
            }
            return [
                { type: 'health' },
                ...GROUP_ORDER.map((kind) => ({ type: 'group', kind }) as GroupNode),
            ];
        }

        if (element.type !== 'group') {
            return [];
        }

        const count = this.health ? countForGroup(this.health, element.kind) : 0;
        if (count === 0) {
            return [{ type: 'info', kind: element.kind, label: 'No findings · clean', description: 'signal clear' }];
        }

        const risks = this.risksForGroup(element.kind);
        if (risks.length === 0) {
            return [
                {
                    type: 'info',
                    kind: element.kind,
                    label: `${count} findings`,
                    description: 'open report for detail',
                },
            ];
        }

        return risks.map((risk) => ({ type: 'risk', kind: element.kind, risk }));
    }

    private async ensureLoaded(): Promise<void> {
        if (this.loaded) {
            return;
        }
        this.loaded = true;
        this.error = undefined;

        if (!this.gateway.isReady()) {
            const lsp = this.gateway.lspState();
            this.health = undefined;
            this.error =
                lsp.phase === 'error'
                    ? `Loctree LSP error: ${lsp.message}`
                    : `Loctree LSP ${lspStateLabel(lsp).toLowerCase()}: ${lsp.message}`;
            return;
        }

        try {
            this.health = await this.gateway.health(true);
            this.retriedNotLoaded = false;
        } catch (error) {
            this.health = undefined;
            if (isSnapshotNotLoaded(error)) {
                // Transient: the initial scan is still in flight. Show a soft
                // analyzing state and retry once instead of a sticky failure.
                this.error = 'Loctree analyzing…';
                if (!this.retriedNotLoaded) {
                    this.retriedNotLoaded = true;
                    setTimeout(() => this.refresh(), LoctreeFindingsTreeProvider.NOT_LOADED_RETRY_MS);
                }
                return;
            }
            const message = error instanceof Error ? error.message : String(error);
            this.outputChannel.appendLine(`Failed to load health from LSP: ${message}`);
            this.error = 'Loctree: failed to load findings';
        }
    }
}
