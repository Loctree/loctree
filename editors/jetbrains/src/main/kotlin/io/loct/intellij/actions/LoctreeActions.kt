/*
 * Loctree IDE actions.
 *
 * Wires the editor/project-view context actions and the tool-window
 * title actions through LoctreeLspGateway. LSP requests run on a
 * background task so the EDT is never blocked. File-scoped actions are
 * enabled only for file-backed paths inside the workspace.
 *
 * 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Vetcoders
 */

package io.loct.intellij.actions

import com.intellij.ide.BrowserUtil
import com.intellij.openapi.actionSystem.ActionUpdateThread
import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.actionSystem.CommonDataKeys
import com.intellij.openapi.progress.ProgressIndicator
import com.intellij.openapi.progress.ProgressManager
import com.intellij.openapi.progress.Task
import com.intellij.openapi.project.Project
import com.intellij.openapi.ui.Messages
import com.intellij.openapi.vfs.VirtualFile
import io.loct.intellij.LoctreeBundle
import io.loct.intellij.lsp.LoctreeLspGateway
import io.loct.intellij.protocol.HealthParams
import io.loct.intellij.toolwindow.LoctreeQueryRouter
import io.loct.intellij.toolwindow.LoctreeToolWindowBridge
import io.loct.intellij.toolwindow.QueryMode
import io.loct.intellij.util.LoctreeNotifier
import io.loct.intellij.util.WorkspacePaths
import java.nio.file.Path

/** Base for project-scoped actions. */
abstract class LoctreeProjectAction : AnAction() {
    final override fun actionPerformed(e: AnActionEvent) {
        val project = e.project ?: return
        perform(project, e)
    }

    abstract fun perform(project: Project, e: AnActionEvent)

    override fun getActionUpdateThread(): ActionUpdateThread = ActionUpdateThread.BGT

    protected fun gateway(project: Project): LoctreeLspGateway =
        LoctreeLspGateway.getInstance(project)

    protected fun runRequest(project: Project, title: String, body: () -> String) {
        ProgressManager.getInstance().run(object : Task.Backgroundable(project, title, false) {
            override fun run(indicator: ProgressIndicator) {
                val message = runCatching { body() }
                    .getOrElse { "Loctree: ${it.message}" }
                LoctreeNotifier.info(project, message)
            }
        })
    }

    /**
     * Run a routed Loctree query and publish its result into the tool window,
     * sharing one renderer with the inline query console instead of bespoke
     * notification summaries. The notification stays a lightweight completion
     * hint; failures and a missing LSP fall back to a plain notice.
     */
    internal fun routeToToolWindow(project: Project, mode: QueryMode, query: String, label: String) {
        runRequest(project, label) {
            val request = LoctreeQueryRouter.requestFor(mode, query)
            val response = gateway(project).customRequest(request.method, request.params)
                ?: return@runRequest LoctreeBundle.message("notify.lspNotRunning")
            LoctreeToolWindowBridge.showResult(project, request, response)
            LoctreeBundle.message("notify.resultsInToolWindow", label)
        }
    }
}

/** Base for file-scoped actions; only enabled for in-workspace files. */
abstract class LoctreeFileAction : LoctreeProjectAction() {
    final override fun perform(project: Project, e: AnActionEvent) {
        val filePath = selectedFile(project, e)
        if (filePath == null) {
            LoctreeNotifier.warn(project, LoctreeBundle.message("notify.noFileSelected"))
            return
        }
        performForFile(project, filePath)
    }

    abstract fun performForFile(project: Project, filePath: String)

    override fun update(e: AnActionEvent) {
        val project = e.project
        e.presentation.isEnabled = project != null && selectedFile(project, e) != null
    }

    protected fun selectedFile(project: Project, e: AnActionEvent): String? {
        val file: VirtualFile = e.getData(CommonDataKeys.VIRTUAL_FILE) ?: return null
        if (file.isDirectory) return null
        val base = project.basePath?.let { Path.of(it) } ?: return null
        val safe = WorkspacePaths.toWorkspaceAbsolutePath(file.path, base) ?: return null
        return safe.toString()
    }
}

// Project-wide actions ------------------------------------------------------

class RefreshAction : LoctreeProjectAction() {
    override fun perform(project: Project, e: AnActionEvent) {
        if (gateway(project).refresh()) {
            LoctreeNotifier.info(project, LoctreeBundle.message("notify.refreshRequested"))
        } else {
            LoctreeNotifier.warn(project, LoctreeBundle.message("notify.lspNotRunning"))
        }
    }
}

class ShowHealthAction : LoctreeProjectAction() {
    override fun perform(project: Project, e: AnActionEvent) {
        runRequest(project, LoctreeBundle.message("action.io.loct.intellij.actions.ShowHealthAction.text")) {
            val health = gateway(project).health(HealthParams(includeTopRisks = true))
                ?: return@runRequest LoctreeBundle.message("notify.lspNotRunning")
            "Loctree Health: ${health.healthScoreLabel} (${health.status}) — " +
                "${health.deadExports} Dead, ${health.cycles} Cycles, ${health.twins} Twins"
        }
    }
}

