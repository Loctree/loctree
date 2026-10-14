/*
 * Multi-language path heuristic tests.
 *
 * 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI
 */

package io.loct.intellij.toolwindow

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PathHeuristicTest {

    @Test
    fun acceptsCommonLanguageExtensionsWithoutDirectory() {
        for (name in listOf(
            "App.swift",
            "main.ts",
            "widget.tsx",
            "server.py",
            "handler.go",
            "lib.rs",
            "Main.kt",
            "ViewController.m",
            "styles.css",
            "schema.graphql",
        )) {
            assertTrue("$name must look like a path", PathHeuristic.looksLikeFilePath(name))
        }
    }

    @Test
    fun acceptsPathsWithSeparatorsRegardlessOfExtension() {
        assertTrue(PathHeuristic.looksLikeFilePath("src/unknown.weird"))
        assertTrue(PathHeuristic.looksLikeFilePath("crates\\foo\\bar"))
    }

    @Test
    fun rejectsBareIdentifiers() {
        assertFalse(PathHeuristic.looksLikeFilePath("AuthService"))
        assertFalse(PathHeuristic.looksLikeFilePath("numberOfRows"))
        assertFalse(PathHeuristic.looksLikeFilePath(""))
    }

    @Test
    fun normalizesFileUri() {
        val uri = "file:///home/me/repo/src/App.swift"
        assertTrue(PathHeuristic.looksLikeFilePath(uri))
        val norm = PathHeuristic.normalizeFileRef(uri)
        assertTrue(norm.contains("App.swift"))
        assertFalse(norm.startsWith("file:"))
    }
}
