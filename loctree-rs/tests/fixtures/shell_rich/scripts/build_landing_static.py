#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import re
from html import escape
from pathlib import Path


SITE_ORIGIN = "https://loct.io"

TOP_LEVEL_ROUTES = {
    "/": {
        "title": "Loctree — Map living codebases for AI agents",
        "description": "Loctree maps living codebases for humans and AI agents. Slice dependencies, verify Tauri contracts, spot dead code, and compare source exports to real bundles.",
        "schema": "software",
        "changefreq": "weekly",
        "priority": "1.0",
    },
    "/features": {
        "title": "Loctree Features — Structural truth before you cut",
        "description": "Explore slices, dependency cones, command bridges, dead code checks, and runtime-aware bundle analysis for living codebases.",
        "schema": "page",
        "changefreq": "weekly",
        "priority": "0.8",
    },
    "/docs": {
        "title": "Loctree Docs — Install, scan, and inspect codebases",
        "description": "Get started with loctree, install the CLI, add MCP support, and learn the workflow for mapping live codebases before editing.",
        "schema": "page",
        "changefreq": "weekly",
        "priority": "0.9",
    },
    "/agents-os": {
        "title": "VetCoders Agents OS — Operating surface for AI agent work",
        "description": "Agents OS pairs loctree context, structured implementation, convergence loops, and shipping discipline so agent sessions end in real product movement.",
        "schema": "page",
        "changefreq": "weekly",
        "priority": "0.9",
    },
    "/blog": {
        "title": "Loctree Blog — Real AI agent case studies",
        "description": "Read real AI agent sessions, framework benchmarks, and implementation notes from production loctree work.",
        "schema": "collection",
        "changefreq": "weekly",
        "priority": "0.8",
    },
}

ARTICLE_RE = re.compile(
    r'BlogArticle\s*\{\s*slug:\s*"(?P<slug>[^"]+)",\s*title:\s*"(?P<title>[^"]+)",\s*subtitle:\s*"(?P<subtitle>[^"]+)"',
    re.S,
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate static route entrypoints and sitemap for landing deploys."
    )
    parser.add_argument(
        "target_dir",
        nargs="?",
        default="public_dist",
        help="Directory containing the built landing index.html",
    )
    return parser.parse_args()


def root_dir() -> Path:
    return Path(__file__).resolve().parents[1]


def load_articles(root: Path) -> list[dict[str, str]]:
    content = (root / "landing" / "src" / "content" / "mod.rs").read_text(encoding="utf-8")
    return [match.groupdict() for match in ARTICLE_RE.finditer(content)]


def route_url(route: str) -> str:
    if route == "/":
        return SITE_ORIGIN
    return f"{SITE_ORIGIN}{route.rstrip('/')}/"


def route_output_dir(target_dir: Path, route: str) -> Path:
    if route == "/":
        return target_dir
    return target_dir / route.strip("/")


def route_metadata(route: str, articles: list[dict[str, str]]) -> dict[str, str]:
    if route in TOP_LEVEL_ROUTES:
        return TOP_LEVEL_ROUTES[route]

    slug = route.removeprefix("/blog/")
    article = next((item for item in articles if item["slug"] == slug), None)
    if article is None:
        raise ValueError(f"Unknown blog route: {route}")

    return {
        "title": f"{article['title']} | Loctree Blog",
        "description": article["subtitle"],
        "schema": "article",
        "changefreq": "monthly",
        "priority": "0.7",
    }


def build_schema(route: str, meta: dict[str, str]) -> dict[str, object]:
    url = route_url(route)
    if meta["schema"] == "software":
        return {
            "@context": "https://schema.org",
            "@type": "SoftwareApplication",
            "name": "Loctree",
            "applicationCategory": "DeveloperApplication",
            "operatingSystem": "macOS, Linux",
            "description": meta["description"],
            "url": url,
            "downloadUrl": f"{SITE_ORIGIN}/install.sh",
            "softwareVersion": "0.8.16",
            "creator": {"@type": "Organization", "name": "VetCoders"},
        }
    if meta["schema"] == "collection":
        return {
            "@context": "https://schema.org",
            "@type": "CollectionPage",
            "name": meta["title"],
            "description": meta["description"],
            "url": url,
            "isPartOf": {"@type": "WebSite", "name": "Loctree", "url": SITE_ORIGIN},
        }
    if meta["schema"] == "article":
        return {
            "@context": "https://schema.org",
            "@type": "Article",
            "headline": meta["title"].replace(" | Loctree Blog", ""),
            "description": meta["description"],
            "url": url,
            "author": {"@type": "Organization", "name": "VetCoders"},
            "publisher": {"@type": "Organization", "name": "VetCoders"},
            "isPartOf": {"@type": "Blog", "name": "Loctree Blog", "url": f"{SITE_ORIGIN}/blog"},
        }

    return {
        "@context": "https://schema.org",
        "@type": "WebPage",
        "name": meta["title"],
        "description": meta["description"],
        "url": url,
        "isPartOf": {"@type": "WebSite", "name": "Loctree", "url": SITE_ORIGIN},
    }