class ShowCyclesAction : LoctreeProjectAction() {
    override fun perform(project: Project, e: AnActionEvent) {
        if (!gateway(project).refresh()) {
            LoctreeNotifier.warn(project, LoctreeBundle.message("notify.lspNotRunning"))
        }
    }
}

class AnalyzeCycleAction : LoctreeProjectAction() {
    override fun perform(project: Project, e: AnActionEvent) {
        if (!gateway(project).refresh()) {
            LoctreeNotifier.warn(project, LoctreeBundle.message("notify.lspNotRunning"))
        }
    }
}

class OpenReportAction : LoctreeProjectAction() {
    override fun perform(project: Project, e: AnActionEvent) {
        val base = project.basePath
        if (base == null) {
            LoctreeNotifier.warn(project, LoctreeBundle.message("notify.reportMissing"))
            return
        }
        val report = Path.of(base, ".loctree", "report.html")
        if (!java.nio.file.Files.isRegularFile(report)) {
            LoctreeNotifier.warn(project, LoctreeBundle.message("notify.reportMissing"))
            gateway(project).refresh()
            return
        }
        BrowserUtil.browse(report.toUri())
    }
}

class FindLiteralOccurrencesAction : LoctreeProjectAction() {
    override fun perform(project: Project, e: AnActionEvent) {
        val selectedText = e.getData(CommonDataKeys.EDITOR)?.selectionModel?.selectedText?.trim()
        val query = Messages.showInputDialog(
            project,
            LoctreeBundle.message("dialog.literalSearch.prompt"),
            LoctreeBundle.message("action.io.loct.intellij.actions.FindLiteralOccurrencesAction.text"),
            Messages.getQuestionIcon(),
            selectedText.orEmpty(),
            null,
        )?.trim().orEmpty()
        if (query.isEmpty()) return

        runRequest(project, LoctreeBundle.message("action.io.loct.intellij.actions.FindLiteralOccurrencesAction.text")) {
            val label = LoctreeBundle.message("action.io.loct.intellij.actions.FindLiteralOccurrencesAction.text")
            val request = LoctreeQueryRouter.requestFor(QueryMode.Literal, query)
            val response = gateway(project).customRequest(request.method, request.params)
                ?: return@runRequest LoctreeBundle.message("notify.lspNotRunning")
            LoctreeToolWindowBridge.showResult(project, request, response)
            LoctreeBundle.message("notify.resultsInToolWindow", label)
        }
    }
}

// File-scoped actions -------------------------------------------------------

/**
 * Parity guard (W3): the LSP server (loctree-lsp/src/actions/refactor.rs) emits
 * these exact command strings in CodeActions. Both editors/vscode (package.json
 * + commands.ts) and editors/jetbrains (this file + loctree-lsp.xml) MUST
 * contribute handlers for every one of them, or "cannot execute" happens in one
 * editor. Keep this list, the <action id="loctree.*"> in loctree-lsp.xml, and
 * the vscode contributes in sync. A future test can cross-check the strings.
 */
object ContributedLspCommands {
    /** Every `command: "loctree.*"` emitted by loctree-lsp refactor.rs + quickfix.rs. */
    val ALL = listOf(
        "loctree.analyzeImpact",
        "loctree.findImporters",
        "loctree.openReport",
        "loctree.checkDeadExports",
        "loctree.showCycles",
        "loctree.analyzeCycle",
        "loctree.showCycleDetails",
        "loctree.navigateToFile",
        "loctree.ignoreCycle",
    )
}

class AnalyzeImpactAction : LoctreeFileAction() {
    override fun performForFile(project: Project, filePath: String) {
        routeToToolWindow(
            project,
            QueryMode.Impact,
            filePath,
            LoctreeBundle.message("action.io.loct.intellij.actions.AnalyzeImpactAction.text"),
        )
    }
}

class ShowSliceAction : LoctreeFileAction() {
    override fun performForFile(project: Project, filePath: String) {
        routeToToolWindow(
            project,
            QueryMode.Slice,
            filePath,
            LoctreeBundle.message("action.io.loct.intellij.actions.ShowSliceAction.text"),
        )
    }
}

class FindConsumersAction : LoctreeFileAction() {
    override fun performForFile(project: Project, filePath: String) {
        routeToToolWindow(
            project,
            QueryMode.Impact,
            filePath,
            LoctreeBundle.message("action.io.loct.intellij.actions.FindConsumersAction.text"),
        )
    }
}

class FindImportersAction : LoctreeFileAction() {
    override fun performForFile(project: Project, filePath: String) {
        routeToToolWindow(
            project,
            QueryMode.Impact,
            filePath,
            LoctreeBundle.message("action.io.loct.intellij.actions.FindImportersAction.text"),
        )
    }
}

/**
 * Routes through `follow(dead)` — a working project-scoped dead-export surface.
 * The prior file-scoped `find(file=...)` call could not satisfy the LSP `find`
 * contract (which requires a `query`), so it returned nothing; the routed
 * follow result renders richly in the tool window instead.
 */
class CheckDeadExportsAction : LoctreeFileAction() {
    override fun performForFile(project: Project, filePath: String) {
        routeToToolWindow(
            project,
            QueryMode.Follow,
            "dead",
            LoctreeBundle.message("action.io.loct.intellij.actions.CheckDeadExportsAction.text"),
        )
    }
}
