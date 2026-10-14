/*
 * Health ring + compact risk feed for the Loctree tool window hero.
 *
 * 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI
 */

package io.loct.intellij.toolwindow

import com.intellij.ui.JBColor
import com.intellij.util.ui.JBUI
import com.intellij.util.ui.UIUtil
import io.loct.intellij.LoctreeBundle
import io.loct.intellij.LoctreeColors
import io.loct.intellij.protocol.HealthResponse
import io.loct.intellij.protocol.RiskItem
import io.loct.intellij.util.FileNavigator
import com.intellij.openapi.project.Project
import java.awt.BasicStroke
import java.awt.BorderLayout
import java.awt.Color
import java.awt.Cursor
import java.awt.Dimension
import java.awt.Font
import java.awt.Graphics
import java.awt.Graphics2D
import java.awt.RenderingHints
import java.awt.event.MouseAdapter
import java.awt.event.MouseEvent
import java.awt.geom.Arc2D
import java.awt.geom.Ellipse2D
import javax.swing.Box
import javax.swing.BoxLayout
import javax.swing.JComponent
import javax.swing.JLabel
import javax.swing.JPanel
import javax.swing.SwingConstants

/** Score ring + status label. */
internal class HealthRing : JComponent() {
    private var score: Int? = null
    private var status: String = "—"
    private var accent: Color = LoctreeColors.BONE_MUTE

    init {
        preferredSize = Dimension(JBUI.scale(88), JBUI.scale(88))
        minimumSize = preferredSize
        toolTipText = LoctreeBundle.message("toolwindow.health.tooltip.empty")
    }

    fun update(health: HealthResponse?) {
        val measuredScore = health?.healthScore
        if (health == null || measuredScore == null) {
            score = null
            status = health?.status?.ifBlank { "unknown" } ?: "—"
            accent = LoctreeColors.BONE_MUTE
            toolTipText = LoctreeBundle.message("toolwindow.health.tooltip.empty")
        } else {
            score = measuredScore.coerceIn(0, 100)
            status = health.status.ifBlank { "unknown" }
            accent = when {
                measuredScore >= 80 -> LoctreeColors.TEAL
                measuredScore >= 50 -> LoctreeColors.AMBER
                else -> LoctreeColors.DANGER
            }
            toolTipText = LoctreeBundle.message(
                "toolwindow.health.tooltip.score",
                health.healthScore,
                health.status,
            )
        }
        repaint()
    }

    override fun paintComponent(g: Graphics) {
        val g2 = g.create() as Graphics2D
        try {
            g2.setRenderingHint(RenderingHints.KEY_ANTIALIASING, RenderingHints.VALUE_ANTIALIAS_ON)
            val pad = JBUI.scale(6).toFloat()
            val size = minOf(width, height).toFloat() - pad * 2
            val x = (width - size) / 2f
            val y = (height - size) / 2f
            val track = if (JBColor.isBright()) Color(0xe6e2d8) else Color(0x2a2a26)
            g2.stroke = BasicStroke(JBUI.scale(6).toFloat(), BasicStroke.CAP_ROUND, BasicStroke.JOIN_ROUND)
            g2.color = track
            g2.draw(Ellipse2D.Float(x, y, size, size))
            val s = score
            if (s != null && s > 0) {
                val extent = -(s / 100.0 * 360.0)
                g2.color = accent
                g2.draw(Arc2D.Float(x, y, size, size, 90f, extent.toFloat(), Arc2D.OPEN))
            }
            val scoreText = s?.toString() ?: "—"
            g2.font = UIUtil.getLabelFont().deriveFont(Font.BOLD, JBUI.scale(18).toFloat())
            g2.color = if (s == null) LoctreeColors.BONE_MUTE else accent
            val fm = g2.fontMetrics
            g2.drawString(scoreText, (width - fm.stringWidth(scoreText)) / 2, height / 2 + fm.ascent / 3)
            g2.font = UIUtil.getLabelFont().deriveFont(Font.BOLD, JBUI.scale(9).toFloat())
            g2.color = LoctreeColors.BONE_MUTE
            val st = status.uppercase().take(10)
            val fm2 = g2.fontMetrics
            g2.drawString(st, (width - fm2.stringWidth(st)) / 2, height / 2 + fm.ascent / 3 + JBUI.scale(14))
        } finally {
            g2.dispose()
        }
    }
}

