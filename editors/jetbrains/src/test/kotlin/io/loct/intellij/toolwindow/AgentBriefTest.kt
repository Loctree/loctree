/*
 * Agent brief formatting tests.
 *
 * 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI
 */

package io.loct.intellij.toolwindow

import com.google.gson.JsonParser
import io.loct.intellij.protocol.HealthResponse
import io.loct.intellij.protocol.RiskItem
import org.junit.Assert.assertTrue
import org.junit.Test

class AgentBriefTest {

    @Test
    fun healthBriefDoesNotInventAScoreForAnEmptySnapshot() {
        val brief = AgentBrief.fromHealth(HealthResponse(healthScore = null, status = "unknown"))
        assertTrue(brief.contains("**unknown**"))
        assertTrue(!brief.contains("null/100") && !brief.contains("0/100"))
    }

    @Test
    fun projectedBriefIncludesPathsForSwiftAndTs() {
        val page = JsonParser.parseString(
            """
            {
              "literal_matches": {
                "data": [
                  {"file":"macos/App.swift","line":12,"symbol":"body"},
                  {"file":"src/auth.ts","line":3,"name":"login"}
                ]
              }
            }
            """.trimIndent(),
        )
        val projected = ResultProjector.project("loctree/find", page)
        assertTrue(projected.sections.first().items.all { it.file != null })
        val brief = AgentBrief.fromProjected("loctree/find", projected)
        assertTrue(brief.contains("App.swift"))
        assertTrue(brief.contains("auth.ts"))
        assertTrue(brief.contains("Suggested next"))
    }

    @Test
    fun healthBriefIncludesScoreAndRisks() {
        val health = HealthResponse(
            healthScore = 71,
            status = "yellow",
            deadExports = 3,
            cycles = 1,
            twins = 0,
            hotspots = 2,
            topRisks = listOf(
                RiskItem("dead_export", "src/a.py", "high", "unused helper"),
            ),
            recommendedActions = listOf("loct follow dead"),
        )
        val brief = AgentBrief.fromHealth(health)
        assertTrue(brief.contains("71/100"))
        assertTrue(brief.contains("a.py"))
        assertTrue(brief.contains("loct follow dead"))
    }

    @Test
    fun stringArraySwiftPathsGetFileField() {
        val page = JsonParser.parseString(
            """{"deps":["Sources/App/MainWindowController.swift","lib/foo.ts"]}""",
        )
        val projected = ResultProjector.project("loctree/slice", page)
        val files = projected.sections.flatMap { it.items }.mapNotNull { it.file }
        assertTrue(files.any { it.endsWith(".swift") })
        assertTrue(files.any { it.endsWith(".ts") })
    }
}