def replace_once(text: str, pattern: str, replacement: str) -> str:
    updated, count = re.subn(pattern, replacement, text, count=1, flags=re.S)
    if count != 1:
        raise RuntimeError(f"Expected one match for pattern: {pattern}")
    return updated


def replace_attr(text: str, attr_selector: str, value: str) -> str:
    escaped_value = escape(value, quote=True)
    pattern = rf'(<meta {attr_selector} content=")([^"]*)(">)'
    return replace_once(text, pattern, rf"\g<1>{escaped_value}\g<3>")


def render_page(template: str, route: str, meta: dict[str, str]) -> str:
    url = route_url(route)
    rendered = template
    rendered = replace_once(rendered, r"(<html lang=\")([^\"]+)(\">)", r"\g<1>en\g<3>")
    rendered = replace_once(rendered, r"<title>.*?</title>", f"<title>{escape(meta['title'])}</title>")
    rendered = replace_attr(rendered, r'name="description"', meta["description"])
    rendered = replace_attr(rendered, r'property="og:title"', meta["title"])
    rendered = replace_attr(rendered, r'property="og:description"', meta["description"])
    rendered = replace_attr(rendered, r'property="og:type"', "article" if meta["schema"] == "article" else "website")
    rendered = replace_attr(rendered, r'property="og:url"', url)
    rendered = replace_attr(rendered, r'name="twitter:title"', meta["title"])
    rendered = replace_attr(rendered, r'name="twitter:description"', meta["description"])
    rendered = replace_once(
        rendered,
        r'(<link rel="canonical" href=")([^"]*)(">)',
        rf'\g<1>{escape(url, quote=True)}\g<3>',
    )
    schema = json.dumps(build_schema(route, meta), ensure_ascii=False, indent=4)
    rendered = replace_once(
        rendered,
        r'(<script type="application/ld\+json">\s*)(.*?)(\s*</script>)',
        rf"\g<1>{schema}\g<3>",
    )
    return rendered


def write_route(target_dir: Path, template: str, route: str, meta: dict[str, str]) -> None:
    destination = route_output_dir(target_dir, route)
    destination.mkdir(parents=True, exist_ok=True)
    (destination / "index.html").write_text(render_page(template, route, meta), encoding="utf-8")


def write_not_found(target_dir: Path, template: str) -> None:
    not_found = render_page(
        template,
        "/",
        {
            "title": "Loctree — Page not found",
            "description": "The requested page was not found, but the Loctree landing shell is still available.",
            "schema": "page",
        },
    )
    not_found = replace_once(
        not_found,
        r'(<meta name="robots" content=")([^"]*)(">)',
        r"\g<1>noindex,nofollow\g<3>",
    )
    (target_dir / "404.html").write_text(not_found, encoding="utf-8")


def write_sitemap(target_dir: Path, routes: list[tuple[str, dict[str, str]]]) -> None:
    lines = ['<?xml version="1.0" encoding="UTF-8"?>', '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">']
    for route, meta in routes:
        lines.extend(
            [
                "  <url>",
                f"    <loc>{route_url(route)}</loc>",
                f"    <changefreq>{meta['changefreq']}</changefreq>",
                f"    <priority>{meta['priority']}</priority>",
                "  </url>",
            ]
        )
    lines.append("</urlset>")
    (target_dir / "sitemap.xml").write_text("\n".join(lines) + "\n", encoding="utf-8")


def main() -> None:
    args = parse_args()
    root = root_dir()
    target_dir = (root / args.target_dir).resolve()
    index_html = target_dir / "index.html"

    if not index_html.exists():
        raise SystemExit(f"Missing built landing shell: {index_html}")

    template = index_html.read_text(encoding="utf-8")
    articles = load_articles(root)
    route_pairs = [(route, route_metadata(route, articles)) for route in TOP_LEVEL_ROUTES]
    route_pairs.extend(
        (f"/blog/{article['slug']}", route_metadata(f"/blog/{article['slug']}", articles))
        for article in articles
    )

    for route, meta in route_pairs:
        write_route(target_dir, template, route, meta)

    write_not_found(target_dir, template)
    write_sitemap(target_dir, route_pairs)
    print(f"[landing-static] Generated {len(route_pairs)} route entrypoints in {target_dir}")


if __name__ == "__main__":
    main()