/** Up to 5 clickable top-risk rows. */
internal class RiskFeed(private val project: Project) : JPanel() {
    private val list = JPanel().apply {
        isOpaque = false
        layout = BoxLayout(this, BoxLayout.Y_AXIS)
    }

    init {
        isOpaque = false
        layout = BorderLayout()
        border = JBUI.Borders.empty(0, 4, 0, 0)
        val eyebrow = monoEyebrow(LoctreeBundle.message("toolwindow.risks.eyebrow"))
        add(eyebrow, BorderLayout.NORTH)
        add(list, BorderLayout.CENTER)
    }

    fun update(risks: List<RiskItem>) {
        list.removeAll()
        if (risks.isEmpty()) {
            list.add(
                JLabel(LoctreeBundle.message("toolwindow.risks.empty")).apply {
                    foreground = LoctreeColors.BONE_MUTE
                    font = UIUtil.getLabelFont().deriveFont(Font.ITALIC, 11f)
                    border = JBUI.Borders.empty(4, 2)
                },
            )
        } else {
            risks.take(5).forEach { risk ->
                list.add(riskRow(risk))
                list.add(Box.createVerticalStrut(JBUI.scale(2)))
            }
        }
        revalidate()
        repaint()
    }

    private fun riskRow(risk: RiskItem): JComponent {
        val severity = risk.severity.ifBlank { "normal" }
        val color = when {
            severity.contains("high", ignoreCase = true) || severity.contains("critical", ignoreCase = true) ->
                LoctreeColors.DANGER
            severity.contains("med", ignoreCase = true) || severity.contains("warn", ignoreCase = true) ->
                LoctreeColors.AMBER
            else -> LoctreeColors.TEAL
        }
        val text = buildString {
            append(risk.message.ifBlank { risk.kind.ifBlank { "risk" } })
            if (risk.file.isNotBlank()) {
                append("  ·  ")
                append(risk.file.substringAfterLast('/').substringAfterLast('\\'))
            }
        }
        return JLabel(text).apply {
            foreground = color
            font = UIUtil.getLabelFont().deriveFont(Font.PLAIN, 11f)
            border = JBUI.Borders.empty(2, 2)
            toolTipText = listOfNotNull(
                severity.uppercase(),
                risk.kind.takeIf { it.isNotBlank() },
                risk.message,
                risk.file.takeIf { it.isNotBlank() },
            ).joinToString("\n")
            if (risk.file.isNotBlank()) {
                cursor = Cursor.getPredefinedCursor(Cursor.HAND_CURSOR)
                addMouseListener(object : MouseAdapter() {
                    override fun mouseClicked(e: MouseEvent) {
                        FileNavigator.navigate(project, risk.file, null)
                    }
                })
            }
            horizontalAlignment = SwingConstants.LEFT
            alignmentX = LEFT_ALIGNMENT
        }
    }
}

/** Ring + metrics + risk feed row. */
internal class HealthHeroPanel(project: Project) : JPanel(BorderLayout(10, 0)) {
    val ring = HealthRing()
    private val riskFeed = RiskFeed(project)
    private val metricsHost = JPanel(BorderLayout()).apply { isOpaque = false }

    init {
        isOpaque = false
        border = JBUI.Borders.empty(4, 12, 4, 12)
        val left = JPanel(BorderLayout()).apply {
            isOpaque = false
            add(ring, BorderLayout.CENTER)
            preferredSize = Dimension(JBUI.scale(96), JBUI.scale(100))
        }
        add(left, BorderLayout.WEST)
        add(metricsHost, BorderLayout.CENTER)
        add(riskFeed, BorderLayout.EAST)
        riskFeed.preferredSize = Dimension(JBUI.scale(220), JBUI.scale(100))
    }

    fun setMetricsComponent(metrics: JComponent) {
        metricsHost.removeAll()
        metricsHost.add(metrics, BorderLayout.CENTER)
        metricsHost.revalidate()
    }

    fun updateHealth(health: HealthResponse?) {
        ring.update(health)
        riskFeed.update(health?.topRisks.orEmpty())
    }
}
