/*
 * Paginated<T> + HealthResponse decode tests, incl. unknown-field
 * tolerance (the server may add fields without breaking the client).
 *
 * 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Vetcoders
 */

package io.loct.intellij.protocol

import com.google.gson.Gson
import com.google.gson.JsonElement
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PaginatedDecodeTest {

    private val gson = Gson()

    @Test
    fun nullHealthScoreRemainsUnknownInsteadOfZero() {
        val health = gson.fromJson("""{"health_score":null,"status":"unknown"}""", HealthResponse::class.java)
        assertNull(health.healthScore)
        assertEquals("unknown", health.healthScoreLabel)
        assertEquals("0/100", HealthResponse(healthScore = 0).healthScoreLabel)
    }

    @Test
    fun decodesSingleShotEnvelope() {
        val json = """
            {"chunk":0,"total_chunks":1,"next_cursor":null,"data":{"items":[1,2,3]}}
        """.trimIndent()
        val page = gson.fromJson<Paginated<JsonElement>>(json, PaginatedJsonType.TYPE)
        assertEquals(0, page.chunk)
        assertEquals(1, page.totalChunks)
        assertNull(page.nextCursor)
        assertFalse(page.hasMore)
        assertTrue(page.data!!.asJsonObject.has("items"))
    }

    @Test
    fun decodesChunkedEnvelopeWithCursorAndAdvisory() {
        val json = """
            {"chunk":0,"total_chunks":3,"next_cursor":"tok-50","data":[],"advisory":"truncated"}
        """.trimIndent()
        val page = gson.fromJson<Paginated<JsonElement>>(json, PaginatedJsonType.TYPE)
        assertEquals(3, page.totalChunks)
        assertEquals("tok-50", page.nextCursor)
        assertTrue(page.hasMore)
        assertEquals("truncated", page.advisory)
    }

    @Test
    fun toleratesUnknownFields() {
        val json = """
            {"chunk":0,"total_chunks":1,"data":42,"future_field":{"x":1},"extra":"ignored"}
        """.trimIndent()
        val page = gson.fromJson<Paginated<JsonElement>>(json, PaginatedJsonType.TYPE)
        assertEquals(42, page.data!!.asInt)
    }

    @Test
    fun healthResponseDecodesKnownFieldsAndIgnoresUnknown() {
        val json = """
            {
              "health_score": 82,
              "status": "green",
              "cycles": 1,
              "dead_exports": 4,
              "twins": 2,
              "hotspots": 3,
              "snapshot_stale": false,
              "snapshot_age_seconds": 120,
              "top_risks": [{"kind":"cycle","file":"a.ts","severity":"high","message":"m"}],
              "recommended_actions": ["run loct"],
              "server_only_future_field": true
            }
        """.trimIndent()
        val health = gson.fromJson(json, HealthResponse::class.java)
        assertEquals(82, health.healthScore)
        assertEquals("green", health.status)
        assertEquals(4, health.deadExports)
        assertEquals(1, health.topRisks.size)
        assertEquals("cycle", health.topRisks.first().kind)
        assertEquals(listOf("run loct"), health.recommendedActions)
    }

    @Test
    fun contextPackParamsSerializeAgentFacingKnobs() {
        val json = gson.toJsonTree(
            ContextPackParams(
                cards = listOf("core", "risk"),
                scope = listOf("path:loctree-lsp"),
                task = "IDE context",
                withAicx = true,
            ),
        ).asJsonObject

        assertEquals("core", json.getAsJsonArray("cards")[0].asString)
        assertEquals("risk", json.getAsJsonArray("cards")[1].asString)
        assertEquals("path:loctree-lsp", json.getAsJsonArray("scope")[0].asString)
        assertEquals("IDE context", json.get("task").asString)
        assertTrue(json.get("with_aicx").asBoolean)
    }

    @Test
    fun fileQueryParamsKeepLegacyFileAndTypedTarget() {
        val json = gson.toJsonTree(FileQueryParams(file = "src/lib.rs")).asJsonObject
        assertEquals("src/lib.rs", json.get("file").asString)
        assertEquals("src/lib.rs", json.get("target").asString)
    }
}
