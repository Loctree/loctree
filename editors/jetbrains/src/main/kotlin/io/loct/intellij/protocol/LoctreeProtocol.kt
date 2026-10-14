/*
 * Kotlin mirror of the loctree-lsp wire contract.
 *
 * Field names mirror loctree-lsp/src/protocol.rs (Paginated<T>) and
 * loctree-lsp/src/health.rs (HealthResponse / RiskItem). Decoding uses
 * Gson, which ignores unknown JSON fields by default — the server may
 * add fields without breaking the plugin.
 *
 * 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Vetcoders
 */

package io.loct.intellij.protocol

import com.google.gson.annotations.SerializedName

/**
 * Wire envelope for paginated LSP responses.
 *
 * Mirrors `loctree-lsp/src/protocol.rs::Paginated<T>`. Single-shot
 * responses carry `chunk = 0`, `totalChunks = 1`, `nextCursor = null`.
 */
data class Paginated<T>(
    @SerializedName("chunk") val chunk: Int = 0,
    @SerializedName("total_chunks") val totalChunks: Int = 1,
    @SerializedName("next_cursor") val nextCursor: String? = null,
    @SerializedName("data") val data: T? = null,
    @SerializedName("advisory") val advisory: String? = null,
) {
    val hasMore: Boolean get() = nextCursor != null
}

/** Parameters for `loctree/health`. */
data class HealthParams(
    @SerializedName("project") val project: String? = null,
    @SerializedName("include_top_risks") val includeTopRisks: Boolean = false,
)

/** One entry in `HealthResponse.top_risks`. */
data class RiskItem(
    @SerializedName("kind") val kind: String = "",
    @SerializedName("file") val file: String = "",
    @SerializedName("severity") val severity: String = "",
    @SerializedName("message") val message: String = "",
)

/** Mirror of `loctree-lsp/src/health.rs::HealthResponse`. */
data class HealthResponse(
    @SerializedName("health_score") val healthScore: Int? = null,
    @SerializedName("status") val status: String = "",
    @SerializedName("cycles") val cycles: Int = 0,
    @SerializedName("dead_exports") val deadExports: Int = 0,
    @SerializedName("twins") val twins: Int = 0,
    @SerializedName("hotspots") val hotspots: Int = 0,
    @SerializedName("snapshot_stale") val snapshotStale: Boolean = false,
    @SerializedName("snapshot_age_seconds") val snapshotAgeSeconds: Long = 0,
    @SerializedName("top_risks") val topRisks: List<RiskItem> = emptyList(),
    @SerializedName("recommended_actions") val recommendedActions: List<String> = emptyList(),
) {
    val healthScoreLabel: String get() = healthScore?.let { "$it/100" } ?: "unknown"
}

/** Common shape for file-scoped custom requests (find/impact/slice/follow). */
data class FileQueryParams(
    @SerializedName("project") val project: String? = null,
    @SerializedName("file") val file: String? = null,
    @SerializedName("target") val target: String? = file,
    @SerializedName("symbol") val symbol: String? = null,
    @SerializedName("scope") val scope: String? = null,
    @SerializedName("handler") val handler: String? = null,
    @SerializedName("cursor") val cursor: String? = null,
    @SerializedName("chunk_size") val chunkSize: Int? = null,
    @SerializedName("query") val query: String? = null,
    @SerializedName("mode") val mode: String? = null,
    @SerializedName("lang") val lang: String? = null,
    @SerializedName("dead_only") val deadOnly: Boolean = false,
    @SerializedName("exported_only") val exportedOnly: Boolean = false,
    @SerializedName("whole_token") val wholeToken: Boolean = false,
    @SerializedName("group_by_file") val groupByFile: Boolean = false,
    @SerializedName("count_only") val countOnly: Boolean = false,
    @SerializedName("slim") val slim: Boolean = false,
    @SerializedName("limit") val limit: Int? = null,
    @SerializedName("offset") val offset: Int = 0,
    @SerializedName("transitive") val transitive: Boolean = false,
    @SerializedName("consumers") val consumers: Boolean = false,
    @SerializedName("depth") val depth: Int? = null,
)

/** Parameters for `loctree/body` (mirrors `loctree-lsp/src/body.rs::BodyParams`). */
data class BodyParams(
    @SerializedName("symbol") val symbol: String,
    @SerializedName("max_lines") val maxLines: Int? = null,
    @SerializedName("file") val file: String? = null,
    @SerializedName("project") val project: String? = null,
)

/** Parameters for `loctree/contextAtlas` / atlas-card navigation. */
data class ContextAtlasParams(
    @SerializedName("project") val project: String? = null,
)

/** Parameters for `loctree/contextPack`. */
data class ContextPackParams(
    @SerializedName("project") val project: String? = null,
    @SerializedName("cursor") val cursor: String? = null,
    @SerializedName("cards") val cards: List<String>? = null,
    @SerializedName("scope") val scope: List<String>? = null,
    @SerializedName("task") val task: String? = null,
    @SerializedName("with_aicx") val withAicx: Boolean? = null,
    @SerializedName("no_aicx") val noAicx: Boolean = false,
)

/** Empty params for `loctree/workspaces`. */
data class WorkspacesParams(
    @SerializedName("_unused") val unused: Boolean? = null,
)

/** Parameters for `loctree/diff`. */
data class DiffParams(
    @SerializedName("since") val since: String = "lastQuery",
    @SerializedName("project") val project: String? = null,
    @SerializedName("cursor") val cursor: String? = null,
    @SerializedName("chunk_size") val chunkSize: Int? = null,
)

/** Parameters for `loctree/semantic`. */
data class SemanticParams(
    @SerializedName("scope") val scope: String = "project",
    @SerializedName("target") val target: String? = null,
    @SerializedName("kinds") val kinds: List<String>? = null,
    @SerializedName("project") val project: String? = null,
    @SerializedName("cursor") val cursor: String? = null,
    @SerializedName("chunk_size") val chunkSize: Int? = null,
)

/** Parameters for read-only `loctree/aicx`. */
data class AicxParams(
    @SerializedName("scope") val scope: String = "project",
    @SerializedName("target") val target: String? = null,
    @SerializedName("kinds") val kinds: List<String>? = null,
    @SerializedName("hours") val hours: Long? = null,
    @SerializedName("limit") val limit: Int? = null,
    @SerializedName("project") val project: String? = null,
)

/** Scope selector for `loctree/astQuery`. */
data class AstQueryScope(
    @SerializedName("paths") val paths: List<String>? = null,
    @SerializedName("glob") val glob: String? = null,
)

/** Parameters for `loctree/astQuery`. */
data class AstQueryParams(
    @SerializedName("language") val language: String = "auto",
    @SerializedName("query") val query: String,
    @SerializedName("scope") val scope: AstQueryScope = AstQueryScope(),
    @SerializedName("limit") val limit: Int? = null,
    @SerializedName("project") val project: String? = null,
)

/** Response shape for the `openAtlasCard` execute-command. */
data class OpenAtlasCardResponse(
    @SerializedName("ok") val ok: Boolean = false,
    @SerializedName("card_path") val cardPath: String? = null,
)
