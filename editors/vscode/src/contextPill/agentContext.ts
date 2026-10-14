import { formatHealthScore } from '../gateway';
import type { ContextPillViewModel } from './viewModel';
import { scopeKey } from './scope';

export interface TaskSignal { changedFiles: string[]; selection: string }

/** Infer the task from IDE signals (WIP diff primary, selection refinement).
 * Returns '' when there is no signal — never fabricate. The caller shows this
 * as editable provenance before copying ("not creepy" contract). */
export function inferTask(sig: TaskSignal): string {
  if (sig.selection.trim()) {
    return `working on: ${sig.selection.trim().slice(0, 120)}`;
  }
  if (sig.changedFiles.length) {
    return `editing ${sig.changedFiles.slice(0, 5).join(', ')}`;
  }
  return '';
}

/** Human-readable label for a risk kind (mirrors the webview's humanizeKind). */
function humanizeKind(kind: string): string {
  const map: Record<string, string> = {
    cycle: 'Cycle',
    dead_export: 'Dead export',
    twin: 'Twin',
    hotspot: 'Hotspot',
  };
  if (map[kind]) return map[kind];
  return kind ? kind.charAt(0).toUpperCase() + kind.slice(1) : '';
}

export function buildAgentContextMarkdown(vm: ContextPillViewModel, task: string): string {
  const lines: string[] = [];
  lines.push(`# Loctree context — ${scopeKey(vm.scope)}`);
  lines.push(`scope=${scopeKey(vm.scope)}`);
  if (task) lines.push(`task: ${task}`);
  lines.push('');

  // Literal scope is a string-match surface, not a structural target: the
  // markdown lists the grouped occurrences instead of blast/exports/findings.
  if (vm.scope.kind === 'literal') {
    lines.push(`## Literal occurrences — "${vm.scope.value}"`);
    lines.push(`${vm.literalTotal} occurrence(s) across ${vm.literalGroups.length} file(s)${vm.literalHasMore ? ' (showing first page)' : ''}\n`);
    for (const g of vm.literalGroups) {
      lines.push(`- ${g.file}: ${g.lines.join(', ')}`);
    }
    return lines.join('\n');
  }

  if (vm.summary) lines.push(`## What it does\n${vm.summary}\n`);
  lines.push(`## Blast radius (risk of change)\nA change here can touch ${vm.blastRadius.count} file(s): ${vm.blastRadius.direct.join(', ')}\n`);
  if (vm.exports.length) lines.push(`## Exports\n${vm.exports.map((e) => `- ${e.kind} ${e.name}`).join('\n')}\n`);
  if (vm.deps.length) lines.push(`## Depends on\n${vm.deps.map((d) => `- ${d}`).join('\n')}\n`);
  if (vm.bodyPreview?.found) lines.push(`## Body (${vm.bodyPreview.file})\n\`\`\`\n${vm.bodyPreview.preview}\n\`\`\`\n`);

  // Findings are honest + scope-aware. Emit the PER-FILE risks (the same set
  // the webview surfaces under "FINDINGS · THIS FILE"), then SEPARATE repo-wide
  // totals so the agent never mistakes repo numbers for file numbers.
  const findingsBody = vm.fileRisks.length
    ? vm.fileRisks
        .slice(0, 5)
        .map((r) => `- ${humanizeKind(r.kind)} — ${r.message}`)
        .join('\n')
    : 'No issues in this file';
  lines.push(`## Findings\n${findingsBody}`);
  lines.push(
    `Repo-wide: ${vm.repoFindings.hotspots} hotspots · ${vm.repoFindings.dead} dead · ${vm.repoFindings.cycles} cycles`,
  );
  lines.push(`Repo health: ${formatHealthScore(vm.repoHealth)}`);
  return lines.join('\n');
}
