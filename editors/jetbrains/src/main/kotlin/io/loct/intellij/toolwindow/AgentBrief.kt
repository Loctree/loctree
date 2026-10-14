/*
 * Format projected Loctree results for paste into an agent session.
 *
 * Pure string builder — no Swing/IDE deps. Clipboard is the only side effect
 * (caller). Paths with file= become loct-ready lines.
 *
 * 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI
 */

package io.loct.intellij.toolwindow

import io.loct.intellij.protocol.HealthResponse
import io.loct.intellij.protocol.RiskItem

internal object AgentBrief {

    fun fromProjected(method: String, projected: ProjectedResult): String = buildString {
        appendLine("## Loctree · ${method.removePrefix("loctree/")}")
        appendLine()
        appendLine(projected.headline)
        projected.summaries.forEach { appendLine("- $it") }
        if (projected.summaries.isNotEmpty()) appendLine()
        for (section in projected.sections) {
            appendLine("### ${section.title}")
            if (section.items.isEmpty()) {
                appendLine(section.note ?: "_empty_")
                appendLine()
                continue
            }
            section.items.take(40).forEach { item ->
                append("- ")
                append(item.label)
                item.file?.let { f ->
                    append("  `")
                    append(f)
                    item.line?.let { append(":$it") }
                    append('`')
                }
                appendLine()
                item.description?.takeIf { it.isNotBlank() && it != item.label }?.let {
                    appendLine("  $it")
                }
            }
            appendLine()
        }
        appendLine("---")
        appendLine("Suggested next: `loct slice <file>` · `loct impact <file>` · `loct find --literal <sym>`")
    }

    fun fromFindings(data: FindingsData, health: HealthResponse? = null): String = buildString {
        appendLine("## Loctree · findings signal")
        appendLine()
        if (health != null) {
            appendLine("Health **${health.healthScoreLabel}** · ${health.status}")
            appendLine(
                "dead ${health.deadExports} · cycles ${health.cycles} · twins ${health.twins} · hotspots ${health.hotspots}",
            )
            appendLine()
        }
        for (kind in GroupKind.entries) {
            val count = data.counts.getValue(kind)
            val items = data.groups.getValue(kind)
            appendLine("### ${kind.name.lowercase()} ($count)")
            if (items.isEmpty()) {
                appendLine("_clean_")
                appendLine()
                continue
            }
            items.take(30).forEach { item ->
                append("- ")
                append(item.label)
                item.file?.let { f ->
                    append("  `")
                    append(f)
                    item.line?.let { append(":$it") }
                    append('`')
                }
                appendLine()
            }
            appendLine()
        }
        health?.topRisks?.takeIf { it.isNotEmpty() }?.let { risks ->
            appendLine("### top risks")
            risks.forEach { appendRisk(it) }
            appendLine()
        }
        appendLine("---")
        appendLine("Suggested next: `loct findings --summary` · `loct follow dead` · `loct context --task '…'`")
    }

    fun fromHealth(health: HealthResponse): String = buildString {
        appendLine("## Loctree · health")
        appendLine()
        appendLine("**${health.healthScoreLabel}** · ${health.status}")
        if (health.snapshotStale) appendLine("snapshot: stale (age ${health.snapshotAgeSeconds}s)")
        appendLine(
            "dead ${health.deadExports} · cycles ${health.cycles} · twins ${health.twins} · hotspots ${health.hotspots}",
        )
        appendLine()
        if (health.topRisks.isNotEmpty()) {
            appendLine("### top risks")
            health.topRisks.forEach { appendRisk(it) }
            appendLine()
        }
        if (health.recommendedActions.isNotEmpty()) {
            appendLine("### recommended")
            health.recommendedActions.forEach { appendLine("- $it") }
            appendLine()
        }
        appendLine("---")
        appendLine("Suggested next: `loct findings --summary` · `loct context --full --markdown`")
    }

    private fun StringBuilder.appendRisk(risk: RiskItem) {
        append("- [")
        append(risk.severity.ifBlank { "?" })
        append("] ")
        append(risk.message.ifBlank { risk.kind })
        if (risk.file.isNotBlank()) {
            append("  `")
            append(risk.file)
            append('`')
        }
        appendLine()
    }
}
